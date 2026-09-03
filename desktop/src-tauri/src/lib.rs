//! The GPTBots desktop shell.
//!
//! The backend embeds gptbots-server directly: a room is hosted on
//! 127.0.0.1 inside this process and the webview attaches to its WebSocket
//! endpoint, exactly like the Flutter client attaches to a remote host.
//! Agent-CLI bots run their real CLIs from here, so the machine's memories,
//! skills and MCP servers are available to the room.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;


/// Where the room host listens. The webview connects to ws://{addr}/ws.
const HOST_ADDR: &str = "127.0.0.1:8787";

/// What the shell is currently hosting.
#[derive(Debug)]
struct HostState {
    /// The room file being hosted, if any.
    config: Option<PathBuf>,
    /// The serving task.
    task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            config: None,
            task: None,
        }
    }
}

/// App-wide state behind tauri's managed map.
#[derive(Default)]
struct AppState {
    host: Mutex<HostState>,
}

/// The address the shell hosts rooms on.
#[derive(Serialize, Clone)]
struct HostInfo {
    /// Whether a room is running.
    running: bool,
    /// The room file, when running.
    config: Option<String>,
    /// The WebSocket URL for the UI to attach to.
    ws_url: String,
}

/// Start hosting the room in `config` on the local loopback.
#[tauri::command]
fn host_start(app: tauri::AppHandle, config: String) -> Result<HostInfo, String> {
    let path = PathBuf::from(&config);
    let addr: SocketAddr = HOST_ADDR.parse().expect("static host addr parses");
    let managed = app.state::<AppState>();
    let mut state = managed.host.lock().map_err(|_| "state lock poisoned".to_string())?;
    if state.config.is_some() {
        // Already hosting a room; starting again is a no-op for the same file
        // and a replace otherwise.
        return Ok(HostInfo {
            running: true,
            config: Some(config),
            ws_url: format!("ws://{HOST_ADDR}/ws"),
        });
    }

    let owned = path.clone();
    let task = tauri::async_runtime::spawn(async move {
        if let Err(error) = gptbots_server::serve_room(&owned, addr).await {
            eprintln!("room host failed: {error}");
        }
    });
    state.config = Some(path);
    state.task = Some(task);
    Ok(HostInfo {
        running: true,
        config: Some(config),
        ws_url: format!("ws://{HOST_ADDR}/ws"),
    })
}

/// Stop hosting the room, if any.
#[tauri::command]
fn host_stop(app: tauri::AppHandle) -> Result<HostInfo, String> {
    let managed = app.state::<AppState>();
    let mut state = managed.host.lock().map_err(|_| "state lock poisoned".to_string())?;
    if let Some(task) = state.task.take() {
        task.abort();
    }
    state.config = None;
    Ok(HostInfo {
        running: false,
        config: None,
        ws_url: format!("ws://{HOST_ADDR}/ws"),
    })
}

/// What the shell is hosting right now.
#[tauri::command]
fn host_status(app: tauri::AppHandle) -> HostInfo {
    let managed = app.state::<AppState>();
    let state = managed.host.lock().unwrap();
    HostInfo {
        running: state.config.is_some(),
        config: state.config.as_ref().map(|p| p.display().to_string()),
        ws_url: format!("ws://{HOST_ADDR}/ws"),
    }
}

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![host_start, host_stop, host_status])
        .run(tauri::generate_context!())
        .expect("error while running the GPTBots desktop shell");
}
