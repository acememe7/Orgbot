//! # orgbot-ffi
//!
//! A C ABI over the room engine, so the phone app can run a room in-process
//! instead of talking to one hosted elsewhere.
//!
//! The shape is deliberately dull: strings in, JSON out, and a poll the caller
//! drains when it likes. No callbacks cross the boundary — a callback into
//! Dart from a Tokio worker thread has to be marshalled back to the right
//! isolate, and getting that wrong is a crash rather than a bug. Polling is
//! slower and much harder to break.
//!
//! Every returned string is owned by the caller and must be released with
//! [`orgbot_string_free`].
//!
//! ```c
//! uint64_t room = orgbot_room_open(toml, "ANTHROPIC_AUTH_TOKEN=...");
//! orgbot_room_say(room, "you", "how is it going?");
//! char* events = orgbot_room_poll(room);   // JSON array
//! orgbot_string_free(events);
//! orgbot_room_close(room);
//! ```

// This crate exists to cross a C boundary, so it is the one place unsafe is
// unavoidable. Every unsafe block below documents what the caller must
// guarantee for it to be sound.
#![allow(unsafe_code)]
#![warn(missing_debug_implementations, rust_2018_idioms)]

pub mod room;

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use room::Room;

/// Live rooms, keyed by the handle handed to the caller.
///
/// A handle rather than a raw pointer: a stale integer is a clean "no such
/// room" error, where a stale pointer is undefined behaviour.
static ROOMS: Mutex<Option<HashMap<u64, Room>>> = Mutex::new(None);

/// Source of handles. Never reused, so a double close cannot hit a new room.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The last error, for a caller that got a null or a zero back.
    ///
    /// Per-thread, like `errno`: a single global would let one caller consume
    /// another's error, which turns a clear failure into a confusing one as
    /// soon as two threads use the library at once.
    static LAST_ERROR: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn set_error(message: impl Into<String>) {
    let message = message.into();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(message));
}

fn with_rooms<T>(f: impl FnOnce(&mut HashMap<u64, Room>) -> T) -> Option<T> {
    let mut guard = ROOMS.lock().ok()?;
    Some(f(guard.get_or_insert_with(HashMap::new)))
}

/// Copy a Rust string out for the caller to own.
fn out(value: String) -> *mut c_char {
    match CString::new(value) {
        Ok(text) => text.into_raw(),
        // A string with an interior NUL cannot cross a C boundary; report it
        // rather than truncating silently.
        Err(_) => {
            set_error("value contained a NUL byte and could not be returned");
            std::ptr::null_mut()
        }
    }
}

/// Read a C string the caller owns.
///
/// # Safety
/// `ptr` must be null or a NUL-terminated string valid for this call.
unsafe fn borrow<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a valid NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Parse `KEY=value` pairs separated by newlines.
fn parse_environment(raw: Option<&str>) -> Vec<(String, String)> {
    raw.unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// Open a room from a TOML room file.
///
/// `environment` is optional `KEY=value` lines carrying credentials, so a key
/// or OAuth token never has to be written into the room file. Returns a handle,
/// or `0` on failure — call [`orgbot_last_error`] for the reason.
///
/// # Safety
/// Both arguments must be null or valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn orgbot_room_open(
    toml: *const c_char,
    environment: *const c_char,
) -> u64 {
    // SAFETY: the caller guarantees valid strings; see the doc comment.
    let Some(toml) = (unsafe { borrow(toml) }) else {
        set_error("a room file is required");
        return 0;
    };
    // SAFETY: as above.
    let environment = parse_environment(unsafe { borrow(environment) });

    match Room::open(toml, &environment) {
        Ok(room) => {
            let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
            match with_rooms(|rooms| rooms.insert(handle, room)) {
                Some(_) => handle,
                None => {
                    set_error("the room registry was poisoned");
                    0
                }
            }
        }
        Err(error) => {
            set_error(error);
            0
        }
    }
}

/// Everything the room has broadcast since the last poll, as a JSON array.
///
/// Returns null on an unknown handle. The caller owns the result.
#[no_mangle]
pub extern "C" fn orgbot_room_poll(handle: u64) -> *mut c_char {
    let events = with_rooms(|rooms| rooms.get(&handle).map(|room| room.drain()));
    match events.flatten() {
        Some(events) => match serde_json::to_string(&events) {
            Ok(json) => out(json),
            Err(error) => {
                set_error(format!("could not encode events: {error}"));
                std::ptr::null_mut()
            }
        },
        None => {
            set_error(format!("no room {handle}"));
            std::ptr::null_mut()
        }
    }
}

/// The room's current state, as a JSON snapshot. The caller owns the result.
#[no_mangle]
pub extern "C" fn orgbot_room_snapshot(handle: u64) -> *mut c_char {
    let snapshot = with_rooms(|rooms| rooms.get(&handle).and_then(|room| room.snapshot()));
    match snapshot.flatten() {
        Some(message) => match serde_json::to_string(&message) {
            Ok(json) => out(json),
            Err(error) => {
                set_error(format!("could not encode the snapshot: {error}"));
                std::ptr::null_mut()
            }
        },
        None => {
            set_error(format!("no room {handle}, or it has stopped"));
            std::ptr::null_mut()
        }
    }
}

/// Say something to the room as a person. Returns 1 on success, 0 otherwise.
///
/// # Safety
/// `author` and `body` must be valid NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn orgbot_room_say(
    handle: u64,
    author: *const c_char,
    body: *const c_char,
) -> i32 {
    // SAFETY: the caller guarantees valid strings; see the doc comment.
    let (Some(author), Some(body)) = (unsafe { borrow(author) }, unsafe { borrow(body) }) else {
        set_error("author and body are both required");
        return 0;
    };
    let sent = with_rooms(|rooms| rooms.get(&handle).map(|room| room.say(author, body)));
    match sent.flatten() {
        Some(true) => 1,
        _ => {
            set_error(format!("no room {handle}, or it has stopped"));
            0
        }
    }
}

/// Pause the room. Returns 1 on success.
#[no_mangle]
pub extern "C" fn orgbot_room_pause(handle: u64) -> i32 {
    let ok = with_rooms(|rooms| rooms.get(&handle).map(|room| room.pause("paused from the app")));
    matches!(ok.flatten(), Some(true)) as i32
}

/// Resume a paused room. Returns 1 on success.
#[no_mangle]
pub extern "C" fn orgbot_room_resume(handle: u64) -> i32 {
    let ok = with_rooms(|rooms| rooms.get(&handle).map(|room| room.resume()));
    matches!(ok.flatten(), Some(true)) as i32
}

/// Stop a room and release it. Closing an unknown handle is not an error.
#[no_mangle]
pub extern "C" fn orgbot_room_close(handle: u64) {
    let _ = with_rooms(|rooms| rooms.remove(&handle));
}

/// The last error message, or null if there has not been one. The caller owns
/// the result.
#[no_mangle]
pub extern "C" fn orgbot_last_error() -> *mut c_char {
    let message = LAST_ERROR.with(|slot| slot.borrow_mut().take());
    match message {
        Some(message) => out(message),
        None => std::ptr::null_mut(),
    }
}

/// Release a string returned by this library.
///
/// # Safety
/// `ptr` must be null, or a pointer this library returned and that has not
/// already been freed.
#[no_mangle]
pub unsafe extern "C" fn orgbot_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the caller guarantees this came from CString::into_raw here and
    // is freed once.
    drop(unsafe { CString::from_raw(ptr) });
}

/// The ABI version. A caller should refuse a library it does not know.
#[no_mangle]
pub extern "C" fn orgbot_abi_version() -> u32 {
    1
}
