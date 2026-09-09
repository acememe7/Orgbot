//! The C surface, exercised the way the phone app will use it.
//!
//! These run a real room in-process — the same engine the desktop uses — which
//! is the point of this crate: a standalone app on a device that cannot spawn
//! a subprocess.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use orgbot_ffi::*;

/// A room needing no credentials, so the test needs none either.
const ROOM: &str = r#"
[room]
name = "phone"
goal = "answer the person"
completion = "manual"

[[bots]]
handle = "helper"
roles = ["writer"]
provider = { kind = "scripted", script = "I hear you." }
"#;

fn c(text: &str) -> CString {
    CString::new(text).expect("no interior NUL")
}

/// Take an owned string back from the library.
fn take(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the library returned this, and we free it exactly once.
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { orgbot_string_free(ptr) };
    Some(text)
}

/// Collect events until everything in `wanted` has been seen, or give up.
///
/// Accumulating matters: a poll returns a *batch*, so checking one batch and
/// returning would throw away everything else it contained — which is how the
/// bot's reply went missing the first time this was written.
fn collect_until(handle: u64, wanted: &[&str]) -> String {
    let mut seen = String::new();
    for _ in 0..100 {
        if let Some(json) = take(orgbot_room_poll(handle)) {
            seen.push_str(&json);
        }
        if wanted.iter().all(|needle| seen.contains(needle)) {
            return seen;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    seen
}

#[test]
fn the_abi_is_versioned() {
    assert_eq!(orgbot_abi_version(), 2);
}

#[test]
fn a_room_runs_in_process_and_answers_you() {
    let toml = c(ROOM);
    let env = c("");
    // SAFETY: both are valid NUL-terminated strings for this call.
    let handle = unsafe { orgbot_room_open(toml.as_ptr(), env.as_ptr()) };
    assert_ne!(
        handle,
        0,
        "the room should open; error was {:?}",
        take(orgbot_last_error())
    );

    // A UI can render from the snapshot before any event arrives.
    let snapshot = take(orgbot_room_snapshot(handle)).expect("a snapshot");
    assert!(snapshot.contains("helper"), "snapshot was {snapshot}");

    let author = c("you");
    let body = c("are you there?");
    // SAFETY: valid strings, live handle.
    let sent = unsafe { orgbot_room_say(handle, author.as_ptr(), body.as_ptr()) };
    assert_eq!(sent, 1, "the message should be accepted");

    let seen = collect_until(handle, &["are you there?", "I hear you"]);
    assert!(
        seen.contains("are you there?"),
        "what the person said must come back on the event stream; saw: {seen}"
    );
    assert!(
        seen.contains("I hear you"),
        "and a bot must answer it; saw: {seen}"
    );

    let before: serde_json::Value =
        serde_json::from_str(&take(orgbot_room_snapshot(handle)).unwrap()).unwrap();
    let renewal = c(
        r#"{"type":"update_credentials","environment":{"GROK_AUTH_TOKEN":"private-renewed-test-token"}}"#,
    );
    assert_eq!(unsafe { orgbot_room_control(handle, renewal.as_ptr()) }, 1);
    let after: serde_json::Value =
        serde_json::from_str(&take(orgbot_room_snapshot(handle)).unwrap()).unwrap();
    assert_eq!(before["room"]["id"], after["room"]["id"]);
    assert_eq!(before["roster"], after["roster"]);
    assert!(!take(orgbot_room_poll(handle))
        .unwrap()
        .contains("private-renewed-test-token"));

    orgbot_room_close(handle);
}

#[test]
fn a_bad_room_file_reports_why_instead_of_crashing() {
    let toml = c("this is not TOML at all {{{");
    let env = c("");
    // SAFETY: valid strings.
    let handle = unsafe { orgbot_room_open(toml.as_ptr(), env.as_ptr()) };
    assert_eq!(handle, 0);
    assert!(
        take(orgbot_last_error()).is_some_and(|e| !e.is_empty()),
        "the caller needs to know what was wrong"
    );
}

#[test]
fn an_unknown_handle_is_an_error_not_a_crash() {
    assert!(take(orgbot_room_poll(999_999)).is_none());
    assert!(take(orgbot_room_snapshot(999_999)).is_none());
    assert_eq!(orgbot_room_pause(999_999), 0);
    // Closing something never opened is harmless.
    orgbot_room_close(999_999);
}

#[test]
fn null_arguments_are_refused_rather_than_dereferenced() {
    // SAFETY: passing null is exactly what this checks is handled.
    let handle = unsafe { orgbot_room_open(std::ptr::null(), std::ptr::null()) };
    assert_eq!(handle, 0);
    assert!(take(orgbot_last_error()).is_some());
}

#[test]
fn freeing_null_is_harmless() {
    // SAFETY: null is explicitly allowed.
    unsafe { orgbot_string_free(std::ptr::null_mut()) };
}

/// Credentials arrive out of band so they never sit in a room file.
#[test]
fn room_credentials_do_not_mutate_the_process_environment() {
    let toml = c(ROOM);
    let env = c("ORGBOT_FFI_TEST_TOKEN=abc123\n");
    // SAFETY: valid strings.
    let handle = unsafe { orgbot_room_open(toml.as_ptr(), env.as_ptr()) };
    assert_ne!(handle, 0);
    assert_eq!(
        std::env::var("ORGBOT_FFI_TEST_TOKEN").ok().as_deref(),
        None,
        "one room must not leak credentials to another through global environment"
    );
    orgbot_room_close(handle);
}

#[test]
fn native_json_controls_pause_resume_and_resync() {
    let handle = unsafe { orgbot_room_open(c(ROOM).as_ptr(), c("").as_ptr()) };
    assert_ne!(handle, 0);
    assert_eq!(
        unsafe { orgbot_room_control(handle, c(r#"{"type":"pause","reason":"test"}"#).as_ptr()) },
        1
    );
    let seen = collect_until(handle, &[r#""phase":"paused""#]);
    assert!(seen.contains(r#""phase":"paused""#), "{seen}");
    assert!(!seen.contains(r#""event":"room_closed""#));
    assert_eq!(
        unsafe { orgbot_room_control(handle, c(r#"{"type":"resume"}"#).as_ptr()) },
        1
    );
    let seen = collect_until(handle, &[r#""phase":"running""#]);
    assert!(seen.contains(r#""phase":"running""#), "{seen}");
    assert_eq!(
        unsafe { orgbot_room_control(handle, c(r#"{"type":"resync"}"#).as_ptr()) },
        1
    );
    let seen = collect_until(handle, &[r#""type":"snapshot""#]);
    assert!(seen.contains(r#""type":"snapshot""#), "{seen}");
    orgbot_room_close(handle);
}
