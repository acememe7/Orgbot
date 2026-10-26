# OrgBots adoption update

Owner request, 26 October 2026: repair the Pages root, explain the product and
shared context accurately, update GitHub discovery metadata, make recovery
storage configurable, and use worker testing plus independent combined review.

## Scope and design

- Pages forwards to the canonical product site while the signed APT archive
  keeps its existing URL and bytes.
- One owner-private trash configuration is read by the trusted helper for
  integration, manual recovery and scheduled cleanup. New installations default
  to local user storage. Existing HDD selections and entries stay in place;
  ambiguous legacy upgrades require an explicit selection. Invalid settings or
  an unavailable selected mount stop the operation without fallback.
- Settings change the selected root through that helper, not by editing JSON
  from Flutter. Switching away from stored entries is refused. The systemd
  cleanup sandbox must allow only the selected root; seven full days remains
  the minimum retention age.
- Workers record focused test evidence, Quinn handles useful QA, and the
  reviewer owns checks of the combined result. These are agent instructions,
  not an automatic shell hook or engine-enforced proof of test completion.
- Comparisons describe documented product differences, not unsupported claims
  that competitors lack isolation or billing features. Ollama means Cloud in
  the GUI. Savings and exact subscription consumption remain unmeasured.

## Delivery checks

- [x] Pages source fix reviewed by Astra and published as `e004b23`; deployment
  `40382994415` succeeded. Public root HTML now serves the canonical forwarder
  and public APT Release remains reachable.
- [x] Browser forwarding verified; responsive site previews inspected at 375,
  768 and 1440 px, and static checks pass for all six pages.
- [x] README includes founder context, a sourced comparison and shared context.
  `docs/SHARING.md` retains the requested listing blurb. GitHub About, homepage
  and all 12 requested topics are set and read back.
- [x] Configurable trash backend: 29 helper tests, 25 workspace tests and the
  isolated fresh/managed-upgrade/foreign-file/timer-mismatch installer fixture
  pass. The reload-rollback race has a concurrent CLI-move regression.
- [x] Linux recovery-folder settings: 18 focused Flutter tests pass, targeted
  analysis passes, readable normal/narrow captures inspected.
- [x] Worker/QA/reviewer prompt and preset instructions updated. All 110 runtime
  crate tests and six specialist-preset checks pass. The Rust release build passes.
- [x] Linux 2.0.1 builds and package checks pass. Signed archive commit `9086ddc`
  preserves 2.0.0 packages; Pages run `40387218167` succeeded. A fresh isolated
  public APT refresh downloaded 2.0.1 with the expected SHA-256.
- [x] Astra source review and staged credential-pattern scan passed; source
  commit `13622b2` is pushed to both `main` and `master`.
- [x] Website release `13622b2` is live. Home/install/robots/sitemap bytes and
  CSP match source; browser forwarding and the custom HTTP 404 pass. Direct
  origin access remains denied. The matching private Google Drive backup passed
  download/hash comparison and a 20-file isolated restore.
- [x] Final scoped release and evidence review approved by Astra; no remaining
  corrective blockers in this update.

No paid provider calls, real trash purges, host app upgrade or third-party
promotional PRs are part of these verification fixtures.

## Verification notes

The real trusted workspace/trash integration fixture preserves replaced bytes
and keeps project removals pending until the exact deletion review token is
approved (25 workspace tests). A transient user-unit fixture with a private
HOME/XDG directory, read-only home, strict filesystem sandbox and only its
recovery folder writable passed `cleanup --dry-run`. It did not run the owner
cleanup timer or touch existing recovery entries. The Flutter Linux release
build passes. Package payload/dependency inspection passes; packaging emits
existing Flutter/RPATH dependency-scanner warnings. Two existing unused test
variables produce runtime-test compiler warnings.

Review caught and corrected a root-switch rollback race and manager/caller XDG
mismatch before publication. Astra approved the scoped implementation after
re-review. Installer mismatch refuses timer enablement but may leave staged
managed configuration/files and retained backups; normal storage changes roll
back on sync failure. Final packaged helpers, engine, preset, maintainer script
and copyright were compared with the reviewed sources. Public package SHA-256:
`ece4bc912a9847d0922cc5032d9aa7a4ed6e678540f6cf23db427d730ea512c5`.

Website publication changed only the OrgBots release link and the structured-data
CSP hash in its existing vhost. The old release and vhost were preserved. Nginx
validation passed with pre-existing protocol-option warnings in other vhosts;
those sites were not changed. One Python urllib request was refused by the edge;
curl and actual browser requests subsequently returned the verified live pages.
The owner app was not upgraded, existing trash was not purged, and no paid-model
benchmark was run. These checks do not establish token savings or live-provider
quality.
