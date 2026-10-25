# Linux specialist workspace validation — 2026-10-25

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
