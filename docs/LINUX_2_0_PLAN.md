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

Real subscription benchmarks wait for the owner's Claude/Ollama/Grok accounts.
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
