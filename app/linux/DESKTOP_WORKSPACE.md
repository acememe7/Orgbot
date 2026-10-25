# OrgBots desktop sessions

Implementation scope: persistent room sessions, targeted message replies and
mentions, desktop navigation, project files/preview/Git, local CLI integrations,
and a named specialist team. Android keeps its existing navigation and OAuth.

- Replies travel through the shared WebSocket/FFI protocol and persist in the
  transcript. An addressed message wakes its recipient; explicit Pause remains
  an owner-controlled stop.
- Desktop sessions have no default total-turn or elapsed-time cutoff. Idle
  sessions wait for work without model requests. Per-episode tool bounds,
  provider failures, checkpoints and manual Pause still apply.
- Native CLIs retain their installed integrations. Supported local MCP servers
  and skill directories are discoverable for API bots; unsupported plugin
  hooks/authentication are reported rather than presented as working tools.
- The desktop shell has a session sidebar, conversation, and project/work/team/
  memory inspector. Local attachments preserve originals; unsupported previews
  open in the system viewer. Git inspection is read-only.
- Keep existing bot identities/history and API credentials when expanding the
  team. Review remains an OrgBots team responsibility; nested CLI runs retain
  Luna delegation without launching a duplicate mandatory Astra review.

Implemented in 1.6.0. See [VALIDATION.md](VALIDATION.md) for test results, native
interaction checks, the saved-team migration, package checksum and acceptance
limits. Live provider spending and multi-day endurance require separate evidence.
