# OrgBot

A group chat of AI bots that work a shared task board together. Bots hold
different roles, pick up tasks, **communicate**, **hand work off** to one
another, **wait** on those handoffs, get their work **inspected**, and finally
**submit** it — all in one room you can watch and talk to. No shell, no computer
use: a bot's entire capability is chat, the board, handoffs, and artifacts.

Runs on Linux and Android. Bots can be backed by Claude (Anthropic), DeepSeek,
or any OpenAI-compatible endpoint — including a Codex/ChatGPT sign-in — and a
single room can mix providers. A bot can also be a full agent session on this
machine — the Codex CLI, Claude Code, or Grok CLI — so it brings that CLI's own
sign-in, memories, skills, and MCP servers into the room and works a real
project folder.

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

Everything is event-sourced: the room is a fold over an append-only log, so a
client can attach mid-run and catch up, and a run is reproducible after the fact.

## Layout

```
core/                     Rust workspace — the whole backend
  gptbots-proto           wire types + the task/handoff state machines
  gptbots-engine          the room: task board, handoff routing, deadlock detection
  gptbots-providers       model backends: Anthropic, DeepSeek, OpenAI-compatible, scripted
  gptbots-tools           the tool surface bots act through (chat, board, handoffs, review)
  gptbots-runtime         the bot turn loop and the scheduler
  gptbots-server          TOML rooms, a WebSocket host, and the `gptbots` CLI
app/                      Flutter client for Linux + Android
```

## Run it

Build the CLI:

```bash
cd core && cargo build --release
```

Create a room, check it, and run it headless (live transcript in your terminal):

```bash
./target/release/gptbots init room.toml
./target/release/gptbots check room.toml
./target/release/gptbots run room.toml
```

Or host it for the app to attach to:

```bash
./target/release/gptbots serve room.toml --addr 127.0.0.1:8787
```

Then run the Flutter app and connect to `ws://127.0.0.1:8787/ws`:

```bash
cd app && flutter run -d linux      # or: flutter run -d <android-device>
```

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

## Tests

```bash
cd core && cargo test --workspace     # engine, providers, tools, runtime, server
cd app  && flutter test               # protocol folding + widget rendering
```

The Rust suite includes whole-room runs driven by providers that react to the
real prompt and tool-result text, so a coordination bug — a lost id, a bot that
never gets woken to answer — fails a test rather than surfacing in production.
