//! OrgBot desktop shell: a bot library and group-chat rooms over the local
//! engine. Rooms are persisted as JSON under the app data dir; starting one
//! writes a room definition the embedded gptbots host runs, with agent-CLI
//! bots working the room's project path.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::Manager;

const HOST_ADDR: &str = "127.0.0.1:8787";

// ---------------------------------------------------------------- models

/// A bot the user created, reusable in any room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotDef {
    pub id: String,
    pub name: String,
    pub handle: String,
    pub persona: String,
    /// "codex" | "claude" | "grok" | "api" | "scripted"
    pub provider: String,
    pub model: String,
}

/// A bot placed in a room, with the role it plays there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomBotSlot {
    pub bot_id: String,
    pub roles: Vec<String>,
}

/// A room: a group chat of chosen bots working one project path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDef {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub path: String,
    pub tasks: Vec<String>,
    pub bots: Vec<RoomBotSlot>,
}

fn new_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}_{nanos:x}")
}

// ---------------------------------------------------------------- storage

struct Store {
    bots: Vec<BotDef>,
    rooms: Vec<RoomDef>,
    base: PathBuf,
}

impl Store {
    fn load(base: PathBuf) -> Self {
        let read = |name: &str| -> Vec<BotDef> {
            std::fs::read(base.join(name))
                .ok()
                .and_then(|raw| serde_json::from_slice(&raw).ok())
                .unwrap_or_default()
        };
        Store {
            bots: read("bots.json"),
            rooms: std::fs::read(base.join("rooms.json"))
                .ok()
                .and_then(|raw| serde_json::from_slice(&raw).ok())
                .unwrap_or_default(),
            base,
        }
    }

    fn persist(&self) {
        let _ = std::fs::create_dir_all(&self.base);
        if let Ok(json) = serde_json::to_string_pretty(&self.bots) {
            let _ = std::fs::write(self.base.join("bots.json"), json);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.rooms) {
            let _ = std::fs::write(self.base.join("rooms.json"), json);
        }
    }
}

struct AppState {
    store: Mutex<Store>,
    host: Mutex<HostState>,
}

#[derive(Default)]
struct HostState {
    config: Option<PathBuf>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
}

#[derive(Serialize, Clone)]
struct HostInfo {
    running: bool,
    config: Option<String>,
    ws_url: String,
}

#[derive(Serialize, Clone)]
struct BotView {
    id: String,
    name: String,
    handle: String,
    persona: String,
    provider: String,
    model: String,
    provider_label: String,
}

fn bot_view(b: &BotDef) -> BotView {
    let label = match b.provider.as_str() {
        "codex" => "Codex CLI",
        "claude" => "Claude CLI",
        "grok" => "Grok CLI",
        "api" => "Anthropic API",
        _ => "Scripted demo",
    };
    BotView {
        id: b.id.clone(),
        name: b.name.clone(),
        handle: b.handle.clone(),
        persona: b.persona.clone(),
        provider: b.provider.clone(),
        model: b.model.clone(),
        provider_label: label.to_string(),
    }
}

fn clean_handle(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect()
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn bots_list(app: tauri::AppHandle) -> Vec<BotView> {
    let managed = app.state::<AppState>();
    let store = managed.store.lock().unwrap();
    store.bots.iter().map(bot_view).collect()
}

#[tauri::command]
fn bot_save(
    app: tauri::AppHandle,
    id: Option<String>,
    name: String,
    handle: Option<String>,
    persona: String,
    provider: String,
    model: String,
) -> Result<BotView, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("give the bot a name".into());
    }
    let id = id.unwrap_or_else(|| new_id("bot"));
    let handle_raw = handle.unwrap_or_default().trim().to_string();
    let handle = {
        let clean = clean_handle(&handle_raw);
        if clean.is_empty() { name.clone() } else { clean }
    };
    let bot = BotDef {
        id: id.clone(),
        name,
        handle,
        persona: persona.trim().to_string(),
        provider: provider.trim().to_string(),
        model: model.trim().to_string(),
    };
    let managed = app.state::<AppState>();
    let mut store = managed.store.lock().map_err(|_| "store locked".to_string())?;
    if let Some(existing) = store.bots.iter_mut().find(|b| b.id == id) {
        *existing = bot.clone();
    } else {
        store.bots.push(bot.clone());
    }
    store.persist();
    Ok(bot_view(&bot))
}

#[tauri::command]
fn bot_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let managed = app.state::<AppState>();
    let mut store = managed.store.lock().map_err(|_| "store locked".to_string())?;
    store.bots.retain(|b| b.id != id);
    store.persist();
    Ok(())
}

#[derive(Serialize, Clone)]
struct RoomView {
    id: String,
    name: String,
    goal: String,
    path: String,
    bot_count: usize,
    task_count: usize,
}

fn room_view(r: &RoomDef) -> RoomView {
    RoomView {
        id: r.id.clone(),
        name: r.name.clone(),
        goal: r.goal.clone(),
        path: r.path.clone(),
        bot_count: r.bots.len(),
        task_count: r.tasks.len(),
    }
}

#[tauri::command]
fn rooms_list(app: tauri::AppHandle) -> Vec<RoomView> {
    let managed = app.state::<AppState>();
    let store = managed.store.lock().unwrap();
    store.rooms.iter().map(room_view).collect()
}

#[tauri::command]
fn room_save(
    app: tauri::AppHandle,
    id: Option<String>,
    name: String,
    goal: String,
    path: String,
    tasks: Vec<String>,
    bots: Vec<RoomBotSlot>,
) -> Result<RoomView, String> {
    if name.trim().is_empty() {
        return Err("give the room a name".into());
    }
    let id = id.unwrap_or_else(|| new_id("room"));
    let room = RoomDef {
        id: id.clone(),
        name: name.trim().to_string(),
        goal: goal.trim().to_string(),
        path: path.trim().to_string(),
        tasks: tasks
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        bots,
    };
    let managed = app.state::<AppState>();
    let mut store = managed.store.lock().map_err(|_| "store locked".to_string())?;
    if let Some(existing) = store.rooms.iter_mut().find(|r| r.id == id) {
        *existing = room.clone();
    } else {
        store.rooms.push(room.clone());
    }
    store.persist();
    Ok(room_view(&room))
}

#[tauri::command]
fn room_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let managed = app.state::<AppState>();
    let mut store = managed.store.lock().map_err(|_| "store locked".to_string())?;
    store.rooms.retain(|r| r.id != id);
    store.persist();
    Ok(())
}

#[tauri::command]
fn room_start(app: tauri::AppHandle, id: String) -> Result<HostInfo, String> {
    let config_path = {
        let managed = app.state::<AppState>();
        let store = managed.store.lock().map_err(|_| "store locked".to_string())?;
        let room = store
            .rooms
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| "room not found".to_string())?;
        let toml = room_toml(room, &store.bots)?;
        let path = std::env::temp_dir().join(format!("orgbot-{}.toml", room.id));
        std::fs::write(&path, toml).map_err(|e| format!("cannot write room file: {e}"))?;
        path
    };
    start_host(app, config_path)
}

/// Host the given room file, waiting for real readiness (never hangs).
fn start_host(app: tauri::AppHandle, config: PathBuf) -> Result<HostInfo, String> {
    let addr: SocketAddr = HOST_ADDR.parse().expect("static host addr parses");
    let managed = app.state::<AppState>();
    {
        let state = managed.host.lock().map_err(|_| "state lock poisoned".to_string())?;
        if state.config.is_some() {
            return Ok(HostInfo {
                running: true,
                config: Some(config.display().to_string()),
                ws_url: format!("ws://{HOST_ADDR}/ws"),
            });
        }
    }
    let shown = config.display().to_string();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<SocketAddr, String>>();
    let owned = config.clone();
    let task = tauri::async_runtime::spawn(async move {
        if let Err(error) =
            gptbots_server::serve_room_with_readiness(&owned, addr, Some(ready_tx)).await
        {
            eprintln!("room host failed: {error}");
        }
    });
    let bound = tauri::async_runtime::block_on(async {
        let wait = tokio::time::timeout(std::time::Duration::from_secs(15), ready_rx).await;
        match wait {
            Ok(Ok(Ok(addr))) => Ok(addr),
            Ok(Ok(Err(reason))) => Err(reason),
            Ok(Err(_)) => Err("room host stopped before it was ready".into()),
            Err(_) => Err("room host did not become ready within 15s".into()),
        }
    })?;
    {
        let mut state = managed.host.lock().map_err(|_| "state lock poisoned".to_string())?;
        state.config = Some(config);
        state.task = Some(task);
    }
    Ok(HostInfo {
        running: true,
        config: Some(shown),
        ws_url: format!("ws://{bound}/ws"),
    })
}

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

// -------------------------------------------------------- room generation

/// Render a room as the TOML the embedded gptbots host runs. Agent-CLI bots
/// work the room's project path; scripted bots are deterministic stand-ins.
fn room_toml(room: &RoomDef, bots: &[BotDef]) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("[room]\n");
    out.push_str(&format!("name = \"{}\"\n", esc(&room.name)));
    out.push_str(&format!("goal = \"{}\"\n", esc(&room.goal)));
    out.push_str("completion = \"all_tasks_terminal\"\n\n");

    for slot in &room.bots {
        let Some(bot) = bots.iter().find(|b| b.id == slot.bot_id) else {
            continue;
        };
        let handle = if bot.handle.is_empty() { &bot.name } else { &bot.handle };
        out.push_str("[[bots]]\n");
        out.push_str(&format!("handle = \"{}\"\n", esc(handle)));
        out.push_str(&format!("display_name = \"{}\"\n", esc(&bot.name)));
        let roles = slot
            .roles
            .iter()
            .map(|r| format!("\"{}\"", esc(r)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("roles = [{roles}]\n"));
        if !bot.persona.trim().is_empty() {
            out.push_str(&format!("persona = \"{}\"\n", esc(&bot.persona)));
        }
        let autonomy = if slot.roles.iter().any(|r| r == "coordinator") {
            "\"coordinator\""
        } else if slot.roles.iter().any(|r| r == "reviewer") {
            "\"inspector\""
        } else {
            "\"worker\""
        };
        out.push_str(&format!("autonomy = {autonomy}\n"));
        match bot.provider.as_str() {
            "codex" | "claude" | "grok" => {
                out.push_str(&format!("provider = {{ kind = \"cli\", cli = \"{}\"", bot.provider));
                if !room.path.trim().is_empty() {
                    out.push_str(&format!(", workdir = \"{}\"", esc(&room.path)));
                }
                if !bot.model.trim().is_empty() {
                    out.push_str(&format!(", model = \"{}\"", esc(&bot.model)));
                }
                out.push_str(", policy = \"workspace_write\", mcp = true }\n");
            }
            "api" => {
                let model = if bot.model.trim().is_empty() {
                    "claude-sonnet-5"
                } else {
                    &bot.model
                };
                out.push_str(&format!(
                    "provider = {{ kind = \"anthropic\", model = \"{}\" }}\n",
                    esc(model)
                ));
            }
            _ => out.push_str("provider = { kind = \"scripted\", script = \"Working on it.\" }\n"),
        }
        out.push('\n');
    }

    for (i, task) in room.tasks.iter().enumerate() {
        out.push_str("[[tasks]]\n");
        out.push_str(&format!("title = \"task {}\"\n", i + 1));
        out.push_str(&format!("brief = \"{}\"\n", esc(task)));
        out.push_str(&format!("required_role = \"writer\"\n\n"));
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

// ------------------------------------------------------------------- main

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let base = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("orgbot"));
            std::fs::create_dir_all(&base).ok();
            app.manage(AppState {
                store: Mutex::new(Store::load(base)),
                host: Mutex::new(HostState::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bots_list,
            bot_save,
            bot_delete,
            rooms_list,
            room_save,
            room_delete,
            room_start,
            host_stop,
            host_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running the OrgBot desktop shell");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_toml_renders_a_room_with_cli_agents_on_the_path() {
        let room = RoomDef {
            id: "r1".into(),
            name: "ship notes".into(),
            goal: "write the release notes".into(),
            path: "/home/me/proj".into(),
            tasks: vec!["cover every change".into()],
            bots: vec![RoomBotSlot {
                bot_id: "b1".into(),
                roles: vec!["coordinator".into()],
            }],
        };
        let bots = vec![BotDef {
            id: "b1".into(),
            name: "Lead".into(),
            handle: "lead".into(),
            persona: "plan the work".into(),
            provider: "codex".into(),
            model: "".into(),
        }];
        let toml = room_toml(&room, &bots).unwrap();
        assert!(toml.contains("handle = \"lead\""), "{toml}");
        assert!(toml.contains("cli = \"codex\""), "{toml}");
        assert!(toml.contains("workdir = \"/home/me/proj\""), "{toml}");
        assert!(toml.contains("autonomy = \"coordinator\""), "{toml}");
        assert!(toml.contains("mcp = true"), "{toml}");
        assert!(toml.contains("cover every change"), "{toml}");
    }

    #[test]
    fn store_round_trips_bots_and_rooms() {
        let dir = std::env::temp_dir().join(format!("orgbot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::load(dir.clone());
        store.bots.push(BotDef {
            id: "b1".into(),
            name: "Writer".into(),
            handle: "writer".into(),
            persona: "write".into(),
            provider: "claude".into(),
            model: "claude-sonnet-5".into(),
        });
        store.persist();
        let reloaded = Store::load(dir.clone());
        assert_eq!(reloaded.bots.len(), 1);
        assert_eq!(reloaded.bots[0].name, "Writer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
