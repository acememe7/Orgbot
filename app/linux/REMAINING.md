# Linux desktop: delivered work and external acceptance

The feature list from 1.6.1 is implemented in the 1.7.0 source. Current evidence
and package details are recorded in [DESKTOP_COMPLETION.md](DESKTOP_COMPLETION.md)
and [VALIDATION.md](VALIDATION.md). The release package and clean native playback checks pass.

## Original feature list

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

## External acceptance and product limits

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

## Current completion checks

- Flutter: 264 passed, three external SSH checks skipped; analyzer clean.
- Rust workspace: 775 passed, one ignored; release engine built.
- Clean native MP4/WAV controls, package verification and final scoped Astra
  review passed; evidence is recorded in [VALIDATION.md](VALIDATION.md).
