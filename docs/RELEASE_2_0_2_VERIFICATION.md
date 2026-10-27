# OrgBots 2.0.2 verification

Date: 2026-10-27. Target: Ubuntu 24.04 amd64.

## Changes

- Task-scoped rule selection and on-demand skill/MCP discovery, loading,
  one-use/task lifetimes, release and reload. Native CLI transcripts remain
  owned by the CLI; unloading cannot erase their previous context.
- Codex resume separates option flags from prompts beginning with `---`.
- Independent reviewers can run offline checks in a read-only workspace copy,
  with temporary output, no mounted credentials and no integration of changes.
- Room MCP launchers recover the executable's original path after an atomic
  binary replacement, or report that a restart is required.
- Codex usage is counted as the increment since the last full session total,
  including after process restart. Unknown legacy baselines and incomplete or
  decreasing counters are reported as missing usage instead of inflated totals.
  Existing historical room totals are not rewritten.
- Compatible dependency patches update rustls, rustls-webpki, webpki-roots,
  quinn-proto and tokio. The final 233-dependency RustSec scan reports zero
  vulnerabilities and zero warnings, including the
  [certificate verification advisory](https://rustsec.org/advisories/RUSTSEC-2026-0382.html).

## Live evidence

One accepted Astra/Sol baseline and Astra/Sol/DeepSeek team pair completed both
tasks, including independent reviewer checks. Each passed the same six original
tests unchanged. Native sessions resumed successfully. The pair preceded the
accounting fix, so its figures were independently reconciled against native
Codex records. See the [usage report](benchmarks/2026-10-27-release-validation.md)
for disjoint token totals, API estimates and failed attempts. One pair does not
establish general savings or subscription quota percentages.

DeepSeek lifecycle probe 6 passed all 16 acceptance checks: discovery,
one-use skill consumption, selected MCP schema loading, exact-argument approval,
pause/restart with the same pending decision, schema reload, one actual external
call, released skill/schema content and a handoff to review. Its source files
and project sentinel were unchanged. The probe paused the room with its task
awaiting review; it does not claim a reviewer approved that one-worker fixture.
The previous five failed probe attempts remain in the accounting report.
An optional probe 7 on the dependency-patched final binary timed out after
restart; its shared allowance ledger reported insufficient request reservation
under the unchanged $0.25 ceiling. It remains incomplete. Probe 6 covers the
unchanged lifecycle code; no complete final-binary lifecycle rerun is claimed.

Three real Codex episodes, including native resume and provider reconstruction,
reconciled against the native session log: 35,083 uncached input, 59,904 cached
input and 250 output tokens. This diagnostic is separate from the comparison.

Twelve host CLI rule/configuration files retained their pre-test SHA-256 hashes.
Private profiles, credentials and raw transcripts remain outside Git.

## Local checks

- Current Python benchmark/probe tests: 25 passed. Workspace-helper and Linux
  acceptance-harness tests: 30 passed.
- Final Rust workspace run: 854 passed, zero failed, three ignored. The new
  ignored native usage test was explicitly run and passed separately. Optional
  provider tests that return early without credentials are not live evidence.
  Existing unused-variable warnings remain in test code. An earlier parallel
  run hit a fake CLI `Text file busy` failure; exclusive fixture directories
  were added, three provider-suite runs passed, and the final full run used
  one test thread. The original race was not conclusively reproduced.
- Flutter analyze: no issues. Flutter tests: 337 passed, five environment or
  fixture skips. The Flutter source is unchanged by this release continuation.
- Linux acceptance: 17 passed, zero skipped, including private D-Bus checks.
- Existing release GUI rendered at 1280 x 720 under a nested X server with
  private HOME/XDG/keyring state. The empty workspace screenshot was inspected.
  This is startup evidence, not a new responsive or complete interaction audit.
  Private portal/theme warnings were recorded; no user room was opened.
- Existing package, installer and signed-repository fixtures passed. Final
  packages passed an offline 2.0.1-to-2.0.2 upgrade in an isolated filesystem
  at kernel UID 1000, using fakeroot only to emulate package ownership. All
  eight saved-data sentinels and the trash policy survived; the host install
  was unchanged. Dependency installation was simulated separately using APT.
- Final signed metadata, isolated APT refresh, 2.0.2 selection, upgrade solver
  and downloaded package hashes passed locally. Older packages are retained.
- Gitleaks 8.30.1 found no secrets in the extracted package's scannable text
  and assets (about 1.08 MB); binary code is not covered by that text scan. The staged source diff also
  passed Gitleaks with zero findings.

## Release gates

- [x] Fresh native Codex usage proof, including resume and provider restart.
- [x] Final Rust regression run and release engine build.
- [x] Rebuilt packages, dependency scan, source/artifact secret checks.
- [x] Final package upgrade, signed metadata and dependency-solver checks.
- [x] Astra prepublication review of source, evidence and package identity.
  Scoped approval: no unresolved blocking findings; public verification follows.
- [x] Publication and independent public signed-archive download verification.

## Operational scope

This release changes a local desktop/CLI product and its signed GitHub Pages
package archive. It introduces no hosted account, database, payment service,
new telemetry, backup destination or data processor. No website deployment is
included. No host app installation or saved-room migration is performed by
publishing the archive.

| Gate | Configuration and verification status on 2026-10-27 |
| --- | --- |
| Live protections | Local sandbox/approval tests and signed archive validation passed; public HTTPS, signatures, candidate selection and both downloaded package hashes verified. GitHub-managed WAF, firewall and TLS-only policy configuration were not inspected. |
| Backups | Earlier signed package versions and private local evidence are retained. Scheduled independent archive/signing-key backup configuration was not checked; no new transfer or backup schedule is claimed. |
| Restore tests | Final rebuilt candidate passed isolated fixture upgrade/data preservation. No full machine or signing-key disaster recovery exercise was performed. |
| Legal/privacy | No new data flow or jurisdiction is introduced. Product-wide legal compliance was not reassessed or certified. Existing website/legal review gaps remain outside this package change. |

Saved evidence is in the owner's private release archive, grouped by immutable
live engine, trial, lifecycle probe and check run. Public hashes and the publication record are below.

Final engine SHA-256: `318aabb8673e7ac283003932c9feade1770918c00b36472f7bb84acb264feb29`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.0.2_amd64.deb | `1e06aaca19a903f06022a21ccc39d319179753dbb8f68bbee8f82bb1c68dc610` |
| orgbot_2.0.2_all.deb | `9985b991f93cde50c3479bd5b09b197963d738321a8dd517aad0dcbcd4cdd007` |

## Publication

Published on 2026-10-27 using
[GitHub Actions run 40480618565](https://github.com/acememe7/Orgbot/actions/runs/40480618565).
Implementation source is `72492bcb12aa8285e2c9220f8cb018d67a432441`; the workflow
checked out signed archive commit `e89f2517dc287ac69b913397da0761372fe54ee4`.
The first dispatch picked up the previous archive while its upload was still
finishing; the second dispatch and independent public verification confirm the
new archive.

A fresh isolated APT refresh against the public HTTPS archive verified both
OpenPGP signatures, selected 2.0.2, solved the 2.0.1 upgrade with no removals,
and downloaded both packages. Their hashes exactly match the table above.
The four older package files remain byte-for-byte unchanged in the archive.
Host APT sources, installed packages and saved rooms were not modified.

The staged diff and all newly published source commits passed Gitleaks. Public
verification evidence is retained privately as `apt-public-final/result.json`
and its command log. The release is published; the explicitly documented
operational and live-rerun limits remain.
