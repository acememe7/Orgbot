# Linux desktop completion record — 1.8.0

Desktop motion, Linux branding, collapsible per-agent activity and durable owner
questions are implemented. Questions provide choices and custom input, resume
only their asking agents, and retain retry behavior across view changes.
The separate HDD trash script and timer are installed and verified; cleanup is
not an agent task and consumes no model allowance.

Final Flutter (285 passed, three external SSH skips), analyzer, affected Rust
checks, release builds, package contents and isolated native launch/readback
passed. Detailed counts, frame timings, hashes and evidence are recorded in
[VALIDATION.md](VALIDATION.md). The real Linux notification plugin passed Notify/click-callback checks against
an isolated synthetic D-Bus service; this does not establish GNOME toast/focus
acceptance. The staged credential-pattern scan found no credentials in 54 owned
files. Final scoped Astra review approved the source, integration, notification fixture,
visuals, package evidence and documentation; all required findings are resolved.

The 30-second scripted activity profile measures UI p95 4.307 ms and raster
p95 16.788 ms, with larger rendering spikes. Wide and 800 px desktop views were
inspected; 375 px desktop mode is cramped. This does not establish uniform
60 FPS, multi-day, Wayland or physical Android acceptance. Notifications require
the GUI to remain running. The trash workflow cannot intercept arbitrary `rm`
or other application deletions; use inactive files and the documented command.

The GUI package is built, not installed. The owner-authorized standalone cleanup
timer is enabled and active. No paid inference or user-file deletion was used
for this verification. See [REMAINING.md](REMAINING.md) and [TRASH.md](TRASH.md).

---

# Linux desktop completion record — 1.7.0

The seven feature gaps from 1.6.1 are implemented. Scope and external acceptance
limits are recorded in [REMAINING.md](REMAINING.md); artifact details are in
[VALIDATION.md](VALIDATION.md). A passing build does not establish multi-day,
Wayland, physical Android or universal provider/tool acceptance.

## Delivered capabilities

- Persistent per-room Linux supervision, private descriptors/checkpoints,
  Secret Service credentials, UI detach/reconnect, crash restart, explicit stop,
  and pause-preserving recovery. Duplicate monitors cannot start duplicate rooms.
- Durable provider allowances shared across rooms: request/CLI launch counters,
  reset windows, API text token/cost reservations with owner-configured prices,
  cooldowns and role-preserving routing. CLI internals and unpublished account
  quotas remain opaque; an agent launch is not a token or monetary cap.
- Offline project memory with automatic completed-work capture, local concept
  vectors, bounded history summaries, and owner search/hide/delete/share controls.
- Managed MCP configuration, authentication and connection tests; skill
  folder/ZIP installation/removal; supported native CLI plugin operations.
  Unsupported formats/policies remain unavailable to API agents. Native CLI
  agents retain their installed tools and permissions.
- Native VTE terminal, conflict-safe text editing with recovery ownership,
  Git branch/commit-staged/merge actions, embedded HTTP(S) browser and native
  audio/video playback. Browser state survives geometry changes and dialogs.
- Four timestamped video samples for model context, retained originals and
  system Open With. Playback depends on installed codecs; visual sampling does
  not claim full temporal or audio understanding.

## Verification evidence

- **Supervisor:** isolated systemd service smoke covered synthetic keyring
  storage, attach/reconnect, crash restart, explicit stop, credential clearing,
  restart after stop and checkpoint retention. No user room was involved.
- **Endurance:** 600.1 seconds, ten scheduled cycles, 12 fake CLI launches and
  12 matching allowance ledger entries, durable checkpoints, crash recovery,
  paused state preserved. No live inference in this fixture. This is narrower
  than multi-day, sleep or reboot acceptance.
- **Providers:** saved DeepSeek key accepted; bounded live response used 48
  input and 2 output tokens. Codex reply/resume and Grok reply passed. Claude's
  installed Pro session was authenticated but exhausted its weekly allowance;
  it reported reset at 11 PM Africa/Lagos. Ollama was not contacted because the
  owner is unsubscribed. Availability is time-specific.
- **Integrations:** eight discovered MCP entries, six configured, one reachable,
  five unavailable and two unsupported during read-only initialize/tools-list
  checks. No MCP tools were executed. Isolated Secret Service read/write/delete
  and room credentials passed; real saved provider keys were never logged.
- **Tests:** Flutter 264 passed, three external SSH checks skipped; analyzer
  clean. Rust workspace 775 passed, zero failed, one ignored; release engine
  build passed. A fake CLI test was made independent of external shell tools
  after one unexplained suite failure; two subsequent workspace runs passed.
- **Native desktop:** actual X11/Nvidia profile build, isolated app data/keyring,
  1440×920, 800×920 and 375×920. Verified terminal input/project working directory,
  text save to disk, Git branch creation, spending-control save, memory loading,
  scripted room reply and reconnect, browser interaction/state retention and
  modal occlusion. Native recovery, overlay allocation, navigation and document
  lifecycle regression checks passed.
- **Media:** actual embedded MP4 controls advanced, paused at about 0.585 seconds,
  sought to 2.5 seconds and retained playhead on resize; WAV advanced and paused
  at about 0.46 seconds. The generated preview enables browser control scripting
  while CSP blocks page scripts and external resources. Diagnostic probes used
  for acceptance are removed from the final source. The isolated audio fixture
  uses the host PulseAudio socket; no audible-listening acceptance is claimed.
- **Profile sample:** 32 inspector switches produced 115 measured Flutter frames
  on the X11/Nvidia desktop. UI median 1.272 ms, p95 8.440 ms, max 9.170 ms;
  raster thread median 3.378 ms, p95 16.581 ms, max 17.263 ms (four samples over
  16.67 ms). These are trace durations, not GPU hardware timings or a universal
  FPS guarantee; the workload is a small scripted room, not a large live run.

## Build and evidence locations

GTK builds require `libwebkit2gtk-4.1-dev`, `libvte-2.91-dev` and
`libsecret-1-dev`, plus Flutter/Rust. Runtime packaging includes Secret Service
CLI tools, FFmpeg, GStreamer codecs and discovered WebKit/VTE dependencies.

Temporary evidence: `/tmp/orgbots-completion-*.log`,
`/tmp/orgbots-completion-native/shots/`,
`/tmp/orgbots-completion-native/timeline-interactions.json` and
`/tmp/orgbots-completion-native/media-diagnostic.log`.

Clean-profile media retest and release-package verification passed. Final
scoped Astra approval is recorded in [VALIDATION.md](VALIDATION.md).
