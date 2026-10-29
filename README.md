# OrgBots

[Product website](https://orgbots.theaecestudio.com/) ·
[Install on Ubuntu](docs/installation.md) ·
[Installer script](scripts/install-orgbots.sh) ·
[Test results and usage split](#verified-results) ·
[Verification and benchmark status](https://orgbots.theaecestudio.com/results/)

A group chat of AI bots that work a shared task board together. Bots hold
different roles, pick up tasks, **communicate**, **hand work off** to one
another, **wait** on those handoffs, get their work **inspected**, and finally
**submit** it — all in one room you can watch and talk to. Computer access depends on the platform and the workspace you select.

On Linux, connect installed **Codex, Claude Code and Grok CLIs** using their
existing sign-ins, alongside **DeepSeek and Ollama Cloud API keys**. A room can
mix these providers. In-app Codex/ChatGPT and Grok sign-in belongs to Android.
The headless Rust engine also supports Anthropic API and custom
OpenAI-compatible configurations through TOML room files.

## Why I built OrgBots

I built OrgBots because I got tired of switching between AI apps and terminals
to finish one task. Using Claude, Codex and Grok on the same project meant
passing messages between them, asking each to read the other's work, and
watching them repeat investigations that someone had already done.

OrgBots puts those agents in one room around the project:

- **Shared handoffs:** specialists pass task briefs, findings and artifact
  references to each other instead of making me relay every update.
- **Visible usage:** input, output and cache usage are recorded when providers
  report them, with API estimates separated from subscription equivalents.
- **Protected changes:** agents work in separate project copies. Integration
  checks for conflicting edits and requires approval for project-file removals.

It is meant to reduce coordination work and unnecessary repetition. Whether it
saves total tokens or money is a question for the benchmarks, not a promise.

## Why OrgBots? Comparing workflows

A CLI is a good fit for direct work with one provider. Buzz is a broader shared
workspace for people and agents. OrgBots focuses on coordinating different
providers around a local Linux project.

| Area | CLI coding agents | [Buzz by Block](https://github.com/block/buzz) | OrgBots |
|---|---|---|---|
| Main workflow | Direct coding sessions; some CLIs also support subagents | Shared rooms, channels, Git activity and workflows for people and agents | Local project rooms with specialist roles, handoffs and review |
| Setup | Install and authenticate the CLI | Connect to a shared relay or deploy your own | Signed Ubuntu package plus your chosen CLI accounts or API keys; no relay account |
| Agent workspaces | Codex and Claude Code offer optional Git worktrees | Workspace behavior depends on the connected agent harness | Automatic per-agent project copies and three-way conflict checks |
| Review and recovery | Depends on CLI permissions and the workflow you configure | Agent identities, room membership and an auditable event log | Independent task review, explicit project-removal approval and recoverable originals |
| Usage reporting | Varies by CLI and provider | Depends on the provider and agent integration; not assessed here | Reported token/cache usage, API cost estimates and separate subscription equivalents |
| Provider setup | Generally configured within each CLI | Agent integrations include Claude Code, Codex and Goose | Codex, Claude and Grok CLIs; DeepSeek and Ollama Cloud API keys |

Comparison checked on 26 October 2026 against the
[Buzz README](https://github.com/block/buzz/blob/main/README.md),
[Buzz architecture](https://github.com/block/buzz/blob/main/ARCHITECTURE.md),
[Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference) and
[Codex documentation](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan).
This describes workflow differences, not a security ranking or a cost benchmark.

For community listings, use the [prepared sharing blurb](docs/SHARING.md).

## Install on Ubuntu

The desktop package targets **Ubuntu 24.04 amd64**. Version 2.1.0 keeps
several bots working side by side without idle slots, shows pictures that
Codex bots generate right in the chat, lets bots use your graphics card, and
gives them their own SSH key for servers you choose instead of yours. It also
changed the license; see [License](#license) for the current terms and the
[2.1.0 live room record](docs/benchmarks/2026-10-29-live-room-run.md).
Version 2.2.0 keeps servers out of reach until you allow a bot onto one for a
specific task, lets bots finish the turn they are on when you pause, lets a
coordinator propose retiring stale tasks for you to approve with one tap, keeps
handoffs open while the receiving bot is busy, retries a
bot that hit a temporary provider error on time, names the CLI to sign in to
when a sign-in expires, and moves to the PolyForm Shield License, which allows
use at work.
Set up the signed third-party APT repository once on each computer:

```bash
sudo apt update
sudo apt install curl gnupg ca-certificates coreutils
curl -fsSLo install-orgbots.sh \
  https://raw.githubusercontent.com/acememe7/Orgbot/main/scripts/install-orgbots.sh
cat install-orgbots.sh
bash install-orgbots.sh
```

The installer verifies the archive key fingerprint before configuring APT.
Review the package list at APT's confirmation prompt. Launch **OrgBots** from
the application menu or run `orgbots`. After setup, the same commands install
an available update while preserving existing app data:

```bash
sudo apt update
sudo apt install orgbots
```

See the [installation guide](docs/installation.md) for the key fingerprint and
workspace requirements, and the [APT distribution guide](docs/apt-distribution.md)
for archive maintenance. Installing OrgBots does not install or authenticate
the model CLIs.

| Platform | Delivery status |
|---|---|
| Ubuntu 24.04 amd64 | Signed APT package |
| Android arm64, API 24+ | Source build instructions below; Flutter/Android version 1.3.2+18 |
| Windows and macOS | Distribution deferred |
| Tauri shell (`desktop/`) | Separate development shell; not the supported Linux package |

## Using the Linux app

Connect providers in **Accounts**, create or edit bots in **Team**, then add
them to a room with a goal and a project folder.

| Provider | Connection |
|---|---|
| Codex | Installed `codex` CLI and its existing sign-in |
| Claude | Installed `claude` CLI and its existing sign-in |
| Grok | Installed `grok` CLI and its existing sign-in |
| DeepSeek | Saved API key |
| Ollama Cloud | Saved API key; cloud catalog, no local Ollama installation required |

Finding a CLI does not verify its account access; the CLI checks that when it
runs. API keys are kept in encrypted platform storage. Available models and
usage limits depend on each account.

Since 2.0.3, OrgBots includes OpenCode and Antigravity provider configuration. OpenCode
requires an explicit `provider/model` and the selected
provider's existing OpenCode API credential; OAuth reuse is unavailable.
Antigravity uses a separate private sign-in. A protected Linux server smoke
completed on `gemini-3.8-flash-medium` through the room MCP bridge. A separate
11-check context test passed exact Astra-to-Gemini nonce routing, same-session
recall after restart, and a distinct isolated room that returned `UNKNOWN`.
An authorized room-MCP file write and readback also passed while direct native
file tools remained denied. Provider billing classification is unknown. These
additions have been included since the 2.0.3 package. See the
[integration status and verification](docs/NATIVE_CLI_INTEGRATIONS.md)
and [Antigravity runtime evidence](docs/benchmarks/2026-10-27-antigravity-runtime.json).

The [development live-test report](docs/benchmarks/2026-10-27-subscription-free-validation.md)
records the Astra/DeepSeek coding test, separate provider usage, and the
OpenCode free-allowance result. The Antigravity runtime proof is recorded
separately and has been included since 2.0.3. These reports do not establish general subscription savings.

Rooms have a shared conversation, task board, artifacts and editable memory.
Reply to a message or use `@handle` to direct work to an agent; an untagged
message goes to one available bot, preferring a coordinator. `@all` wakes the
whole team. Agents can use mentions and task handoffs to request another
agent's work. Questions that need you offer answers and a custom reply;
background room services can notify you while the GUI is closed.

The desktop workspace includes files, Git inspection, attachments and previews.
Supported previews depend on the file format and installed media tools.
CLI sessions use private copies of supported profile settings and skills.
The source runtime discovers room skills and MCP capabilities on demand, with
task or one-use lifetimes and explicit unloading. See the
[capability lifecycle](docs/CAPABILITY_LIFECYCLE.md) for the API and native CLI
context boundaries.
Configure external MCP servers through the room: each external tool call needs
an **Allow once / Deny** answer for its exact arguments. Host MCP configurations
are not automatically inherited, and review-only agents cannot call external
MCP tools. See [workspace boundaries](docs/WORKSPACE_SAFETY.md).

### Specialist team

The repository's [specialist preset](app/tool/specialist-team.json) assigns the
following responsibilities. It is a reference for configuring **Team**; a fresh
library does not automatically create all ten bots. These are configured model
choices, not proof of access through every provider account.

| Agent | Configured model/service | Responsibility |
|---|---|---|
| Astra (`@astra`) | GPT-6 Astra / Codex | Code and security review; security/backend planning and coordination; review-only workspace |
| Julian (`@opus`) | Opus 5.5 / Claude | Design, architecture and product coordination; relevant reviews |
| Miles (`@grok`) | Grok 4.7 | Bounded support and handoffs |
| Finn (`@grok-quick`) | Grok 4.7 | Fast, simple implementation |
| Devon (`@deepseek-builder`) | DeepSeek Flash | Backend and data implementation |
| Rhea (`@ollama-deepseek`) | DeepSeek via Ollama Cloud | Research and analysis |
| Quinn (`@ollama-glm`) | GLM via Ollama Cloud | QA and reproduction |
| Sol (`@sol`) | GPT-6.1 Sol / Codex | Difficult implementation and integration, extra-high reasoning |
| Mira (`@mira`) | GPT-6 Luna / Codex | Bounded implementation and frontend subtasks |
| Ellis (`@ellis`) | Haiku 5.5 / Claude | Writing, editing and documentation |

Astra and Julian coordinate within their areas and can cross-check significant
findings. The preset avoids automatic review loops and duplicate nested Astra
reviews; bounded Luna delegation remains available.

### Shared context and testing

Each agent keeps its own provider session. OrgBots supplies a fresh room digest
with the current task board, relevant recent events, pending handoffs and review
work. Handoffs carry a brief, expected results and artifact IDs; agents read the
artifact contents when needed. Old board snapshots are replaced rather than
accumulated as if they were still current.

For example, Astra can record a security finding as an artifact and hand Julian
a focused architecture question with that artifact attached. Julian receives
the handoff and reads the relevant evidence. This does not automatically share
private model reasoning or produce a diff summary of every file, and agents
still inspect source when their task requires it.

Workers run focused checks and attach commands, results and unrun checks before
review. Quinn handles reproduction and broader QA when useful. The assigned
reviewer owns verification of the combined project, running checks where its
permissions allow or handing execution to Quinn and inspecting the results.
Read-only reviewers keep generated outputs outside project files. This is an
agent workflow, not an automatic pre-integration shell hook or a mechanically
enforced guarantee that tests passed.

### Usage report

Open a room, select **Work**, then find **What this has drawn**. It shows
input/output tokens, cache reads/writes and cost totals for the room, with
account and agent breakdowns. Missing usage or prices remain unknown.

API cost estimates and subscription API equivalents are separate. A subscription
equivalent is not an extra bill or a measurement of your remaining subscription
quota. Splitting work can spread consumption across services; planning, handoffs
and review can also add tokens. Real subscription savings have not been measured.
See the [verified results](#verified-results) below and the
[benchmark procedure](scripts/benchmarks/README.md).

## Verified results

### One bot versus a reviewed room, 29 October 2026

The same real task, public party queueing in a private battle royale game, ran
from the same commit with the same Codex model alone, and as the implementer in
a room with an independent reviewer and a coordinator. Ten hidden checks,
written beforehand, judged both.

- **One bot: 3 of 10.** Its own tests and the project's 822 passed, yet it
  shipped five defects, including parties that stayed queued after a member
  left and a web cancel that never reached the server.
- **Room: 10 of 10**, delivered after the reviewer sent the work back three
  times (5, 2 and 1 blocking findings) and then approved it.
- **Cost:** 2.7 times the output tokens (138,800 against 50,527) and about three
  times the time.

One task and one run of each. See the
[full record and its limits](docs/benchmarks/2026-10-29-one-bot-vs-room.md).

### 2.1.0 live room run, 28 to 29 October 2026

One real project room ran overnight with **10 bots on 5 providers**: Claude
Code, Codex, Grok CLI, the DeepSeek API and Ollama Cloud.

- **118 turns** and **8 tasks delivered** after an independent review by another
  bot, out of 15 created.
- **1,418,410 output tokens**, 7,593,215 uncached input and cache-write tokens
  and 190,907,064 cache-read tokens, as each provider reported them. These are
  not invoices or quota readings.
- **The final hour on the 2.1.0 package:** 34 turns, no claim released by
  mistake, and one turn failed on a temporary Codex capacity error before the
  bot recovered.
- **Account dashboards the owner observed:** the Codex weekly limit still 97%
  left; Ollama Pro credits used $0.37 for the month; DeepSeek about $0.25 for
  the run. Claude showed 6% of the weekly limit, mostly from a separate
  development session, and Grok 7% of a 5-hour window, as reported by the owner.
- **2.1.0 checks:** 907 Rust tests and 363 Flutter tests passed, plus the
  workspace sandbox and package tests.

One room on one night is not a controlled comparison. See the
[full record, per-bot table and limits](docs/benchmarks/2026-10-29-live-room-run.md).

### 2.0.2 release checks and comparison, 27 October 2026

Recorded **27 October 2026** for **OrgBots 2.0.2 on Ubuntu 24.04 amd64**:

- **854 Rust tests passed**, zero failed, three ignored. The native usage test
  was explicitly run separately; optional tests that exit without credentials
  are not live-provider evidence.
- **337 Flutter tests passed**, five skipped; Flutter analysis found no issues.
- **17/17 Linux acceptance checks passed**, including private D-Bus checks.
- **Lifecycle probe 6 passed all 16 checks**, including exact tool approval,
  pause/restart with a pending decision, one actual external call, and release
  of loaded skill and tool-schema content. The fixture stopped awaiting review.
- **Three real Codex episodes**, including resume and provider reconstruction,
  reconciled exactly with native records: 35,083 uncached input, 59,904 cached
  input and 250 output tokens. This separately verifies the accounting fix.

One baseline/team comparison completed the same two tasks in both runs. Both
passed the same **six original tests unchanged** and independent acceptance
review. The accepted trial recorded this usage:

| Run | Model | Uncached input | Cached input | Output |
| --- | --- | ---: | ---: | ---: |
| Baseline | GPT-6.1 Sol | 60,388 | 921,600 | 10,975 |
| Baseline | GPT-6 Astra | 61,933 | 1,107,840 | 6,749 |
| **Baseline** | **Codex total** | **122,321** | **2,029,440** | **17,724** |
| Team | GPT-6.1 Sol | 32,223 | 420,608 | 4,744 |
| Team | GPT-6 Astra | 56,932 | 909,824 | 9,257 |
| **Team** | **Codex total** | **89,155** | **1,330,432** | **14,001** |
| Team | DeepSeek Flash | 20,205 | 279,040 | 13,164 |

The comparison preceded the accounting fix. Codex figures above use each
session's final native total, correcting repeated cumulative snapshots in the
historical room report. DeepSeek figures use API usage records. Token categories
are disjoint; cache writes were zero.

Codex used a subscription; its token counts do not establish a dollar charge
or quota percentage. DeepSeek made **22 API requests**, with a separate
**$0.023542 estimated API cost**, not an invoice. Across all team models, totals
were **109,360 uncached input, 1,609,472 cached input and 27,165 output** tokens.
Codex usage was lower in this trial; combined output was higher after including
DeepSeek. One accepted pair does not establish general cost, quota or speed
savings.

Earlier failed trials and lifecycle probes remain in the report. Probe 7 on
the final dependency-patched binary timed out after restart, and its allowance
ledger could not reserve another request under the existing $0.25 ceiling.
A complete final-binary lifecycle rerun is not claimed.

Read the pinned [release verification record](https://github.com/acememe7/Orgbot/blob/b48566d3d6ef5c7ac7b5143cbb5e37723933378b/docs/RELEASE_2_0_2_VERIFICATION.md)
and [usage reconciliation, failed attempts and test limits](https://github.com/acememe7/Orgbot/blob/b48566d3d6ef5c7ac7b5143cbb5e37723933378b/docs/benchmarks/2026-10-27-release-validation.md),
or view the [website evidence page](https://orgbots.theaecestudio.com/results/).

## Why it holds together

The hard part of a multi-bot room is not the models; it is the coordination.
The design puts every coordination rule in one tested place:

- **Task and handoff lifecycles are explicit state machines.** An illegal move
  — submitting work that was never inspected, two bots claiming one task — is a
  typed error at the boundary, not corrupt state discovered three steps later.
- **Separation of duties is enforced.** A bot cannot inspect its own work.
  Review and final submission require their respective permissions; a task
  must pass review before final submission.
- **Handoffs are first-class, so waiting is safe.** A blocking handoff parks the
  sender automatically; any settlement — completed, rejected, withdrawn, expired
  — wakes it. A refusal can never strand a bot.
- **Deadlocks are detected, not survived.** The engine keeps a wait-for graph
  and, when A waits on B who waits on A, breaks the cycle deterministically —
  preferring to withdraw the blocking handoff so the victim keeps its work.
- **Rework is bounded.** A worker and an inspector who never agree fail the task
  on a revision budget instead of looping until the room's turn budget is gone.

The engine records coordination events and serves snapshots with replay for reconnecting clients. Flutter saves each run under **Room options → Saved run history**. Linux room services retain checkpoints, agent memory and CLI session identity across restarts; paused rooms stay paused, and interrupted operations are not blindly replayed.

Linux 2.0 adds protected per-agent working copies with conflict checks, explicit
file-removal approval and recoverable originals, opt-in sandboxed API commands, background
service notifications, and portable room/project backups restored paused.
New installations keep recovery copies in `~/.local/share/orgbots/trash`
(or `$XDG_DATA_HOME/orgbots/trash`). Choose another private folder under
**Connections & tools → File recovery**, including `/mnt/hdd/OrgBots Trash` on an HDD.
Existing HDD selections stay in place; unavailable selected storage stops
replacement/removal instead of falling back to another disk. Older package-only
installations ask you to select storage explicitly. A folder containing recovery
entries cannot be switched until those entries are resolved. The optional daily
cleanup timer keeps entries for seven full days before clearing eligible entries
on day eight or later. See the [recovery guide](app/linux/TRASH.md).
The sandbox protects workspace writes but still
allows network access and reading host files; remote tool effects are outside
the local recovery boundary. Portable backups are private, unencrypted and
limited to 64 MiB; histories can contain secrets you pasted. See
[workspace boundaries](docs/WORKSPACE_SAFETY.md) and
[acceptance coverage](scripts/LINUX_ACCEPTANCE.md).

## Layout

```
core/crates/              Rust workspace crates
  orgbot-proto           wire types + the task/handoff state machines
  orgbot-engine          the room: task board, handoff routing, deadlock detection
  orgbot-providers       native CLI agents, API backends and scripted fixtures
  orgbot-tools           the tool surface bots act through (chat, board, handoffs, review)
  orgbot-runtime         the bot turn loop and the scheduler
  orgbot-server          TOML rooms, a WebSocket host, and the `orgbot` CLI
  orgbot-ffi             Android in-process engine and workspace tool bridge
app/                      Flutter client for Linux + standalone Android
  packaging/              Debian package, protected workspaces and recovery helpers
  tool/                   specialist team preset and migration helpers
desktop/                  Separate Tauri development shell
scripts/                  installer, acceptance/SSH fixtures and benchmark harness
docs/                     installation, safety and verification records
website/                  product website source
deploy/website/           website delivery and recovery tools
```

## Build from source

Run these commands from the repository root. Install Rust and Flutter first.
Linux source builds also need `clang`, `cmake`, `ninja-build`, `pkg-config`,
`libgtk-3-dev`, `libsecret-1-dev`, `libwebkit2gtk-4.1-dev` and `libvte-2.91-dev`;
Debian packaging additionally needs `dpkg-dev`. APT installs runtime dependencies
for the published package.

Build the CLI:

```bash
(cd core && cargo build --release --locked)
```

Create a room configuration:

```bash
./core/target/release/orgbot init room.toml
```

Edit its models and paths, then configure credentials as described below.
Check the configuration without running a model:

```bash
./core/target/release/orgbot check room.toml
```

This resolves credentials and checks CLI discoverability, but does not verify
account access or workspace execution. Starting a room uses the configured
providers and their allowance:

```bash
./core/target/release/orgbot run room.toml
```

Or host it for the app to attach to:

```bash
./core/target/release/orgbot serve room.toml --addr 127.0.0.1:8787
```

The host prints a generated room token, including on loopback. Enter it when joining `ws://127.0.0.1:8787/ws`. Flutter-created Linux rooms exchange their generated token automatically. Remote room connections require `wss://` and a TLS proxy; this is separate from Android SSH workspaces. Arbitrary website Origins are refused.

Run Flutter:

```bash
(cd app && flutter pub get && flutter run -d linux)
```

Flutter locates the local engine automatically; set `ORGBOT_ENGINE` to the
absolute engine path if it is elsewhere. See [app development](app/README.md)
for release packaging.

### Android: standalone models, selectable computer

Android loads its bundled Rust library; it neither starts nor depends on a Linux CLI. Sign in to supported providers on the phone (Codex/Grok), or configure a DeepSeek or Ollama Cloud API key. Claude CLI subscriptions remain a desktop capability. Bot keys, sign-ins and SSH credentials are stored in encrypted platform storage, with migration of old plaintext preferences after a successful secure write.

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
(cd app && flutter pub get && flutter build apk --debug --target-platform android-arm64)
adb install --user 0 --no-streaming -r app/build/app/outputs/flutter-apk/app-debug.apk
```

Gradle builds and copies the current Rust engine before packaging, and checks the required workspace ABI symbol. Set `ANDROID_NDK_HOME` if NDK discovery needs an override. Android requires API 24+. Release APK/AAB builds require your own `app/android/key.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`); production builds cannot silently use the debug signing key. Keep that file and the keystore private.

## Headless room files

For TOML rooms, keep secrets out of the room file. API providers resolve
credentials at runtime. These settings are separate from the GUI's **Accounts**
screen and its Linux CLI connections:

| Backend | Resolved from |
|---|---|
| Anthropic API | `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` |
| DeepSeek | `DEEPSEEK_API_KEY` |
| Codex CLI bot | `codex login` sign-in on this machine |
| Claude Code bot | Claude Code sign-in on this machine |
| Grok CLI bot | Grok CLI sign-in on this machine |
| OpenAI-compatible API | The API key environment variable configured for that provider |

Custom OpenAI-compatible endpoints belong to the TOML engine configuration;
the GUI's Ollama option connects to Ollama Cloud.

### Example configuration

```toml
[room]
name = "release-notes"
goal = "Write clear, accurate release notes for 1.3."
completion = "all_tasks_terminal"
state_path = "/absolute/private/orgbots/release-notes.state.json"

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

Replace the example model IDs with models available to your account. Set
`state_path` inside a private, owner-controlled directory outside the project.
Linux CLI agents require this path for protected workspaces and saved native
sessions; GUI-created rooms configure it automatically. A room with a coordinator
plans its own board; without one, seed the work with `[[tasks]]` entries.

To add a CLI agent to that room, use an existing absolute project directory:

```toml
[[bots]]
handle = "coder"
roles = ["writer"]
provider = { kind = "cli", cli = "codex", workdir = "/path/to/project", policy = "workspace_write", mcp = true }
# policy: read_only | workspace_write | full_access
# mcp = true serves the room's board to the agent as MCP tools, so it can
# claim tasks, hand work off, and ask for review mid-episode.
```

From version 2.0.3, OpenCode uses `cli = "opencode"` with an explicit
`model = "provider/model"`. Antigravity's saved identifier is
`cli = "antigravity"` (`agy` is also accepted); its executable is `agy`, and
the explicit model slug `gemini-3.8-flash-medium` passed a guarded Linux room
MCP smoke, an 11-check context/restart test, and an authorized MCP file
write/readback with direct native file tools denied. Billing treatment remains
unknown; credential refresh and live external-MCP approvals remain unverified.
On Linux, `orgbot-engine agy login` opens the separate private sign-in in your terminal;
`orgbot-engine agy status` reports stored-credential presence only. Connections & tools
also exposes these actions in the Linux app. Neither new provider
uses the unprotected Dart runner or the Android engine.

Each turn starts a CLI process and resumes that bot's saved native session when
available. A new process does not mean a new conversation. Session reuse requires
matching provider, model, project path, sandbox policy and MCP mode. Missing or
rejected CLI history can require a fresh session. On Linux the CLI works in its
protected project copy, with conflict-checked integration back to the project.

## Saved rooms and background work

Opening a room returns to its conversation. Leaving the screen keeps it running.
Chat, tasks, artifacts and bot context are checkpointed locally after changes.
Linux room services can continue with the GUI closed and restore their saved
running/paused status; Android rooms reopen paused after process termination.
Older app journals migrate on first open without deleting the original logs.
The Memory tab includes editable room notes and saved outputs; your profile is
shared across rooms.

Rooms can continue through multiple turns, subject to configured room/task
budgets, provider limits and failures. Linux CLI turns have no time limit; they
end when the agent finishes or when you pause or stop the room. Set
`ORGBOT_CLI_TURN_TIMEOUT_SECS` to impose one. Persistent sessions do
not remove per-call limits. Elapsed multi-day operation and physical
suspend/reboot recovery remain unverified; the
[release record](docs/LINUX_2_0_PLAN.md) distinguishes those gaps from the
scripted crash/restart and long-workload checks.

Android shows an ongoing notification while rooms are active, with a **Pause all** action. A foreground service and wake locks keep the engine and SSH connections running when you leave the app or turn off the display. Pausing the last room releases those locks. Android force-stop, reboot or process termination still stops execution; saved rooms remain available for explicit resume. Linux continues to use the system engine and model CLIs.

In **Accounts**, save a DeepSeek or Ollama Cloud API key once; new bots reuse their provider's key from encrypted storage. Create an Ollama key at https://ollama.com/settings/keys, then choose **Ollama Cloud** when adding a bot. Models load from the live cloud catalog, and requests go directly to Ollama over HTTPS; no local Ollama installation is needed. Model availability and usage remain subject to your provider account. A single existing DeepSeek per-bot key migrates automatically. Distinct legacy keys are preserved. In room settings, **Working computer → Browse folders** loads the phone or SSH folder contents before you select a working path; **Files** opens that saved folder.

## Tests

Run from the repository root:

```bash
(cd core && cargo test --workspace --locked)
(cd app && flutter analyze && flutter test)
cargo check --locked --manifest-path desktop/src-tauri/Cargo.toml
```

The Rust suite includes whole-room runs driven by providers that react to the
real prompt and tool-result text, so a coordination bug — a lost id, a bot that
never gets woken to answer — fails a test rather than surfacing in production.

### Android device checks

`app/integration_test/workspace_device_test.dart` exercises the actual native FFI, phone file tools, file viewer and real SSH operations, including encrypted private-key authentication. Use the isolated fixture in a temporary Python environment with `asyncssh` and `bcrypt` installed. Start it in one terminal from the repository root:

```bash
python3 scripts/ssh-test-server.py
```

In a second terminal, also from the repository root, substitute your device ID
and the absolute `device-defines.json` path printed by the fixture:

```bash
adb reverse tcp:22222 tcp:22222
(cd app && ORG_GRADLE_PROJECT_orgbotQa=true flutter test integration_test/workspace_device_test.dart \
  -d 'DEVICE_ID' --no-uninstall \
  --dart-define-from-file='/absolute/path/to/device-defines.json')
```

Integration-test targets are refused unless `orgbotQa=true`, protecting the normal app package. The QA property uses `com.orgbot.app.qa`, keeping device tests separate from the normal package. Verify the reverse mapping remains active during the test (ADB reconnection clears it). Use `--no-uninstall`: Flutter otherwise removes its test package on completion. These deterministic tests do not validate a real paid model account or your own VPS configuration.

If Wi-Fi ADB repeatedly loses the test debugger, `integration_test/smoke_main.dart` runs phone-file, native-engine/tag, room navigation/restart and pinned-SSH-reconnect checks without a debugger. During its `background window` stage, background the QA app and turn off the display; `heartbeat.json` records progress and a native message plus an SSH reconnect run during that window. Reopen the QA app after 34 seconds to capture the restored chat and Memory screen. Build it with the same QA property and fixture defines using `flutter build apk --profile --target-platform android-arm64 -t integration_test/smoke_main.dart`, then install the QA APK, restore `adb reverse tcp:22222 tcp:22222`, and launch `com.orgbot.app.qa/com.orgbot.app.MainActivity`. Results are in the QA app’s `files/workspaces/smoke-result.json`. This smoke check supplements the widget/integration suite.

## Provider terms

OrgBots runs each provider's own CLI under your own sign-in and never calls a
provider with a token it extracted. Whether a subscription may be used this way
is up to each provider, and their terms change:

- **Claude Code** documents `claude -p` on Pro and Max plans, but Anthropic's
  terms restrict third-party tools that store or route subscription
  credentials. Check the current terms before relying on a subscription.
- **Codex CLI** documents non-interactive `codex exec` with a ChatGPT sign-in.
- **Grok Build** documents headless use for SuperGrok and X Premium subscribers.
- **Antigravity**: Google's FAQ and terms say a personal Google account must
  not be used through third-party apps, and accounts have been suspended for
  it. Do not use a personal Antigravity sign-in in OrgBots.
- API providers such as DeepSeek and Ollama Cloud use API keys and bill per use.

Several bots on one account share that account's limits.

## License

OrgBots 2.2.0 and later releases are licensed under the
[PolyForm Shield License 1.0.0](LICENSE). Copyright 2026 The AECE Studio.

- **Free to use, including at work.** Individuals and companies may use,
  change and share OrgBots for any purpose, including commercial software
  development.
- **No competing products.** You may not offer a product that competes with
  OrgBots, whether sold or free: for example, bundling OrgBots into a product
  you sell, reselling or white-labelling it, hosting it as a service, or
  distributing a fork as a substitute. A separate commercial license for these
  uses is available from The AECE Studio: theaece@theaecestudio.com.
- **No warranty.** The software is provided as is, without any warranty, and
  the licensor is not liable for any damages.

OrgBots 2.1.0 was published under the PolyForm Noncommercial License 1.0.0, and
releases 2.0.3 and earlier under the Apache License 2.0; copies of
those releases remain available under the license they were published with.
Bundled third-party components keep their own license terms and notices; for
example, the Inter font's SIL Open Font License and attribution remain in
`app/assets/fonts/OFL.txt` and in the desktop bundle. This summary is not legal
advice; the license text governs.
