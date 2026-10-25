# Linux desktop: current inventory — 1.8.0, 2026-10-25

The requested motion, desktop icon, collapsible activity, owner-question choices
and standalone HDD trash workflow are implemented. Verification and artifact
hashes are in [VALIDATION.md](VALIDATION.md).

- Short transitions respect reduced motion; hidden-page animations stop.
- Each agent has independent public status and up to 12 recent tool steps.
  Activity updates are batched, and text deltas do not rebuild the preview.
- Owner questions persist, accept suggested or custom answers, and resume their
  asking agents. Retry/disposal and taskless questions are covered by tests.
  Waiting questions do not spend empty planning turns or grant permissions.
- Linux notifications route to the saved room while its GUI is running.
- The standalone trash command and daily timer are installed, tested and active.
  Entries become eligible after seven full days, on day eight or later. Cleanup
  has no model calls. See [TRASH.md](TRASH.md) for restore and safety limits.
- Final Flutter: 285 passed, three external SSH skips; analyzer clean. Rust
  workspace: 793 passed, one ignored, with subsequent affected-server regression
  checks passed. Release builds, package content/dependencies and native isolated
  launch/readback passed. No paid-provider calls were made in 1.8.0.

## Remaining acceptance and limits

- Final scoped Astra review approved the implementation and verification
  evidence. The Linux notification plugin's D-Bus delivery and click callback
  passed with a synthetic service; actual desktop-daemon toast/click-to-focus
  acceptance remains unchecked. Staged credential-pattern scan: zero findings.
- The GUI package has not been installed. Existing room services need a deliberate
  restart to adopt the new engine; active work is not interrupted automatically.
- Wide and 800 px desktop layouts were checked. A 375 px desktop window remains
  cramped. The final scripted profile has UI p95 4.307 ms and raster p95
  16.788 ms, with a 66.781 ms raster maximum; no uniform 60 FPS claim is made.
- Notifications require the GUI to remain running; background service-only
  delivery/relaunch is not implemented.
- The trash script protects its supported workflow, not arbitrary CLI/application
  deletion. It supports inactive regular files/directories and rejects unsafe
  entries. Timer cleanup requires a user systemd session; missed runs catch up.
- Multi-day operation, sleep/reboot, Wayland, physical Android, comprehensive
  screen-reader and new live-provider acceptance remain separate checks.

## Prior implementation inventory — 1.7.1 and earlier

The feature list below records earlier work and its original acceptance limits.

The feature list from 1.6.1 was implemented; 1.7.1 added coordinator routing
and CLI continuity corrections. Historical evidence and package details are
recorded in [DESKTOP_COMPLETION.md](DESKTOP_COMPLETION.md) and
[VALIDATION.md](VALIDATION.md). Those earlier release results do not satisfy the
1.8 acceptance limits above.

The 1.7.1 roster migration was designed to run once when that Linux app opens.
Existing running room services must be stopped and restarted to adopt edited
roles and the updated engine. That 1.7.1 update was not installed on the owner's
system.

## Existing capabilities and original acceptance limits

1. **Background supervision:** private per-room user services, Secret Service
   credentials, checkpoints, UI detach/reconnect, crash recovery, explicit stop,
   and pause-preserving restart. An isolated systemd lifecycle smoke and a
   600.1-second fake-provider endurance run passed. Host sleep, reboot and
   multi-day elapsed operation still require separate acceptance.

2. **Provider allowances:** shared durable request counters and configurable
   daily/weekly windows; API token and monetary reservations use configured
   prices. CLI allowances count agent launches, since their internal calls and
   subscription accounting are opaque. No unpublished subscription-quota or
   automatic pricing feed is claimed. Corrupt accounting fails closed.

3. **Allowance-aware routing:** disabled/exhausted providers are excluded from
   eligible work; rate limits create shared cooldowns and waits. Routing keeps
   role and explicit-recipient constraints. It never silently changes a named
   reviewer into a general worker. Unknown provider quota remains unknown.

4. **Project memory:** automatically captured completed work, local concept
   vector retrieval, cross-room project memory, owner hide/delete/share controls,
   and bounded conversation summaries. Retrieval is offline and does not call
   an embedding API. It is not a hosted neural embedding or universal recall
   service.

5. **MCP, skills and plugins:** managed MCP editing, authentication and connection
   tests; local skill folder/ZIP installation and removal; native CLI plugin
   operations where the CLI supports them. API agents import supported formats,
   transports and policies. CLI agents keep their own installed configuration.
   Unsupported formats, per-tool filters, transports and plugin-only hooks are
   not turned into unrestricted tools. Universal interoperability is not claimed.

6. **Native workspace:** a VTE terminal in the selected project, a text editor
   with conflict/recovery protection, and Git branch, commit-staged and merge
   actions. The API-agent `workspace_run` boundary remains unchanged; native
   CLI agents use their existing CLI permissions.

7. **Web and media:** embedded HTTP(S) web preview, native codec-dependent media
   playback, and four sampled video frames with timestamps for model context.
   MP4 play/pause/seek and WAV play/pause passed on X11. Sampling does not establish
   full temporal or audio understanding. Originals and system Open With remain
   available; attachments retain the five-file and 20 MiB/file limits.

## External acceptance inherited from 1.7.1

- Measured multi-day supervision, host sleep/reboot and unattended live-provider
  work remain unchecked. The ten-minute recovery fixture is narrower evidence.
- Wayland and other desktop environments are unverified. Current native visual,
  keyboard and interaction checks use the owner's X11/Nvidia desktop.
- Android device installation and comprehensive screen-reader acceptance remain
  open. Linux UI changes and Rust/FFI tests do not establish physical Android
  acceptance.
- Saved DeepSeek authentication and a bounded request passed (48 input, 2 output
  tokens). Codex reply/resume and Grok reply passed. Claude was authenticated but
  its weekly allowance was exhausted at the time of the check, with the provider
  reporting an 11 PM Africa/Lagos reset. Ollama was not contacted because the
  owner is unsubscribed. These are time-specific checks, not availability promises.
- Local MCP discovery/initialize checks cover recognized configurations; each
  actual tool and provider combination still needs acceptance in its own task.
- Active agents consume subscription/API allowance. Splitting work across models
  does not guarantee fewer total tokens. Idle waiting makes no inference calls.

## 1.7.1 completion checks (historical)

- Flutter: 270 passed, three external SSH checks skipped; analyzer clean.
- Rust workspace: 782 passed, one ignored; release engine built. Python: six
  preset checks passed.
- 1.7.1 package extraction, old-roster migration and native Team view passed.
  The native MP4/WAV checks are historical 1.7.0 evidence; that UI is unchanged.
  Evidence is recorded in [VALIDATION.md](VALIDATION.md).
