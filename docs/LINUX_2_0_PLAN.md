# OrgBots Linux 2.0 delivery

Owner request: implement the six audited gaps, strengthen acceptance testing,
show per-agent and per-room input/output/cache usage and spending, and publish
a signed GitHub Pages APT repository for `apt install orgbots`.

## Implementation and ownership

- [x] Accounting: backward-compatible durable usage events; disjoint uncached
  input/output/cache read/cache write counts; model rates; complete/partial
  metering; actual API estimates separate from subscription API equivalents.
  Worker: usage_benchmark (Rust accounting, Flutter usage models/view).
- [x] Repeated benchmark workloads with explicit acceptance checks, baseline
  and team reports; deterministic fixtures now, opt-in paid runs later.
  Worker: usage_benchmark.
- [x] Isolated per-agent working copies, Linux process sandbox, protected
  baseline manifest, conflict-checked integration, HDD retention and explicit
  owner approval before applying deletions. Helper: linux_continuity;
  provider/API/UI integration: primary.
- [x] Controlled local API command execution inside the same protected
  working copies; bounded output/time/process lifetime. Primary.
- [x] Room-service notifications while the GUI is closed; action opens the
  saved room and surfaces its existing questions. Primary.
- [x] Portable private backup/import into a fresh paused room/project with
  integrity/path/resource validation and no credential export or execution.
  Worker: portable_backup (backup/UI/native picker).
- [x] `orgbots` 2.0.0 Debian package preserving existing paths/data, signed
  APT repository and one-time Signed-By setup. Host selected by owner:
  https://acememe7.github.io/Orgbot/ . Worker: apt_distribution; primary
  performs final artifact verification and publication.

## Verification gates

- [x] Focused regressions and malicious/invalid input tests for each boundary.
- [x] Full Rust/Flutter test suites, isolated keyring and user-data fixtures.
- [x] Real Linux sandbox denial, concurrent edits, conflicts, delete retention.
- [x] Backup export and actual isolated restore, corruption/no-overwrite tests.
- [x] Private D-Bus notification delivery, action and GUI routing checks.
- [x] Controlled crash/restart, disconnect, quota cooldown, pause/resume and
  long-running scripted workload; no paid provider calls in these fixtures.
- [x] Current release build, rendered Linux flows, package content/dependencies.
- [x] Signed APT metadata, untrusted/tampered/expired rejection and isolated
  installation; public Pages availability after publication.
- [x] Secret scan, mandatory Astra final review and verified local commit.

The Astra/Sol/DeepSeek live pilot is recorded below. Broader subscription
benchmarks still wait for the owner's Claude/Ollama/Grok accounts.
Elapsed multi-day operation and physical suspend/reboot are separate evidence:
accelerated clocks/process restarts cannot be reported as those checks.
Live host suspension/reboot must not interrupt unrelated user work.

## Verification record — 2026-10-26

- Current workspace Rust suite: **827 passed, 2 opt-in tests ignored**;
  the private-service notification test is exercised separately by the harness.
- Flutter suite: **327 passed**, with its four fixture-gated checks subsequently
  run separately: **3 SSH + 1 private D-Bus passed**. No real credentials used.
- Production `lib/`, `tool/` and all seven changed test/fixture files pass
  targeted analysis. Whole-app analysis reports no code diagnostics but emits
  host inotify resource errors; this is not a clean whole-app analysis result.
- Workspace helper **24 tests**, trash helper **17 tests**, Linux acceptance
  harness **12/12 scenarios** and benchmark/harness unit checks **17 tests** pass.
- Synthetic benchmark: three trials of two workloads per strategy, all **12
  artifacts accepted**. Usage/prices remain unknown; no claimed savings.
- Rendered production widgets at 1280×720 and 1024×720: usage, conflicts,
  pending deletions and permission dialog; no Flutter errors in fixture capture.
- Current Rust release engine and production Flutter Linux release build pass.
- Package signature, expiry/tamper rejection, download, transitional-upgrade
  and isolated installation fixtures pass. Actual package executable modes,
  launcher links, extracted engine and plugin runtime links pass.
- Astra approved the scoped implementation after all requested corrections.
  Clean native GTK cold/warm activation passes in a private session.
  Signed actual package refresh/download verifies the expected SHA-256.
  Astra approved final source/artifacts for publication. GitHub Pages deployment
  `40348136526` succeeded; public APT refresh and both package downloads passed
  with the expected package hash. No host installation was performed.

Local logs are under `/tmp/orgbots2-*`; the acceptance report is
`/tmp/gptbots-linux-acceptance-current.json`. No live provider calls, host
reboots or multi-day elapsed trials were performed. The Linux sandbox protects
workspace writes; it is not complete isolation from readable host credentials
or every network/Unix service. See `WORKSPACE_SAFETY.md`. Portable backups are
private, unencrypted and capped at 64 MiB; pasted secrets in history must be
treated as sensitive. Existing-file replacement requires the configured HDD
trash mount to be available.

Publication: `https://acememe7.github.io/Orgbot/apt/`, source milestone
`cc100d5`, archive branch milestone `f4e5d24`. The actual public package SHA-256
is `7597e7729aabb629735fd71aa05a287a0e255b2c79c817d497560e468baa8da1`.
See `apt-distribution.md` for the one-time client setup and maintenance window.

## Documentation correction — 2026-10-26

- [x] Replace the remaining Apache appendix placeholder with `Copyright 2026
  The AECE Studio`; all other license text is unchanged.
- [x] Audit the root README against source and published APT metadata. Correct
  installation/build commands, platform/provider distinctions, specialist roles,
  the usage-report location, CLI continuity and verification limits. Both build
  guides now list the WebKitGTK and VTE development packages required by CMake;
  their package names were checked against Ubuntu 24.04 APT metadata.
- [x] Validate all 13 local Markdown links, 12 shell blocks and both TOML blocks.
  The existing engine accepts the combined example with placeholder credentials
  and temporary paths using `orgbot check`; no model turn is executed.
- [x] Existing Debian packaging fixture passes with the corrected license.
  No app build, host installation or paid-provider benchmark was performed for
  this documentation-only change; the published 2.0.0 package is unchanged.

## Live Astra / Sol / DeepSeek pilot — 2026-10-26

- [x] Use the saved DeepSeek key and existing Codex CLI login for isolated,
  bounded live attempts. Retain raw evidence privately; commit sanitized usage.
- [x] Correct CLI profile-copy limits/errors and restore protected DNS without
  exposing the host runtime directory. Release engine build, three focused Rust
  tests, 25 workspace helper tests and 18 benchmark harness tests pass.
- [x] Preserve failed-run usage, require room completion and unchanged independent
  acceptance tests, and count requests from usage events correctly.
- [x] Record model usage, API estimate and separate CLI diagnostic consumption
  in the [pilot report](benchmarks/2026-10-26-astra-sol-deepseek.md).
- [ ] Complete an accepted baseline/team pair. The latest baseline code passed
  six tests, but review-only command restrictions blocked combined reviewer
  testing, and the next Codex resume invocation failed CLI argument parsing.
  Those observed workflow blockers remain unresolved. No savings are claimed.

The installed application, public package and website were not changed by this
pilot. Earlier verification records above describe their original scoped runs;
they do not supersede these live failures.

## Task-scoped rules and input accounting — 2026-10-26

Scope: route available domain manuals by the assigned task while retaining core
permissions, safety and engineering requirements. Reuse the existing skill index
and on-demand `use_skill` path. Put the shared policy in the stable system brief
so API and CLI agents receive it, without adding task-dependent text to the
cached prefix. Preserve all host/private rule files and tool availability.

- [x] Verify prompt delivery; release engine build, 109 runtime tests and 30
  native CLI provider tests pass. Two pre-existing unused-variable warnings
  remain in runtime restart tests; no new code diagnostics.
- [x] Check actual selected rule reads in one bounded Sol episode: core and
  production rules read; no UI/game manuals observed; six unchanged tests pass.
  The room pauses at its one-turn budget with both tasks awaiting review.
- [x] Record per-response usage and attribution limits in the
  [context audit](benchmarks/2026-10-26-context-audit.md). Host config and twelve
  instruction files retain their pre-probe hashes. This is instruction-based
  selection, not a filter that removes files or guaranteed quota savings.
- [x] Obtain Astra review and save the verified milestone locally. Scoped review
  approved the policy, evidence, arithmetic and stated limitations.

The reviewer command restriction and native CLI resume parser defect remain
separate workflow blockers; this work does not relax those safety boundaries.

## Skills and MCP lifecycle — 2026-10-26

Use one stable discovery/load/call/unload surface for API agents and the native
CLI room bridge. Skill bodies and selected tool schemas load on demand; catalogs
return bounded summaries. Task leases end when work leaves the worker; one-use
leases expire after consumption. Preserve protocol call/result pairing when
removing released context from API requests. Native CLI transcripts remain owned
by their CLI; do not reset sessions to simulate selective context removal.

Connect only the requested MCP server and isolate connection leases per agent.
Unloading releases that agent's connection when no selected tools still need it.
Every actual external call retains exact-argument owner approval and reviewer
restrictions. Configurations, source skills and host profiles remain untouched.

- [x] Implement lifecycle state, discovery and both dispatch paths.
- [x] Verify expiry, reload, restart, lazy connections and authorization locally.
  Cancellation expires temporary leases before checkpointing; same-response task
  release blocks later MCP use; active context can reload after compaction.
- [x] Run affected regression checks and obtain Astra review: 128 runtime and
  104 server tests passed. Two existing server tests require separate fixtures;
  opt-in live-provider checks were not enabled. Two pre-existing unused-variable
  warnings remain in runtime restart tests. No paid model calls were made.
  The optimized server build passed; installed packages and public distribution
  remain unchanged.

## 2.0.2 acceptance and publication — 2026-10-27

Owner authorized repeated testing and corrective work, then packaging and
publication. Release gates: preserve saved rooms and approvals; prove native
session resume with the installed CLI; let an independent reviewer run combined
checks without project writes or network access; finish a baseline/split-team
fixture with unchanged acceptance tests and complete usage accounting; exercise
skill/MCP lifecycle and pause/restart; pass package/signature/upgrade checks.

- [x] Correct and verify native resume and safe reviewer testing.
- [x] Capture accepted bounded live runs and retain failures separately.
  One pair passed unchanged tests; lifecycle probe 6 passed all 16 gates.
  Three native accounting episodes reconcile across resume/restart. Probe 7
  remains incomplete, with the shared allowance ledger reporting insufficient
  reservation. See [release verification](RELEASE_2_0_2_VERIFICATION.md).
- [x] Complete source, desktop, package and signed archive acceptance.
  Final Rust: 854 passed, three ignored; patched dependency audit clean;
  staged/package secret scans clean; final rootless fixture upgrade and local
  signed APT selection/download verified. Public verification follows publication.
- [x] Obtain Astra review of the final implementation and evidence.
  Scoped prepublication approval covers exact signed artifacts and recorded limits.
- [ ] Publish 2.0.2 and independently verify the public signed archive/download.

Keep private logs/profiles/credentials outside Git. Do not infer subscription
quota savings from token counts or claim general savings from one pilot pair.
