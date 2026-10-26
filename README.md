# OrgBots

[Product website](https://orgbots.theaecestudio.com/) ·
[Install on Ubuntu](docs/installation.md) ·
[Installer script](scripts/install-orgbots.sh) ·
[Verification and benchmark status](https://orgbots.theaecestudio.com/results/)

A group chat of AI bots that work a shared task board together. Bots hold
different roles, pick up tasks, **communicate**, **hand work off** to one
another, **wait** on those handoffs, get their work **inspected**, and finally
**submit** it — all in one room you can watch and talk to. Computer access depends on the platform and the workspace you select.

Runs on Linux and Android. Bots can be backed by Claude (Anthropic), DeepSeek,
or any OpenAI-compatible endpoint. Android supports in-app Codex/ChatGPT sign-in; a
single room can mix providers. A bot can also be a full agent session on this
machine — the Codex CLI, Claude Code, or Grok CLI — so it brings that CLI's own
sign-in, memories and skills into the room and works a real project folder.
Linux external MCP calls pass through the room's one-use owner approvals.

## Why it holds together

The hard part of a multi-bot room is not the models; it is the coordination.
The design puts every coordination rule in one tested place:

- **Task and handoff lifecycles are explicit state machines.** An illegal move
  — submitting work that was never inspected, two bots claiming one task — is a
  typed error at the boundary, not corrupt state discovered three steps later.
- **Separation of duties is enforced.** A bot can never inspect its own work,
  inspectors cannot submit, and a coordinator cannot sign off on its own plan.
- **Handoffs are first-class, so waiting is safe.** A blocking handoff parks the
  sender automatically; any settlement — completed, rejected, withdrawn, expired
  — wakes it. A refusal can never strand a bot.
- **Deadlocks are detected, not survived.** The engine keeps a wait-for graph
  and, when A waits on B who waits on A, breaks the cycle deterministically —
  preferring to withdraw the blocking handoff so the victim keeps its work.
- **Rework is bounded.** A worker and an inspector who never agree fail the task
  on a revision budget instead of looping until the room's turn budget is gone.

The engine records coordination events and serves snapshots with replay for reconnecting clients. Flutter saves each run under **Room options → Saved run history**. Linux room services retain checkpoints, agent memory and CLI session identity across restarts; paused rooms stay paused, and interrupted operations are not blindly replayed. The separate Tauri shell is not the supported Linux delivery target.

Linux 2.0 adds protected per-agent working copies with conflict checks, explicit
file-removal approval and HDD retention, opt-in sandboxed API commands, background
service notifications, and portable room/project backups restored paused.
Room usage shows input, output and cache tokens per agent and in total. API cost
estimates and subscription API equivalents stay separate; missing usage or prices
remain unknown. See [workspace boundaries](docs/WORKSPACE_SAFETY.md),
[acceptance coverage](scripts/LINUX_ACCEPTANCE.md) and the
[repeatable benchmark](scripts/benchmarks/README.md). Fixture benchmarks do not
establish real subscription savings.

## Layout

```
core/                     Rust workspace — the whole backend
  orgbot-proto           wire types + the task/handoff state machines
  orgbot-engine          the room: task board, handoff routing, deadlock detection
  orgbot-providers       model backends: Anthropic, DeepSeek, OpenAI-compatible, scripted
  orgbot-tools           the tool surface bots act through (chat, board, handoffs, review)
  orgbot-runtime         the bot turn loop and the scheduler
  orgbot-server          TOML rooms, a WebSocket host, and the `orgbot` CLI
  orgbot-ffi             Android in-process engine and workspace tool bridge
app/                      Flutter client for Linux + standalone Android
desktop/                  Separate Tauri desktop shell
scripts/                  Isolated SSH test fixture
```

## Run it

Build the CLI:

```bash
cd core && cargo build --release
```

Create a room, check it, and run it headless (live transcript in your terminal):

```bash
./target/release/orgbot init room.toml
./target/release/orgbot check room.toml
./target/release/orgbot run room.toml
```

Or host it for the app to attach to:

```bash
./target/release/orgbot serve room.toml --addr 127.0.0.1:8787
```

The host prints a generated room token, including on loopback. Enter it when joining `ws://127.0.0.1:8787/ws`. Flutter-created Linux rooms exchange their generated token automatically. Remote room connections require `wss://` and a TLS proxy; this is separate from Android SSH workspaces. Arbitrary website Origins are refused.

Run Flutter:

```bash
cd app && flutter run -d linux
```

### Android: standalone models, selectable computer

Android loads its bundled Rust library; it neither starts nor depends on a Linux CLI. Sign in to supported providers on the phone (Codex/Grok), or configure a DeepSeek API key. Claude CLI subscriptions remain a desktop capability. Bot keys, sign-ins and SSH credentials are stored in encrypted platform storage, with migration of old plaintext preferences after a successful secure write.

Grok subscriptions use the [Grok CLI catalog and chat proxy](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/README.md); API keys retain the public xAI API route. Access tokens renew before expiry, including while a native room is active. A rejected catalog request gets one renewal attempt; network and permission failures retain the saved sign-in. Mobile bots require a concrete model ID, with older empty selections repaired from the account's catalog.

Streaming model turns finish on the provider's completion event instead of waiting for the HTTP connection to close. Truncated streams cannot execute incomplete tool calls. Requests allow five minutes of network inactivity and thirty minutes overall; pausing the room cancels a request immediately.

Create or edit a room and tap **Working computer**:

- **This phone:** a separate persistent folder per room, with text-file list/read/write/directory tools. A custom absolute folder must already be accessible to the Android app. **Room options → Workspace files** lets you inspect/edit files and import/export UTF-8 documents with Android's document picker.
- **SSH / VPS:** enter host, port, username, existing absolute project directory and a password or OpenSSH/PEM private key (optional passphrase). For a key, use **Import key file** or **Paste key** to preserve its line breaks. The matching public key must be authorized for that server account. A VPS console password works for SSH only if the server permits SSH password login. Verify the server fingerprint independently, then use **Test connection and save**. Future connections require the saved key; a changed key fails closed. The remote machine needs SSH and SFTP, plus a POSIX shell if commands are enabled. It needs no OrgBot installation or model CLI.

Every bot in a room receives that computer's identity, root and available tools. File changes can be disabled. Shell commands require a separate switch and run with the SSH account's permissions; the working folder is **not a shell sandbox**. Use a restricted account when that is the intended access. File tools reject parent traversal and symlink escapes. Text files are limited to 1 MiB, listings to 500 entries, command output to 64 KiB, and commands to five minutes by default. Bots can set `timeout_seconds` from 1 to 1800 for longer builds or tests; a command timeout closes its channel while keeping SSH usable. Reads retry once after a connection loss; writes and commands are never automatically replayed. Existing files are replaced using a staged rename while retaining their basic permission bits; new files start private (0600). Servers without overwrite-rename support refuse replacement rather than truncate the original.

For an existing room, **Test connection and save** persists its computer settings immediately. A dropped SSH transport reconnects before the next tool call using the pinned server identity. The room shows connection status and a **Reconnect computer** action. No interrupted command or write is replayed automatically.

Set **Bots & rooms → Your profile** to choose the name and context sent to bots in new runs. Type **@** in the room composer to pick a bot from the list (arrow keys, then Enter or Tab), or type `@handle`. `@all` (also `@everyone`) asks every bot, and each one replies. A message with no tag goes to one bot, usually an available coordinator. Tags direct attention while the transcript remains visible to the whole room.

Pause interrupts the model turn and closes the workspace connection. Resume verifies/reconnects the selected computer. Failed writes and commands are never automatically retried: inspect their effects first. Active Android rooms use a foreground service while the app is in the background; force-stopping the app still stops execution. Saved rooms reopen paused with their history. The Work board shows blocking reasons and offers **Reply to unblock** for tasks waiting for your answer. Start failures remain visible across all room tabs.

Build an **arm64 development APK** and install without clearing app data:

```bash
# From repository root; Flutter, Rust, Android SDK and NDK must be installed.
rustup target add aarch64-linux-android
export PATH="$HOME/.cargo/bin:$PATH"
cd app
flutter pub get
flutter build apk --debug --target-platform android-arm64
adb install --user 0 --no-streaming -r build/app/outputs/flutter-apk/app-debug.apk
```

Gradle builds and copies the current Rust engine before packaging, and checks the required workspace ABI symbol. Set `ANDROID_NDK_HOME` if NDK discovery needs an override. Android requires API 24+. Release APK/AAB builds require your own `app/android/key.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`); production builds cannot silently use the debug signing key. Keep that file and the keystore private.

Linux still uses the local `orgbot` executable and installed model CLIs. Install the Flutter desktop prerequisites and `libsecret-1-dev` (Ubuntu/Debian) for secure storage. Build the engine and set `ORGBOT_ENGINE` when its path is not automatically found.

On Ubuntu 24.04 amd64, use the [installation guide](docs/installation.md) to verify the archive key, configure the signed APT source, and install the desktop package. This is a third-party repository and requires a one-time key/source setup on each computer. See the [APT distribution guide](docs/apt-distribution.md) for package and signing maintenance details.

### Credentials

Room files hold **no secrets**. Each bot resolves its credential at runtime:

| Backend | Resolved from |
|---|---|
| Anthropic | `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, or an `ant auth login` profile |
| DeepSeek | `DEEPSEEK_API_KEY` |
| Codex CLI bot | `codex login` sign-in on this machine |
| Claude Code bot | Claude Code sign-in on this machine |
| Grok CLI bot | Grok CLI sign-in on this machine |
| OpenAI / Codex | `OPENAI_API_KEY`, or a `codex login` sign-in at `~/.codex/auth.json` |

## A room file

```toml
[room]
name = "release-notes"
goal = "Write clear, accurate release notes for 1.3."
completion = "all_tasks_terminal"

[[bots]]
handle = "lead"
roles = ["coordinator"]
autonomy = "coordinator"
provider = { kind = "anthropic", model = "claude-opus-5" }

[[bots]]
handle = "writer-a"
roles = ["writer"]
provider = { kind = "anthropic", model = "claude-sonnet-5" }

[[bots]]
handle = "writer-b"
roles = ["writer"]
provider = { kind = "deepseek", model = "deepseek-chat" }

[[bots]]
handle = "qa"
roles = ["reviewer"]
autonomy = "inspector"
provider = { kind = "anthropic", model = "claude-opus-5" }
```

A room with a coordinator plans its own board; without one, seed the work with
`[[tasks]]` entries.

A bot that runs as a real CLI agent on this machine (in a project folder, under
whatever sandbox you grant it) looks like:

```toml
[[bots]]
handle = "coder"
roles = ["writer"]
provider = { kind = "cli", cli = "codex", workdir = "/path/to/project", policy = "workspace_write", mcp = true }
# policy: read_only | workspace_write | full_access
# mcp = true serves the room's board to the agent as MCP tools, so it can
# claim tasks, hand work off, and ask for review mid-episode.
```

Each turn the engine spawns `codex exec` (or `claude -p` / `grok -p`) in that
folder and resumes the same session afterwards, so the bot remembers its own
earlier work like a human agent would.

## Saved rooms and background work

Opening a room returns to its conversation. Leaving the screen keeps it running. Chat, tasks, artifacts and each bot’s conversation context are checkpointed locally after changes; reopening after a process restart restores the room paused. Resume explicitly to continue model and workspace operations. Older app journals migrate on first open without deleting the original logs. The Memory tab includes editable room notes and saved outputs; your profile is shared across rooms.

Android shows an ongoing notification while rooms are active, with a **Pause all** action. A foreground service and wake locks keep the engine and SSH connections running when you leave the app or turn off the display. Pausing the last room releases those locks. Android force-stop, reboot or process termination still stops execution; saved rooms remain available for explicit resume. Linux continues to use the system engine and model CLIs.

In **Accounts**, save a DeepSeek or Ollama Cloud API key once; new bots reuse their provider's key from encrypted storage. Create an Ollama key at https://ollama.com/settings/keys, then choose **Ollama Cloud** when adding a bot. Models load from the live cloud catalog, and requests go directly to Ollama over HTTPS; no local Ollama installation is needed. Model availability and usage remain subject to your provider account. A single existing DeepSeek per-bot key migrates automatically. Distinct legacy keys are preserved. In room settings, **Working computer → Browse folders** loads the phone or SSH folder contents before you select a working path; **Files** opens that saved folder.

## Tests

```bash
(cd core && cargo test --workspace --locked)
(cd app && flutter analyze && flutter test)
cargo check --locked --manifest-path desktop/src-tauri/Cargo.toml
```

The Rust suite includes whole-room runs driven by providers that react to the
real prompt and tool-result text, so a coordination bug — a lost id, a bot that
never gets woken to answer — fails a test rather than surfacing in production.

### Android device checks

`app/integration_test/workspace_device_test.dart` exercises the actual native FFI, phone file tools, file viewer and real SSH operations, including encrypted private-key authentication. Use the isolated fixture in a temporary Python environment with `asyncssh` and `bcrypt` installed:

```bash
python scripts/ssh-test-server.py
# Use its printed connection.json for host tests and device-defines.json for the phone.
adb reverse tcp:22222 tcp:22222
cd app
ORG_GRADLE_PROJECT_orgbotQa=true flutter test integration_test/workspace_device_test.dart \
  -d <device> --no-uninstall \
  --dart-define-from-file=<fixture-device-defines.json>
```

Integration-test targets are refused unless `orgbotQa=true`, protecting the normal app package. The QA property uses `com.orgbot.app.qa`, keeping device tests separate from the normal package. Verify the reverse mapping remains active during the test (ADB reconnection clears it). Use `--no-uninstall`: Flutter otherwise removes its test package on completion. These deterministic tests do not validate a real paid model account or your own VPS configuration.

If Wi-Fi ADB repeatedly loses the test debugger, `integration_test/smoke_main.dart` runs phone-file, native-engine/tag, room navigation/restart and pinned-SSH-reconnect checks without a debugger. During its `background window` stage, background the QA app and turn off the display; `heartbeat.json` records progress and a native message plus an SSH reconnect run during that window. Reopen the QA app after 34 seconds to capture the restored chat and Memory screen. Build it with the same QA property and fixture defines using `flutter build apk --profile --target-platform android-arm64 -t integration_test/smoke_main.dart`, then install the QA APK, restore `adb reverse tcp:22222 tcp:22222`, and launch `com.orgbot.app.qa/com.orgbot.app.MainActivity`. Results are in the QA app’s `files/workspaces/smoke-result.json`. This smoke check supplements the widget/integration suite.

## License

Original OrgBots work is licensed under Apache License 2.0. Copyright 2026 The
AECE Studio. Bundled third-party components retain their own license terms and
notices; for example, the Inter font's SIL Open Font License and attribution
remain in `app/assets/fonts/OFL.txt` and in the desktop bundle.
