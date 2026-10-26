# OrgBots website and installer

The product site lives in `website/`, with no build step or external runtime
dependencies. Each page works as ordinary HTML; small JavaScript enhancements
handle copying installation commands and viewing product examples. Public
assets contain no room data, credentials or live provider requests.

The installer is `scripts/install-orgbots.sh`. Its source of packages remains
the signed GitHub Pages APT archive. The product subdomain is separately hosted
at `https://orgbots.theaecestudio.com`; it must not replace the APT Pages domain.

## Scope and verification

Changes reach the README, installation and archive guides, installer fixtures,
static pages/assets, hosting configuration and static-site checks. Existing
desktop binaries, saved rooms, provider credentials and other hosted sites are
outside this change.

Run `python3 scripts/test-install-orgbots.py` and
`python3 scripts/check-website.py`, then inspect the site in a browser at
375, 768 and 1440 pixels. Check navigation, command copying, keyboard focus,
privacy controls, product examples and the custom 404. Verify the deployed
HTTPS responses, headers and public installer after publication.

## Updating the evidence

`website/results/index.html` is the public evidence page. Its current figures
come from `docs/LINUX_2_0_PLAN.md`, not a live provider comparison. Preserve this
distinction when editing it. The screenshots use synthetic fixture data.

For real comparisons, use `scripts/benchmarks/README.md` and its existing
baseline/team harness. Record the exact source revision, model identifiers,
provider plans, task fixtures, trial count, run dates and machine environment.
Publish accepted outputs and failures alongside elapsed time, human
interventions, input/output/cache counts, API charges or estimates and unknown
values. Keep subscription usage separate from hypothetical API equivalents.

Before publishing reports, remove credentials, private prompts, machine paths
and user identifiers. Commit a sanitized report under `docs/benchmarks/` and
link that exact commit from the results page. Update installation limitations
only when the corresponding checks have passed. A synthetic run, accelerated
clock or process restart is not paid savings or real multi-day evidence.

## Delivery record

Verified on 26 October 2026:

- Installer fixtures pass for fingerprint mismatch, extra primary keys, GPG and
  download failures, unsupported platforms, missing prerequisites, symlinks,
  unchanged setup and preserved backups using GNU `install`. No host APT files
  were changed by those fixtures. The public script matches the repository.
- Six-page metadata, headings, internal links/assets, sitemap and CSP hash
  checks pass. Bash and JavaScript syntax checks pass. GitHub Actions checks
  passed for source `136dcb5` (run `40358905685`).
- Browser checks at measured content widths of 375, 768 and 1440 pixels show
  no document overflow. Mobile menu/Escape focus, installation copy success,
  screenshot switching and privacy dismissal/reopening work. A local failure
  fixture verified blocked clipboard guidance and screenshot error, loading
  skeleton and successful retry. Main text/background pairs exceed 4.5:1.
  Reduced-motion rules were inspected; OS-level motion emulation was not run.
- The dedicated subdomain is live behind Cloudflare with strict origin TLS.
  HTTP redirects to HTTPS; all five public pages, scripts, styles, sample
  image/favicon/share assets, sitemap and robots return 200; a missing page
  returns the custom 404. Tested responses include HSTS, nosniff and CSP.
  Site-script errors were not observed in the tested browser; unrelated
  installed-extension warnings were excluded.
- The first deployed release and its replacement are retained separately.
  All 19 deployed website files matched a SHA-256 manifest. A downloaded copy
  was extracted into an isolated local folder and all 19 checksums matched.
  This is a static-content restore check, not a full host recovery exercise.

Paid provider comparisons, physical suspend/reboot and real multi-day runs
remain unperformed. This task changed installation/distribution and the website,
not application code. The following record updates the earlier setup gates.

## Follow-up on 26 October 2026

- Google Search Console ownership is verified for the HTTPS OrgBots URL-prefix
  property. Google fetched `sitemap.xml` successfully and discovered five pages.
  This is not proof that the pages are indexed or have search traffic.
- Decorative arrows were removed from the home and evidence links. The layout
  is preserved. Copyright notices identify 2026 The AECE Studio; the owner
  confirmed this is a business name registered in Nigeria. Standard Apache 2.0
  license terms remain intact. Package builders now include the notices in
  future packages; already-published 2.0 binaries have not been rebuilt.
- A real manual Cloudflare Web Analytics property exists. The implementation is
  prepared behind `data-analytics-enabled="true"`, but every public page sets
  it to `false`. The disabled path does not access consent storage or load a
  beacon. The privacy page and footer describe that actual state. Before
  enabling, resolve GAID Article 31 applicability with qualified review, complete
  any required DPIA, reconcile the studio's wider no-tracking statement, update
  public notices/CSP, and verify real consent/network behavior. Fixture coverage
  includes cross-tab withdrawal, page restoration, expiry, and storage failures.
- Cloudflare automatic beacon injection is prevented by `public, no-transform`;
  the CSP continues to allow only same-origin scripts/connections and exact
  JSON-LD hashes. The OrgBots-only Nginx check uses the original socket peer,
  preserving the visitor address for logs. Direct HTTP and HTTPS origin
  requests return 403; the proxied website and a real HTTP ACME challenge file
  return 200. The main and games websites still return 200. Nginx validation
  passes with the same four pre-existing protocol warnings.
- The owner selected Google Drive instead of Google Cloud Storage. One private
  copy of release `414104c` and its vhost was uploaded, downloaded and restored:
  20 files, with the exact original SHA-256. This is evidence for that website
  snapshot, not a scheduled backup or complete host recovery. See
  `orgbots-recovery.md` for the recovery procedure and remaining automation gap.

Host-wide SSH restriction, a TLS-1.3-only edge, complete WAF coverage, recurring
Drive uploads/failure alerts, and qualified legal/transfer review remain open.
Shared-host SSH was not changed without a tested recovery path. Field Core Web
Vitals and search results require real traffic and time. The public PageSpeed
API returned HTTP 429 during this follow-up; no score is claimed from that call.
