# OrgBots 2.0.3 verification

Date: 2026-10-27. Target: Ubuntu 24.04 amd64.
Publication status: published and independently verified.

## Changes and live proof

- OpenCode provider selection and protected Linux execution. The selected free
  V4.1 Flash endpoint reached its provider allowance limit; no paid fallback or
  successful free-endpoint response is claimed.
- Antigravity 1.4.8 uses a separately enrolled native account in a private
  Secret Service. Host keyring files and the host home are not mounted. API
  keys and extra G1 credits are not enabled by the integration.
- Enrollment preserves stored credentials and avoids the interactive settings
  atomic-rename failure. Settings, global configuration and the default project
  record remain read-only. Native log/session data persists per bot.
- The protected room MCP bridge supports messaging and authorized workspace
  operations. Native command/file/URL tools retain deny rules; project `.agents`
  and plugins are excluded. Current tool schemas are supplied in the standing
  brief, without granting native file reads.

The [Gemini runtime proof](benchmarks/2026-10-27-antigravity-runtime.json)
records eleven passing context checks, including Astra-to-Gemini routing,
same-session recall after process restart, fresh-room isolation and independent
native transcript reconciliation. Main-room usage was:

| Bot | CLI episodes | Input | Output | Cache read |
| --- | ---: | ---: | ---: | ---: |
| Astra via Codex | 1 | 13,100 | 246 | 61,184 |
| Gemini 3.8 Flash Medium via AGY | 2 | 53,013 | 5,010 | 29,868 |

The separate read-only permission probe confirmed an attempted native write
was denied. The authorized-write probe created and read back the exact requested
fixture through room MCP while retaining the original file. These are synthetic
live tests on the reviewed development implementation. Package-specific checks
are recorded separately below; no provider invoice or quota savings is inferred.

## Release checks

- Full Rust workspace: 879 passed, zero failures, three ignored. Optional
  credential-dependent tests are not additional live provider evidence.
- Full Flutter tests: 351 passed, five skipped. Focused integration checks
  passed 61 workspace-helper tests, 14 proof-validator tests and 13 Flutter
  tests. Full Flutter analysis reports one pre-existing warning at
  `app/lib/workspace/desktop_files.dart:192` (`unawaited_return_in_try_block`);
  the changed provider UI passed targeted analysis. No clean full-analysis
  result is claimed.
- Locked release engine and Flutter Linux release builds passed. Missing
  development headers were extracted to a private build directory; no host
  packages were installed. Packaged engine/helpers match the reviewed build
  and source; the GUI matches the rebuilt release bundle.
- Final packaged GUI opened at 1280 x 720 under a nested X server with fresh
  HOME/XDG state and private D-Bus/keyring. Its empty-workspace render was
  inspected. An accessibility embedding warning was recorded; this startup
  check does not establish a full interaction or accessibility audit.
- Linux acceptance: 17 passed, zero failed/skipped, including private D-Bus.
- Packaging, installer, signed-repository fixtures and Pages forwarding checks
  passed. The exact retained 2.0.2 packages upgraded to the exact 2.0.3 packages
  in a networkless isolated filesystem at normal kernel UID, using fakeroot
  for package ownership. All eight saved-data sentinels and the trash policy
  survived; host installed-file fingerprints were unchanged. Dependency
  installation was simulated separately using APT.
- Local signed metadata, fresh isolated APT refresh, 2.0.3 candidate selection,
  upgrade simulation with no removals and both downloaded hashes passed.
  All six older package files remain byte-for-byte unchanged.
- Fresh RustSec scan: 233 dependencies, zero vulnerabilities and zero warnings.
  Source Gitleaks scanning found one generic-key false positive: the public
  generated source SHA-256 in the historical runtime proof. It was reviewed
  as a digest, not a credential. Extracted package text scanning covered about
  1.15 MB and found no leaks; compiled binary code is outside that text scan.
- Website static checks cover six pages. Consent/focus tests and Chromium
  results/install renders at 375, 768 and 1440 pixels passed with no document
  overflow; the mobile evidence table scrolls within its region.

Final engine SHA-256:
`7b08d61de18c55af6f03bca42e22fbd4447cc3aa37ef922bbd4828290cdc8f6a`.

| Package | SHA-256 |
| --- | --- |
| orgbots_2.0.3_amd64.deb | `98093f0a573b3a245b6a4a554073aa36ff8043a1cb7b11f9877de05654f69c76` |
| orgbot_2.0.3_all.deb | `370e171b0aa523102a8a85c0b58ceaa01ff6817f9edf2d7c16f777d458db22b5` |

## Publication

Astra approved the exact reviewed packages and site changes before publication.
Source revision `f1931cfb810176bb4262995737526ed5c37c4a2e` was pushed to
`main` and `master`. Signed archive revision
`b71957bd068b8373e1636f4b52fd0e0f1ebf08e0` was pushed and verified before
[Pages run 40532408637](https://github.com/acememe7/Orgbot/actions/runs/40532408637)
was dispatched; its log confirms both exact revisions. Publication succeeded.

A fresh isolated APT refresh against the public HTTPS archive verified both
signatures, selected 2.0.3, solved the upgrade from installed 2.0.2 with no
removals, and downloaded both packages. Their hashes exactly match the table.

Website release `f1931cf` was activated only after public package verification.
All 19 public files and the active vhost match source. HTTP redirect, HTTPS
routes, genuine custom 404, security headers and direct-origin denial passed.
Live Chrome checks at 375 and 1440 pixels found no document overflow or
console, exception, loading or CSP errors. Keyboard focus was visible. The
previous `06153e4` release and vhost are retained for rollback.

The deployed site and vhost were uploaded to the existing private owner-only
Drive folder, independently downloaded and checksum-matched. All 20 manifest
entries restored into a fresh isolated directory. The 330,706-byte archive has
SHA-256 `62bd3e18d1739a0bb7bff70bd6ccede6061ef700d1c5322b77e7687697f6fcf6`.
Private folder/file identifiers and raw runtime evidence remain outside Git.

The staged release diff passed secret scanning with no findings. The earlier
source-history digest false positive is documented above. The owner's installed
`orgbots` and `orgbot` packages remain at 2.0.2; publishing did not install them.

Final scoped Astra review approved the public package, website, recovery
evidence and documentation. GitHub website/installer checks passed in runs
40532204564 and 40532203553 for the published source revision.

## Operational scope and limits

One AGY process may execute at a time; concurrent attempts fail before a model
call. Forced credential refresh, live external-MCP approvals and shell-command
workflows were not exercised by the AGY proof. Native account authentication
and separate token accounting do not establish billing or subscription quotas.

| Gate | Current verification scope |
| --- | --- |
| Live protections | Private CLI boundaries and permission tests passed. Public archive signatures, package hashes, upgrade selection and website activation were independently verified. Shared host SSH exposure, TLS 1.2 availability and provider-managed WAF settings are pre-existing limitations. |
| Backups | Older package files, site releases and private evidence were retained. The new site snapshot passed owner-only Drive readback, hash comparison and isolated restore. No new backup schedule or signing-key disaster-recovery claim. |
| Restore tests | Candidate upgrade/data-preservation and the predeployment site snapshot restore checks passed. The new deployed snapshot passed independent download and isolated restore. They do not establish full-machine recovery. |
| Legal/privacy | Static proof content introduces no website accounts, forms, tracking or data processor. Existing website privacy/transfer assessments and unresolved legal-review items remain unchanged. No broad compliance certification is claimed. |

Publishing the package does not upgrade the owner's running application or
rewrite existing room totals. No host installation or profile reset is included.
