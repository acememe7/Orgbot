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
