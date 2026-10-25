# OrgBots Linux desktop 1.8.0 — 2026-10-25

- [x] Short desktop transitions honor reduced motion. Hidden pages stop their
  animation tickers while retaining drafts and state. Linux uses the generated
  desktop/window/package icon; Android keeps its existing logo.
- [x] Collapsible activity shows public working status and tool start/end
  outcomes, capped at 12 recent steps per bot. Parallel agents keep independent
  status. Hidden reasoning and tool arguments/output are not displayed.
  History restore of 500 messages produces one store update; replay of 500
  events produces one batched update; 100 text deltas produce zero preview
  updates. Final messages still arrive. Activity changes batch at 16 ms.
- [x] Agents can ask the owner a durable question with 2–6 choices and custom
  input, with or without a claimed task. Answers reach only the asking agents;
  repeated answers acknowledge current state without waking unrelated agents.
  These questions never grant authorization. Pending questions prevent empty
  planning turns and premature room completion. Lost-answer retry, disposal,
  taskless mobile filtering and duplicate controls have regression coverage.
- [x] Linux notification routing opens the saved room and focuses its question.
  The native hook presents the existing window. GUI-closed/background-only
  notification delivery is not implemented. The installed Linux plugin's real
  D-Bus Notify and ActionInvoked path passed against a synthetic notification
  service, preserving room/event payload and the default open action. This is
  protocol/callback proof, not a GNOME visual-toast or click-to-focus check.
  Reproduce: `dbus-run-session -- env ORGBOTS_PRIVATE_DBUS_TEST=1 flutter test
  --no-pub test/attention_dbus_smoke_test.dart` from `app/`. The test is skipped
  by default; log: `/tmp/orgbots-1.8-notification-dbus-final.log`.
- [x] Standalone `orgbot-trash` and daily user timer installed. Actual hardened
  service: Result=success, ExecMainStatus=0; timer enabled and active. Sixteen
  isolated tests cover seven-full-day retention, restore, mount checks and
  unsafe-entry rejection. No user files were moved or purged. See [TRASH.md](TRASH.md).
- [x] Rust workspace: 793 passed, zero failed, one ignored. After review fixes,
  the affected server suite passed 97 tests (one ignored), followed by the
  final duplicate-answer acknowledgement regression. Release/debug engines built.
  A pre-existing fallback fixture depended on random agent-ID ordering; its
  setup now leaves only its intended planner eligible.
- [x] Final Flutter: 285 passed, three external SSH checks skipped; analyzer
  clean. Python preset: six passed; trash: 16 passed; packaging fixture passed.
  Logs: `/tmp/orgbots-1.8-flutter-final.log`, `orgbots-1.8-analyze-final.log`,
  `orgbots-1.8-rust-full.log`, `orgbots-1.8-server-final.log`,
  `orgbots-1.8-answer-idempotent.log` (all under `/tmp/`).
- [x] X11/Nvidia scripted profile: ten agents, 500 messages, 250 ms activity
  bursts; 550 frames over 30 seconds. UI build p50 0.651 ms, p95 4.307 ms,
  max 8.504 ms; raster p50 3.271 ms, p95 16.788 ms, max 66.781 ms.
  These are measured frame durations, not a 60 FPS guarantee. Wide and 800 px
  desktop views are readable; 375 px desktop mode remains cramped.
  Evidence: `/tmp/orgbots-activity-parallel-final-20261025/`.
- [x] Final release/package built. All 28 extracted bundle files, engine and
  icon match their sources; six native ELF dependencies resolve. Actual package
  launched with an isolated synthetic keyring/old-roster fixture. Keys, room
  memberships, history and tasks survived; Astra/Julian coordinator roles are
  visible. Normal window close and readback passed. Evidence:
  `/tmp/orgbots-1.8-package-qa/`, `/tmp/orgbots-1.8-artifact.json`.
- [x] Credential-pattern scan: zero findings across 54 owned staged files;
  staged diff checks pass. Unrelated README, lockfile and Tauri work is excluded.
- [x] Final scoped Astra source, integration, notification-fixture, visual,
  artifact and verification-claims review approved. All required findings are
  resolved; the external acceptance limits below remain open.

Package: `app/orgbot_1.8.0_amd64.deb`, 15,299,964 bytes.
SHA-256: `004d4726ef5b646a7d1084ddc8c5b35471c61bb61f9213d1e1cc1902246ffa41`.
Engine SHA-256: `689045cb75fd6f784ef2a277e809338dc4239b2739ae141f6aa920816f9c38e9`.

No paid-provider calls, GUI package install, real-library mutation or push were
performed for 1.8.0. The separately authorized cleanup script/timer is installed.
Restart existing room services deliberately to adopt the new engine; installing
an app package does not replace already-running service processes.

---

# OrgBots coordination and continuity 1.7.1 — 2026-10-25

- [x] Astra and Julian (Opus 5.5) coordinate and create role-specific tasks;
  Miles remains support. Other specialist assignments and personalities remain.
  Astra retains read-only workspace access and cannot claim implementation work.
  Coordination permissions do not bypass independent review before delivery.
- [x] Untagged top-level requests address one eligible coordinator, respecting
  retry cooldowns and shared allowances. Tags, replies and explicit `@all`
  retain their routing. An answered request reserves the initial planning turn;
  task-creating workers do not preempt an available coordinator.
- [x] CLI session IDs survive failed/interrupted first episodes in memory and
  on disk. Role briefs refresh on the same native session; failed attempts do
  not mark a new brief as applied. Retrieved memory stays in the current
  situation instead of repeatedly changing the standing instructions.
- [x] One-time Linux library migration validates and securely saves before
  adopting changes. Managed identities, keys, rooms and unrelated specialist
  settings are preserved; subsequent owner edits are not reapplied over.
- [x] Rust workspace: 782 passed, zero failed, one ignored. Python preset
  tests: six passed. Flutter: 270 passed, three external SSH checks
  skipped; analyzer clean. Widget fixtures run asynchronous asset reads outside
  fake time. Logs: `/tmp/orgbots-1.7.1-rust-tests.log`,
  `/tmp/orgbots-1.7.1-flutter-final.log`, `/tmp/orgbots-1.7.1-analyze.log`.
- [x] Release app/engine and packaging fixtures passed. Extracted bundle and
  engine match their builds; native libraries resolve. Actual extracted app
  migrated an isolated old-roster/keyring fixture, preserving test keys,
  memberships, tasks and history. Wide Team view confirms coordinator labels;
  normal Alt+F4 close is clean. Evidence: `/tmp/orgbots-final-qa/`.
- [x] Staged credential-pattern scan found no credentials in the 19 owned
  files; diff checks pass. Unrelated README, lockfile and Tauri changes remain
  outside this update.
- [x] Final scoped Astra source, integration, migration, visual, artifact and
  verification-claims review approved; no required corrections remain.

Package: `app/orgbot_1.7.1_amd64.deb`, 14,087,212 bytes.
SHA-256: `f1f6746af1b0388bc901f965d794caf5bd74dbdfbffa5a4235fe0bd6b0fe413b`.
Engine SHA-256: `2ec9076ef274d62e7a98fa4540e0b7ae506a900df5dc1f85b733c6e4913accdb`.

No real inference, system install, live-library mutation or push was performed
for this update. Install and reopen the app to apply the roster migration.
Already-running room services retain their old configuration until explicitly
stopped and restarted. Provider-live resume acceptance remains the 1.7.0
evidence below; the new failure/retry checks use fake CLIs.

---

# OrgBots desktop workspace 1.7.0 — 2026-10-25

## Delivered and checked

- [x] Linux room supervision, Secret Service credentials, checkpoint recovery,
  UI detach/reconnect and explicit stop; paused work stays paused after restart.
- [x] Durable provider allowances across rooms, API text-token/cost reservations
  with configured prices, CLI launch caps, reset windows and cooldown routing.
- [x] Project memory with offline concept retrieval, bounded history summaries,
  automatic completed-work capture and owner search/hide/delete/share controls.
- [x] Managed MCP settings/authentication/tests, skill folder/ZIP management and
  supported CLI plugin operations, retaining unsupported-policy boundaries.
- [x] Native terminal and text editor, Git branch/commit-staged/merge, embedded
  browser, native audio/video controls and timestamped video attachment samples.
- [x] Browser state retained through resize/collapse/dialogs; stale media errors
  suppressed; no native surfaces covering Flutter dialogs. Background media
  writes are cancellable. Failed editor rollback preserves its recovery copy.

## Verification and artifact

- [x] Flutter: 264 passed, three external SSH checks skipped; analyzer clean.
  Rust workspace: 775 passed, zero failed, one ignored. Release engine built.
- [x] Native editor recovery, overlay bounds, browser navigation/lifecycle and
  media metadata/CSP regression checks passed. The media harness does not claim
  trusted control interaction; actual app Play/Pause/seek was checked separately.
- [x] Real X11/Nvidia desktop checks at 1440×920, 800×920 and 375×920: terminal,
  save-to-disk editor, Git branch, spending control, project memory, scripted
  replies/reconnect, browser interaction, MP4 Play/Pause/seek and WAV Play/Pause.
  Clean profile playback passed after diagnostic logging was removed.
- [x] Measured profile sample: 115 frames from 32 inspector switches; UI p95
  8.440 ms, raster-thread p95 16.581 ms. Four raster samples exceeded 16.67 ms.
  This small-room trace is not a GPU hardware timing or general FPS claim.
- [x] Isolated actual systemd lifecycle test and 600.1-second recovery/allowance
  fixture passed: ten cycles, 12 fake CLI launches, 12 ledger entries, checkpoint
  persistence and paused-state preservation. No real inference in this fixture.
- [x] Saved DeepSeek key verified with a bounded request (48 input, 2 output
  tokens). Codex reply/resume and Grok reply passed. Claude was authenticated but
  at its weekly quota; Ollama was not called because the owner is unsubscribed.
- [x] Linux release and packaging fixtures passed. All 26 extracted Flutter
  bundle files and the bundled engine match the build; all six native ELF files
  resolve their dependencies. Package extracted and launched in isolated app
  data/keyring; no system install, real-library migration or push performed.
- [x] Staged credential-pattern scan: zero findings across 74 owned files;
  staged diff checks pass. Unrelated README, lockfile and Tauri work is preserved.
- [x] Final scoped Astra source, integration, visual, artifact and claims
  review approved; all required corrections resolved. Package hash independently
  verified by the reviewer. Approval does not include system install or push.

Package: `app/orgbot_1.7.0_amd64.deb`, Ubuntu 24.04/amd64, glibc >=2.39.
Size: 14,081,632 bytes.
SHA-256: `4440888b84c904c82487861f6dad59aa94dd172ee8ccfd25180deee3dbbc2930`.
Engine SHA-256: `a6d3f41ff0a501f0873fa3c0cbd721c34f24cd2c997323cdf3996d1319604fbd`.
Artifact manifest: `/tmp/orgbots-linux-1.7.0-artifact.json`.
Detailed evidence: [DESKTOP_COMPLETION.md](DESKTOP_COMPLETION.md).

## External acceptance and limits

Multi-day, host sleep/reboot, Wayland, physical Android and comprehensive
screen-reader acceptance remain unverified. Provider subscription quotas and
opaque CLI internal token/cost usage cannot be enforced as exact account caps.
Tool compatibility and codecs remain provider/format specific. Active agents
consume allowance; dividing work does not guarantee lower total tokens.
See [REMAINING.md](REMAINING.md) for the full implementation/acceptance inventory.

---

# OrgBots desktop refinement 1.6.1 — 2026-10-25

## Delivered and checked

- [x] Compact desktop spacing, neutral light/dark surfaces, flat conversation
  rows, readable text and action contrast, restrained controls and page headers.
  Desktop changes use explicit desktop paths; Android keeps its existing UI.
- [x] Collapsible session sidebar (also Ctrl+B), tools rail, project preview,
  MCP and skills lists, and routine system notices. Error notices stay visible.
  Tools start collapsed; the opened inspector is resizable.
- [x] Retain drafts when switching pages, and selected previews when closing
  tools or resizing between desktop and narrow layouts. Stale asynchronous Git
  responses cannot overwrite a newer project/view selection.
- [x] Reuse unchanged transcript views, suppress irrelevant whole-library
  notifications, and lazily build integration lists. These remove redundant
  work; no measured frame-rate or CPU improvement is claimed.
- [x] No user library migration, credential changes or live inference in this
  pass. CLI and API access remain available; Android OAuth is unchanged.

## Verification and artifact

- [x] Final Flutter suite: 243 passed, three external SSH checks skipped.
  Analysis: no issues. Focused tests include retained navigation/drafts,
  collapsed tools, 375/800px layouts, 1.6x text scaling, light/dark contrast,
  transcript keys/scrolling, Git response ordering and notification filtering.
- [x] Native release inspected in isolated Xvfb/D-Bus/data/keyring at 1440×920,
  800×920 and 375×920. Checked light/dark screens, Team, Connections, MCP/skill
  expansion, text preview and retention across window breakpoints.
  Xvfb uses software rendering and does not establish desktop GPU performance.
- [x] Linux release and packaging fixtures pass. All 26 extracted bundle files
  match the build and all six native binaries/libraries resolve dependencies.
  Rust engine is unchanged from the validated 1.6.0 release below.
- [x] Final scoped Astra source/visual/artifact/inventory review approved. All
  required corrections resolved. Staged credential-pattern scan: zero findings
  across 18 owned files; diff checks pass.

Package: `app/orgbot_1.6.1_amd64.deb`, Ubuntu 24.04/amd64, glibc >=2.39.
Size: 13,718,414 bytes.
SHA-256: `f849743cb04ee3523191ee30c1f6685b4224773525daaed3be7de1e92e701ecb`.
Engine SHA-256: `92164b0d5435a9bef807fd5c1868fa98f4cc76825938e278d56f96b03a7bc782`.
Temporary evidence: `/tmp/orgbots-polish-*.log`,
`/tmp/orgbots-polish-artifact.json` and `/tmp/orgbots-polish-shots/`.

## Remaining work and acceptance

See [REMAINING.md](REMAINING.md) for the prioritized gaps. This pass does not
establish live paid-model collaboration, multi-day endurance, Wayland/GPU frame
timing, comprehensive screen-reader acceptance or Android device acceptance.
Active agents consume provider allowance; distributing tasks does not guarantee
fewer total tokens. No paid provider requests were made during validation.
System installation remains for the owner.

---

# OrgBots desktop sessions 1.6.0 — 2026-10-25

This section supersedes the 1.5.2 behavior and roster recorded below.

## Delivered and checked

- [x] Persistent desktop session sidebar, conversation and resizable project/work/
  team/memory inspector; compact windows switch the inspector to a tab.
- [x] Reply context persists through WebSocket, FFI and room checkpoints. Replies
  target the original agent unless explicit mentions select another recipient.
  Bots can address agents by handle or unambiguous display name. Explicit Pause
  parks incoming work until Resume.
- [x] Native multi-file/folder selection, retained originals, text/image previews,
  short-video thumbnails and system Open With; read-only Git status/diff/history.
  Attachments accept any file format within five files/20 MiB each. Internal
  decoding depends on available codecs; video thumbnails require FFmpeg.
- [x] Native CLIs retain installed MCP/skills/plugins. Compatible stdio and
  Streamable HTTP MCP configuration and local skill folders are discovered for
  API bots. Unsupported policy filters, legacy SSE and plugin-only hooks are not
  imported as unrestricted tools. The UI distinguishes configured from connected.
  Local discovery found seven configured MCP entries, one unsupported entry and
  208 skill folders; those counts do not establish live connectivity.
- [x] CLI conversation IDs persist across engine restarts, bound to CLI/model/
  project and policy. Session files are private. MCP credentials stay in the
  process environment, not generated room TOML or the discovery UI.
- [x] Desktop defaults and both saved rooms use no total turn/time cutoff and
  four concurrent turns. Idle rooms make no inference requests. Worker episodes
  retain 24 model/tool rounds, reviewers eight; individual CLI invocations retain
  the existing 600-second timeout. Failures, provider limits, Pause and host sleep
  still interrupt work. This is not a background service or a spending cap.
- [x] Sol uses extra-high reasoning. OrgBots room instructions retain Luna
  delegation and assign reviews to its team, without a duplicate mandatory
  nested Astra review. Global Codex instructions were not changed.

## Saved team and data

| Name | Model | Assignment |
| --- | --- | --- |
| Astra | `gpt-6-astra` | Code review and security; review-only |
| Julian | `claude-opus-5-5` | Architecture, design, product decisions and selective review |
| Miles | `grok-4.7` | Coordination, support and delivery |
| Finn | `grok-4.7` | Fast changes and general implementation |
| Devon | `deepseek-flash` | Backend, data and implementation |
| Rhea | `deepseek-v4.1-flash:1017` via Ollama | Research and analysis |
| Quinn | `glm-5.3-flash` via Ollama | QA and reproduction |
| Sol | `gpt-6.1-sol` | Difficult implementation, integration and operations |
| Mira | `gpt-6-luna` | Bounded implementation and frontend subtasks |
| Ellis | `claude-haiku-5-5` | Writing, editing and documentation |

- [x] Ten named bots cover all 18 catalog roles. Stable identities/handles keep
  old conversations valid. Personas are candid and professional, without claims
  to be human. Review cross-checks are selective, not automatic ping-pong.
- [x] Replaced the actual seven-bot library with ten bots in both existing rooms.
  Encrypted backup label: `OrgBot team backup 20261025T132116Z`. Backup/readback,
  idempotence, profile, room settings and credential preservation verified.
  All 87 room-history files retain their exact SHA-256 fingerprints.
- [x] DeepSeek/Ollama API fields remain available; no keys were saved on this PC.
  Enter them in Connections & tools. Haiku 5.5 was found in the installed Claude
  CLI catalog; live model authorization and inference were not exercised.

## Verification and artifact

- [x] Rust workspace: 733 passed, one ignored. After the final MCP negotiation
  correction, its 14 focused tests passed again and the release engine rebuilt.
- [x] Final Flutter suite against that engine: 231 passed, three external SSH
  checks skipped. Flutter analysis: no issues. Python migration: six passed.
- [x] Linux native release checked in isolated Xvfb/D-Bus/data/keyring at
  1440×920, 1280×720 and 800×600. Verified restored Pause, targeted reply with
  attachment, response after Resume, project text/image/video previews, Git,
  team and connection pages. Fixed a GTK picker ownership crash and a thumbnail
  seek past the end of short clips found during this native pass.
- [x] Packaging fixtures passed. Every extracted bundle file matches the build;
  all six native binaries/libraries resolve their runtime dependencies.
- [x] Final scoped Astra review approved source, packaged artifact, migration
  receipts and native captures. All required corrections are resolved. Staged
  credential-pattern scan: zero findings across 52 owned files; diff checks pass.

Package: `app/orgbot_1.6.0_amd64.deb`, Ubuntu 24.04/amd64, glibc >=2.39.
Size: 13,740,336 bytes.
SHA-256: `36667833725d592cce6216e866bd8ab15e8b4cfb3245fb766395dab693a1e854`.
Engine SHA-256: `92164b0d5435a9bef807fd5c1868fa98f4cc76825938e278d56f96b03a7bc782`.
Temporary evidence: `/tmp/orgbots-desktop-qa/` and `/tmp/orgbots-*.log`.

## Current acceptance limits

No live paid-model collaboration, all-provider tool compatibility, video playback,
multi-day endurance, Wayland/GPU performance or Android device check was performed.
Native CLI integrations retain their own permissions; read-only room tools are
not OS isolation from inherited integrations. Legacy SSE and unsupported MCP
policy formats remain reported as unsupported. The app and PC must remain running;
reopening a saved room requires Resume. Active work consumes subscription/API
allowance, and dividing work does not guarantee fewer total tokens. No paid model
requests were made during validation. System installation remains for the owner.

---

# Historical: Linux specialist workspace 1.5.2 — 2026-10-25

## Delivered behavior

- [x] Linux supports Codex, Claude Code and Grok CLI sessions alongside saved
  DeepSeek and Ollama Cloud API keys. CLI labels report installation without
  claiming verified sign-in. Android retains its OAuth flow.
- [x] Desktop tabs use a short fade, wide team lists use compact rows, memory
  artifacts render lazily, and unchanged stream deltas no longer rebuild the
  room. Tests cover retry/completion updates, retained search and narrow layouts.
- [x] Fresh Linux rooms require Start, including reopened failed/closed rooms.
  Saved rooms restore paused. Desktop defaults are 100 scheduled turns and one
  concurrent bot; Android defaults remain unchanged. Desktop API turns allow six
  model/tool rounds, or four for reviewers. These are not token or monetary caps;
  CLI episodes retain their separate timeout and internal tool loop.
- [x] Review-only bots cannot claim ordinary implementation tasks or mutate
  workspace files. They can inspect work or submit an assigned review report
  for another bot's approval. Self-approval is rejected. API review-only bots
  cannot call unclassified external MCP tools.
- [x] Review-only CLI configuration forces a read-only filesystem policy. Codex
  explicitly approves the guarded room MCP server and uses approval_policy=never
  for fresh and resumed sessions, avoiding separate automatic-approval reviews.
  Spawned CLIs do not inherit the room-control token or DeepSeek/Ollama API keys.
- [x] API bots can list/read/write/mkdir in a selected Linux project folder.
  Descriptor-relative operations reject symlinks/path escapes and FIFO reads,
  bound listings/file sizes, preserve executable permissions, and write atomically
  off the async executor. API tools cannot run shell commands; Grok handles builds.

## Saved library

- [x] Replaced the actual ten-bot roster with seven specialists and updated both
  existing rooms to the new team and 100-turn/one-bot limits.
- [x] Preserved profile, room identities, goals, tasks, project settings and existing
  memory. Routing instructions are appended once. History files were not deleted.
  Restore tests verify departed bots release claims without losing event history.
- [x] Paused the active room and closed app/engines before maintenance. Its saved
  checkpoint had used one turn; the other room had no checkpoint.
- [x] Saved and read back the full original library in the encrypted keyring before
  replacement. Backup label: `OrgBot team backup 20261025T120957Z`. Re-read the new
  library and verified preservation and idempotence. Other secure entries match.
- [x] No DeepSeek/Ollama key was present. Those bots correctly show a missing-key
  state; keys must be entered in Setup.

| Bot | Model | Assignment |
| --- | --- | --- |
| Astra | `gpt-6-astra` | Code/security review; read-only files |
| Opus | `claude-opus-5-5` | Design, architecture and relevant reviews |
| Grok | `grok-4.7` | Small task plans, routing and delivery |
| Grok Quick | `grok-4.7` | Small changes and build/test commands |
| DeepSeek Builder | `deepseek-flash` | General implementation and fixes |
| Ollama DeepSeek | `deepseek-v4.1-flash:1017` | Research synthesis and documentation |
| Ollama GLM | `glm-5.3-flash` | Test design, QA and integration checks |

The preset calls for selective Astra/Opus cross-checks of material concerns,
without automatic review cycles. This is a role policy, not a measured model
ranking or automatic subscription-quota balancer. Preset and pure migration
helper: `app/tool/`.

Model identifiers were checked against the local Codex model cache, `grok models`,
[Anthropic](https://www.anthropic.com/claude/opus),
[DeepSeek](https://api-docs.deepseek.com/api/list-models/), and
[Ollama](https://ollama.com/api/tags). Ollama here is Cloud and consumes allowance;
see [pricing](https://ollama.com/pricing).

## Verification

- [x] Full Flutter suite against the release engine: 220 passed, three external
  SSH checks skipped. Analysis passed. Flutter source was unchanged afterward.
- [x] Four Python migration tests, packaging fixtures and shell syntax passed.
- [x] Extracted native UI checked in isolated Xvfb, D-Bus, data and keyring at
  1280×800 and 800×600. Team, Setup and Start gate rendered correctly; opening
  the fresh room did not launch an engine. No live inference was tested.
- [x] Final full Rust workspace suite: 713 passed, 1 ignored.
- [x] Final extracted package: all six native files resolve libraries and match
  the release build outputs.
- [x] Mandatory final Astra review approved the source, migration, native captures
  and packaged artifact; all actionable findings were resolved.

Package: `app/orgbot_1.5.2_amd64.deb`, Ubuntu 24.04/amd64, glibc >=2.39.
Package size: 13,635,454 bytes.
SHA-256: `aef240395044a85847bfccfbdd70d9844e7f5e03c695d4ce498a6c9cf017b5ff`.
Session evidence: `/tmp/orgbot-efficiency-oOSTaL/` (temporary).

## Acceptance limits

Real authenticated collaboration, API validity and actual model consumption were
not exercised. Native CLI integrations still follow their inherited configuration;
filesystem policy and guarded room tools are not OS isolation or a blanket block
on mutations through inherited remote integrations. No Wayland/GPU performance,
external SSH or Android device acceptance was performed; Xvfb uses software
rendering, so no hardware frame-rate improvement is claimed.

System installation remains for the owner; installed 1.5.1 was not upgraded.
No Android APK was built. Whole-workspace Rust formatting differs from the
pre-existing baseline; scoped diff checks pass. The old 1.5.1 artifact is retained,
SHA-256 `b149e377f0f7d6ec42b56a12267ea9056d0891721cdd28ea8f15b9d4841e9515`.
