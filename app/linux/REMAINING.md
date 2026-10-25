# Linux desktop: remaining work after the 1.6.1 UI refinement

The desktop conversation, saved sessions, reply targeting, project inspector,
work board, Git inspection, attachments and local CLI setup are implemented.
The desktop acceptance results and known limits are recorded in
[VALIDATION.md](VALIDATION.md). The remaining items below are product or
acceptance gaps, not claims that the desktop UI is unfinished.

## Missing capabilities

1. **Linux background supervision and unattended recovery.** A room restores
from its checkpoint when reopened, but Linux has no daemon or service manager
that keeps it running after OrgBots closes or restarts it. The Android
foreground service is platform-specific. For unattended Linux work, OrgBots
and the host must stay running; host sleep, app exit and reboot interrupt work.
See `app/lib/state/background_rooms.dart` and
`app/lib/engine/core_server.dart`.

2. **Hard provider-allowance or monetary limits.** Rooms can have total-turn and
elapsed-time limits; Linux defaults both to no cutoff. Token usage is collected
and shown in the Work overview where providers report it, but OrgBots has no
reliable provider pricing/quota feed or hard account, daily, weekly, or dollar
cap. A turn limit is not a token limit: there is no guaranteed token cut.
Provider allowance is consumed by active agents. See
`app/lib/orgbot/room_file.dart`, `core/crates/orgbot-proto/src/spend.rs`, and
`app/lib/ui/widgets/objectives_pane.dart`.

3. **Automatic quota-aware model balancing.** The scheduler supports room-wide
parallelism and an optional per-backend concurrency cap. It does not estimate
remaining subscription allowance or automatically allocate turns across
models/accounts. Current behavior and limits are summarized in
`app/linux/VALIDATION.md`. Adaptive rate-limit cooldowns and automatic recovery
from exhausted provider quotas are also unfinished; bounded retries and failure
states already exist.

4. **General semantic memory across rooms.** Bots can search archived room work
and read prior artifacts, and each room has owner-pinned notes. Search is an
explicit lexical query, not semantic/vector retrieval or an automatically
maintained shared knowledge base. Within a long bot conversation, old turns
are trimmed rather than summarized. See `core/crates/orgbot-engine/src/archive.rs`,
`core/crates/orgbot-tools/src/room.rs`, and
`core/crates/orgbot-runtime/src/bot.rs`.

5. **Broader plugin and controller interoperability.** Desktop can discover
supported local MCP servers and skills for rooms. Unsupported MCP formats and
plugin-only hooks are not imported; native Codex, Claude and Grok CLIs retain
their own installed permissions and tools. OrgBots does not provide a general
plugin installer/editor or an OS-level controller for every tool surface.
See `app/lib/orgbot/desktop_integrations.dart` and
`app/linux/VALIDATION.md`.

6. **A native local terminal and general human file editor.** Linux room tools
support bounded project file operations and read-only Git status/diff/log; the
local workspace rejects `workspace_run`. Shell execution is available for a
configured SSH workspace, while native CLIs may use their own tool permissions.
The desktop project pane is for browsing and preview, not general file editing.
See `core/crates/orgbot-server/src/local_workspace.rs` and
`core/crates/orgbot-runtime/src/workspace.rs`.

7. **Built-in web preview and video playback/analysis.** The project pane can
preview supported images and show a video thumbnail; attachments preserve the
original and can open in the system application. OrgBots does not embed a
browser or play/analyze full video in the room. Attachment context is bounded
to five files of 20 MiB each and eight images per model context. See
`app/lib/ui/desktop_project_pane.dart`, `app/lib/workspace/desktop_files.dart`,
and `core/crates/orgbot-runtime/src/attachments.rs`.

## Acceptance still requiring separate evidence

- Paid live-model collaboration, provider-specific tool compatibility and
  actual subscription-quota behavior have not been exercised; no paid calls
  were made during the documented validation.
- Multi-day endurance, recovery after host sleep/reboot, and unattended Linux
  operation have not been verified.
- Full video playback and analysis, Wayland/GPU performance, and other Linux
  desktop environments beyond the documented native test environment have not
  been accepted.
- Android device acceptance, comprehensive screen-reader testing and measured
  frame timing on the owner's Linux desktop remain unchecked.
