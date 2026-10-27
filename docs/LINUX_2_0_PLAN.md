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
- [x] Complete an accepted baseline/team pair (resolved 2026-10-27 for 2.0.2).
  The original pilot failed reviewer testing and native resume; the follow-up
  pair completed both rooms and passed six unchanged tests in each arm. See
  [accepted evidence and corrected usage](benchmarks/2026-10-27-release-validation.md).
  One accepted pair does not establish general savings.

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
- [x] Publish 2.0.2 and independently verify the public signed archive/download.
  Actions run 40480618565 deployed archive e89f251. Fresh public APT refresh,
  signature verification, upgrade solver and both package downloads passed;
  hashes match the reviewed final packages. Owner installation is unchanged.

Keep private logs/profiles/credentials outside Git. Do not infer subscription
quota savings from token counts or claim general savings from one pilot pair.

## OpenCode and Antigravity CLI integration, 2026-10-27 (initial status)

This records the source milestone before private AGY enrollment and runtime
verification. Current AGY results and remaining gates are summarized in the
later **Protected Antigravity runtime smoke** section.

Add desktop provider choices through the existing protected room runtime.
OpenCode uses the released 1.19.4 interface, not the separate v2 documentation;
Antigravity uses `agy` 1.4.8 rather than the IDE launcher. Preserve saved provider
identities, explicit session resume, per-agent usage, private profiles and the
room's external-tool approvals. Published 2.0.2 artifacts remain unchanged.

- [x] Inspect installed CLI/help and pinned upstream contracts; verify the
  isolated OpenCode binary against its official release digest.
- [x] Implement provider serialization, desktop selection and explicit model entry.
- [x] Implement OpenCode isolated execution, MCP configuration and session parsing.
  Real OpenCode 1.19.4 passed fresh/resumed local mock-provider execution and an
  allowed MCP tool call through the production Python workspace helper.
  Only an already configured, selected API credential is supported.
- [x] Resolve Antigravity authentication without exposing host D-Bus.
  Later owner enrollment and protected Linux execution are documented below;
  one room-MCP smoke passed. Cross-context and usage acceptance remains open.
- [ ] Verify safe OpenCode OAuth reuse.
  Blocked: copied refresh credentials may invalidate the owner's native session.
  OAuth is rejected; no automatic API billing fallback is used.
- [x] Inspect rendered Flutter UI fixtures at 375px and 1280px.
  Full Flutter suite: 345 passed, five platform/integration checks skipped.
  Analyzer retains one pre-existing warning at `desktop_files.dart:192`.
  Fifteen host rule/configuration files retain their pre-task SHA-256 hashes.
- [x] Finish regression checks: Rust workspace 869 passed, three ignored;
  Python workspace guard 29 passed. Live-provider tests remained opt-in;
  no external model calls were made. The real OpenCode mock proof also caught
  an unreported title-generation call, removed with a fixed fresh-session title.
- [x] Obtain final Astra review of the scoped milestone and evidence.
  Approved after configuration isolation, credential validation, terminal-event
  handling and proof-harness attribution corrections. This does not approve
  Antigravity execution, OpenCode OAuth, live-account access or a release.
- [x] Commit the verified source milestone with explicit runtime/release limits.
  Staged secret scan and formatting/diff checks passed; no credentials, raw
  prompts, session IDs or private profiles are included in the proof artifact.

See [native CLI integration status](NATIVE_CLI_INTEGRATIONS.md) and its sanitized
conformance record. A later Antigravity live inference is recorded separately;
it does not establish subscription billing or general savings. No package
rebuild/install, push, publication or deployment is included in this milestone.

## Subscription and free-model live validation, 2026-10-27 (initial snapshot)

This is the earlier live-test snapshot before the isolated Antigravity runtime
smoke below. Its OpenCode allowance result and completed Astra/DeepSeek run are
retained as historical evidence.

Owner requested context sharing, split usage and broader runtime checks before
any installation or publication. The selected team is Astra through the existing
Codex subscription, direct DeepSeek `deepseek-flash`, and OpenCode
`opencode/deepseek-v4.1-flash-free`. The owner confirmed V4.1 after the current
catalog did not list V4.2, and explicitly kept Gemini 3.8 Flash pending until
Antigravity can authenticate safely. No paid Gemini fallback is authorized.

Use synthetic fixtures, a current source-built engine and the checksum-verified
OpenCode audit binary outside the installed CLI paths. Reuse existing credentials
in memory and provider-specific private profiles. The test has its own allowance
ledger: direct DeepSeek is bounded to $0.25 at conservative peak rates; native
CLI episodes are bounded by request count and timeout. Free-model pricing is a
provider catalog claim, not a measured subscription saving or invoice.

- [ ] Verify one protected OpenCode free-model inference without fallback.
  Blocked: the selected V4.1 Flash Free route reported its free-usage limit
  reached; the one protected attempt timed out at 180 seconds with unknown usage.
- [x] Repeat the 16-check DeepSeek skill/MCP lifecycle proof on the current engine,
  including pending approval across pause/restart and context release.
  Second attempt passed; the first model instruction-following failure remains
  recorded with its usage. The strict validator was retained.
- [ ] Prove DeepSeek-to-OpenCode-to-Astra context relay, native resume across
  restart, independent room context, and per-agent/provider usage reconciliation.
  The three-model route remains blocked by OpenCode's allowance.
- [x] Run the available DeepSeek/Astra context, restart and room-isolation path.
  Final attempt passed nine applicable checks. Earlier harness failures and an
  interrupted attempt remain separate. Native Astra session identity survived
  restart; the isolated room used a different session and returned `UNKNOWN`.
- [ ] Run the existing code-work fixture with independent Astra review and
  unchanged acceptance tests using the selected three-provider team.
  OpenCode is unavailable; the three-provider variant was not run.
- [x] Run the same code-work fixture with available DeepSeek and Astra.
  Both tasks reviewed by Astra and submitted; all six original tests passed
  independently without changes. Usage: DeepSeek 17 API calls, Astra one CLI
  episode, with separate token/cache totals.
- [x] Record successful, failed and blocked checks separately; review and commit
  the sanitized evidence. Installation and publication remain gated.
  Final Astra review approved the scoped source/evidence milestone, including
  checkpoint hashes, usage sums, restart baseline and both allowance renders.

The [live validation report](benchmarks/2026-10-27-subscription-free-validation.md)
includes all attempt usage, provider blockers and accounting limits. Across the
retained attempts, Codex's 12 episodes and recorded tokens reconcile with its
ledger. DeepSeek has 60 completed calls plus two retained reservations whose
actual usage remains unknown: $0.037572 reported peak-rate estimates versus
$0.062635 conservative budget commitment, within the unchanged $0.25 cap.
The aggregate ledger cannot identify dropped requests; durable reservation
status remains a telemetry follow-up, not a completed accounting claim.

Focused verification: 48 benchmark/probe tests, four allowance widget tests,
targeted Flutter analysis clean, rendered allowance dialog at 375/1280 px,
and 16 host configuration/rule/auth hashes unchanged. No runtime Rust changes,
package installation, push, website deployment or publication were performed.

## Antigravity authentication diagnosis, 2026-10-27 (historical)

- [x] Confirm installed AGY 1.4.8 and trace the three deliberate execution gates.
  The blocker is access to its saved subscription sign-in from a protected
  profile, not executable discovery.
- [x] Check current official auth/headless contracts and installed help.
  No supported personal-account token import/export route was established;
  Gemini API and Cloud ADC were not used as fallbacks.
- [x] Prove a private Secret Service with synthetic data only: store, retrieve,
  restart the test keyring daemon and retrieve again. Host keyring untouched.
- [x] Run installed AGY in a network-disabled protected empty profile.
  It waited for an OAuth code until the 20-second timeout; no sign-in or live
  inference completed. This differs from the documented fail-fast behavior.
- [x] Implement private AGY sign-in/keyring lifecycle and complete owner sign-in.
  The guarded runtime smoke is documented below. Cross-context and isolated-room
  checks were still pending at the time of this diagnosis.

See the [diagnosis and evidence](NATIVE_CLI_INTEGRATIONS.md#antigravity-authentication-diagnosis-27-october-2026).
This milestone diagnoses and prototypes the missing authentication boundary;
it does not enable AGY or change installed applications, accounts or billing.

## Private Antigravity sign-in, 27 October 2026

The owner authorized a separate AGY sign-in while keeping access to the host
keyring blocked. The source change reaches the workspace helper, Linux CLI,
desktop Connections & tools, packaging dependencies and their focused tests.

- [x] Add owner-initiated `orgbot-engine agy login` and metadata-only `agy status`.
- [x] Provision a separate encrypted keyring, protected by one exactly scoped
  OrgBots secret-store item; never copy the existing AGY login or reset an
  existing encrypted store when its wrapping key is unavailable.
- [x] Start a private filesystem D-Bus socket and Secret Service with no service
  activation. Serialize operations and stop only the processes they started.
- [x] Run enrollment in an empty filesystem sandbox with a private home,
  controlled configuration and empty project. Exclude host credentials, desktop
  bus, plugins/hooks, API/ADC variables and paid-credit fallback.
- [x] Verify lifecycle, cancellation, unsafe paths, missing/locked credentials,
  filesystem/environment boundaries and desktop interaction; obtain final Astra
  review before offering the owner the sign-in flow.
- [x] Pass 45 workspace-helper tests, 13 Flutter tests, focused Rust CLI/profile/
  usage tests, analyzer and package metadata checks. Review full Setup renders
  at 375/1280 px. The real AGY binary reaches OAuth in the isolated networkless
  fixture; no model prompt or completed authentication was part of that test.
- [x] Resolve Astra's helper-discovery, passphrase-pipe and configuration-parent
  findings; final enrollment review approved. Open the owner sign-in terminal.
- [x] Complete owner sign-in and a protected room-MCP smoke.
- [x] Pass the 11-check cross-context/restart and isolated-room test, including
  session distinction and usage reconciliation; details follow below.
- [x] Complete the authorized room-MCP write/readback test with direct native
  writes denied; details are in the runtime-smoke section below.
- [ ] Check credential refresh and external-MCP approval behavior. Provider
  billing classification remains unknown.

Credential presence is enrollment metadata; account access is checked when the
bot runs. The separate runtime smoke and remaining acceptance gates are
recorded below. No installation, publication or deployment is part of this
change.
See the [sanitized enrollment proof](benchmarks/2026-10-27-antigravity-enrollment.json).

### Enrollment follow-up: post-login settings error

The owner completed the browser step, then AGY tried to normalize and atomically
replace the protected settings file. The earlier offline probe reached OAuth
only; it did not cover this authenticated startup path. Private credential
metadata is now present and must be preserved.

- [x] Make enrollment stop after credentials are stored, with a locked-keyring
  restart check; avoid starting the full AGY session.
- [x] Return existing stored credentials without launching AGY or reading the
  desktop wrapping key. Keep all filesystem/configuration restrictions.
- [x] Cover process-exit, credential-arrival and cancellation races; 49 helper
  tests pass. The owner profile returns stored/pending verification, and the
  cached-credential changelog command works with networking disabled and
  settings unchanged. Astra approved the source correction.
- [x] Complete final documentation/evidence review; Astra approved the
  enrollment correction. Commit it as a separate verified milestone.
- [x] Resolve authenticated model-runtime settings compatibility by retaining
  read-only settings and seeding the required default project record. The
  guarded runtime smoke is documented below; broader context and usage
  acceptance remains open.

### Protected headless startup compatibility

- [x] Find the second startup blocker: the protected global config had no
  default CLI project record. A response attempt failed before conversation
  creation because AGY could not create its `projects` directory.
- [x] Observe the native structural record in an isolated networkless fixture:
  `{"id":"default-cli-project","name":"CLI Project","projectResources":{}}`.
  Confirm the next run reuses this record with all global config read-only.
- [x] Add the exact record through the existing private JSON writer, preserving
  old profiles and rejecting unexpected records, resources, symlinks or
  hardlinks. Status accepts the old layout without modifying it.
- [x] Complete a protected Gemini 3.8 Flash Medium `READY` probe with the seed;
  all settings/global config stayed read-only, no API credential was supplied,
  and AI-credit overages were disabled. This is direct CLI evidence, not an
  OrgBots room integration or provider-billing proof.
- [x] Verify the final source-generated seed with the real CLI (`READY`, 6,781
  input / 30 output / 0 cache tokens), pass all 51 helper tests, and obtain
  Astra approval for the separate compatibility milestone.

The guarded runtime now keeps per-bot CLI homes separate from the isolated
global keyring, retains host auth-root resolution only for the trusted helper,
and sets per-bot XDG paths inside bubblewrap. It excludes workspace `.agents`
configuration, invokes AGY with the supported `--conversation` resume flag,
and exposes the protected room MCP relay. The live room-MCP smoke, 11-check
context/restart/isolated-room test and authorized room write/readback passed;
credential refresh and external-MCP approval checks remain open below.

## Protected Antigravity runtime smoke, 27 October 2026

- [x] Complete a real AGY 1.4.8 guarded server run with explicit model
  `gemini-3.8-flash-medium`; the agent returned `READY` through one room-MCP
  call. One usage event recorded 20,515 input, 1,945 output, and zero cache
  tokens.
- [x] Pass all eleven context-2 checks: Astra routed an exact nonce challenge to
  Gemini; after restart Gemini recalled the marker in the same native session;
  a fresh room with a distinct session returned `UNKNOWN`; hostile `.agents`
  content stayed masked and its source checksum remained unchanged.
- [x] Reconcile independent AGY transcript usage with engine totals and saved
  baseline; count the restart episode's delta once.
  Main room: Astra/Codex 13,100 input, 246 output, 61,184 cache-read tokens in
  one CLI episode; Gemini/AGY 53,013 input, 5,010 output, 29,868 cache-read
  tokens across two episodes. Isolated room: Gemini/AGY 12,707 input, 278
  output, zero cache-read tokens in one episode. Cache-write usage was zero.
  These are CLI episodes, not AGY internal model steps or invoice events.
- [x] Confirm per-agent private `HOME`, isolated keyring opened using one
  exact OrgBots Secret Service item, private D-Bus only, read-only settings,
  global configuration and default-project seed, empty `.agents`/plugin paths,
  only the room MCP server allowed, native command/file/URL tools denied, no
  API-key environment, and `useG1Credits=false`.
- [x] Confirm only one active AGY run is supported; a concurrent request fails
  before the provider call. Owner login remains explicit.
- [x] Pass 61 Python workspace-helper tests, 150 Rust tests, 14 probe-validator
  tests, 13 Flutter tests, Flutter analysis, and rendered 375/1280 px UI review.
- [x] Pass authorized `workspace_write` through room MCP: create
  `authorized.txt`, read back exact content, retain original fixture, and keep
  native file deny rules intact, project mount read-only, and shell
  commands disabled. This one CLI episode used 53,980 input, 844 output and
  zero cache tokens; billing classification is unknown.
- [x] Confirm room MCP read/post and direct native write-denial behavior in the
  separate read-only permissions run; native schema-file reads were avoided.
- [ ] Check credential refresh and live external-MCP approval behavior.

The route used the existing native AGY account sign-in. `billing_kind` remains
unknown; no invoice, quota treatment or zero-charge claim is available. The
[sanitized runtime report](benchmarks/2026-10-27-antigravity-runtime.json)
records the smoke and context proof. The relay artifact must be owner-controlled,
single-linked and not group- or world-writable; a Cargo hardlink at mode `0775` was
rejected and the run used a private checksum-identical copy without install.
AGY log compatibility allows only `cli.log` to point to relative
`log/cli-YYYYMMDD_HHMMSS.log`, resolving to a private owner-owned regular file
with one link; other symlinks and path escapes are rejected. No package
installation, publication, push or website update is included.
