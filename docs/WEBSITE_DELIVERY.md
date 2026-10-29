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

`website/results/index.html` is the public evidence page. Its current release figures
come from `docs/RELEASE_2_0_2_VERIFICATION.md`; the bounded live comparison and
usage reconciliation are in `docs/benchmarks/2026-10-27-release-validation.md`.
Keep those live records distinct from the screenshots, which use synthetic
fixture data.

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

At that 26 October checkpoint, paid provider comparisons, physical suspend/reboot
and real multi-day runs remained unperformed. This task changed installation/distribution and the website,
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
API initially returned HTTP 429. The PageSpeed website subsequently completed
the audit described below.

## Published follow-up verification

Release `ecc3628e6e1ce89243fcff498df18f823564b75d` was pushed to both `main`
and `master` and deployed on 26 October 2026. Both GitHub Actions checks passed
(runs `40369939735` and `40369939013`). All 19 deployed website files matched
the release manifest; the public home HTML also matched the source byte for byte.

Browser checks at measured widths of 375, 768 and 1440 pixels found no document
overflow. Mobile menu/Escape, privacy close/reopen/focus return, and copying the
installation command passed. No site-origin console errors were captured;
unrelated extension warnings were excluded. The raw public HTML contains no
injected beacon. Browser network tracing was unavailable, so no complete network
capture is claimed. The disabled-source fixture and same-origin CSP provide
additional enforcement. A default Python urllib request was blocked by
Cloudflare error 1010; normal browser, curl, Search Console and Lighthouse
requests succeeded. The unchanged robots file remained in Cloudflare's cache.

[PageSpeed mobile report](https://pagespeed.web.dev/analysis/https-orgbots-theaecestudio-com/p98snw071s?form_factor=mobile)
and [desktop report](https://pagespeed.web.dev/analysis/https-orgbots-theaecestudio-com/p98snw071s?form_factor=desktop)
were captured on 26 October 2026 at 18:37 WAT using Lighthouse 13.5.0. Mobile
(emulated Moto G Power, slow 4G) scored 98 performance / 100 accessibility /
100 best practices / 100 SEO, with LCP 1.2 s, TBT 0 ms and CLS 0. Desktop scored
100 in all four categories, with LCP 0.3 s, TBT 0 ms and CLS 0. These are one-run
lab observations, not a guarantee or field Core Web Vitals result. Field data
was unavailable. Image sizing, cache lifetimes and render-blocking CSS remain
reported opportunities; no new framework or build pipeline was added for them.

The deployed release and active restricted vhost were archived to private
Google Drive, downloaded and restored into a new isolated folder: 20 files,
327,581 bytes, SHA-256
`165c7fac6dce6733e6247e7856df2e2ac40e4eff3081eb51948b183d1e197a6c`.
The earlier release backup remains retained. Automatic Drive scheduling is not
configured; the server has no authorized uploader identity or failure channel.


## 2.0.2 proof update, 27 October 2026

The evidence page now publishes the accepted trial-3 baseline/team comparison,
with each model's input, cache and output counts. Codex subscription usage is
separate from DeepSeek API estimates. Codex totals come from the final native
session records; DeepSeek totals come from API usage records. The all-model
team total and higher combined output are explicit. One pair does not establish
general savings or a subscription quota reduction.

Release counts are 854 Rust tests passed (3 ignored), 337 Flutter tests passed
(5 skipped), and 17 Linux acceptance checks passed. The page records the separate
three-episode native accounting reconciliation, lifecycle probe 6's 16 passed
gates, failed earlier trials/probes, and incomplete final-binary probe 7.
The source evidence links are pinned to `b48566d3d6ef5c7ac7b5143cbb5e37723933378b`.

All public version references now identify 2.0.2; inline structured-data CSP
hashes and asset URL versions match the updated source. Gallery images remain
labeled synthetic. No analytics, forms, provider calls or new data processing
were added. Static page, installer, consent-fixture, Bash and JavaScript checks
pass. Browser checks at measured CSS widths 375, 768 and 1440 found no document
overflow. The narrow table scrolls horizontally and accepts keyboard focus and
scrollbar dragging; desktop columns fit without scrolling. Native arrow-key
horizontal scrolling was not confirmed in the connected browser. Mobile menu/Escape,
gallery switching, privacy close/reopen with focus return, and command-copy
feedback passed. Only browser-extension warnings were captured. Astra cleared
the corrected source and desktop rendering for scoped deployment. Publication
and final backup evidence are recorded below after verification.


Published website source `06153e46f6598c389d349b4c95b179b21c72dc76` to
`main` and `master`, then activated release `06153e4` on the existing static
host. The previous `13622b2` release and vhost are retained. Nginx validation
and reload passed with the same four pre-existing shared-vhost warnings.
All 19 deployed files and the active vhost match the reviewed source. All
19 public responses match their source bytes, including the custom missing-URL
404; the other 18 return 200. HTTP redirects to HTTPS. Security headers and
`no-transform` are present, and direct HTTP/HTTPS origin requests remain 403.

Live browser checks measured 375, 768 and 1440 CSS pixels without document
overflow; narrow tables scroll to reveal the numeric columns. Mobile
menu/Escape and focus return passed. No site-origin warnings/errors or external
site scripts were observed; installed extension messages were excluded. The
analytics flag remains false. Full network tracing and a new Lighthouse or
field-performance run were not performed for this static content update.

The deployed site and matching active vhost were uploaded to the existing
owner-only Google Drive folder. Metadata confirms no sharing and one owner.
The downloaded archive matches the local SHA-256, and all 20 files restored
into a fresh isolated directory. Archive size: 329,433 bytes; SHA-256:
`e684673b7b0992e917d49d9b7b35f8177588463d90456ab0b3bc2afe727d9f30`.
Previous recovery points remain retained. This is a publish-time recovery
point, not recurring backup automation or a full host disaster-recovery test.

Both GitHub Actions website/installer checks passed for `06153e4`:
`40482523240` and `40482523258`. Staged website and documentation source
passed the secret scan with no findings.

Final Astra review approved the deployed page, responsive evidence, public
responses, backup verification and this documentation record. No blocking
findings remain within the scoped website proof update.


## 2.0.3 Gemini proof publication, 27 October 2026

Committed source `f1931cfb810176bb4262995737526ed5c37c4a2e` is on `main`
and `master`; immutable website release `f1931cf` is active. Version copy,
installation link and JSON-LD now identify 2.0.3. The results page includes
Gemini context sharing, resumed usage deltas, fresh-room isolation, separate
native write-denial and authorized MCP write probes. Astra is identified as
the context sender in that fixture. Historical 2.0.2 comparisons remain
explicitly historical; no general quota or billing savings are claimed.

Static checks, consent/focus tests and 375/768/1440 local renders passed.
Public checks matched all 19 file responses to source, including the genuine
404. The active vhost matches the reviewed CSP hashes; HTTP redirect,
security headers and direct-origin HTTPS denial passed. Nginx validation and
reload succeeded with four pre-existing shared-vhost protocol warnings.
Live Chrome checks at 375/1440 for home/results/install reported no document
overflow, console errors, exceptions, loading failures or CSP violations.
Keyboard focus was visible. A new Lighthouse or field-performance audit was
not run for this content update.

The new deployed snapshot passed private Drive upload, owner-only metadata
readback, independent download and isolated restore of all 20 entries.
See [recovery evidence](orgbots-recovery.md) and the
[2.0.3 release record](RELEASE_2_0_3_VERIFICATION.md). The previous release,
vhost and backups remain available. Analytics remains disabled; no accounts,
forms or new tracking were added. Recurring backups and full host recovery
remain outside this publication.

Final scoped Astra review approved the live site, release documentation and
backup evidence. GitHub website/installer checks passed for the published
source in runs 40532204564 and 40532203553.


## 2.1.0 live room evidence publication, 29 October 2026

Committed source `8a4227dcee77565cd4f5ce40c2e20f0fca8cccdc` is on `main`
and `master`; immutable website release `8a4227d` is active. The synthetic
gallery and figures are gone. The home and results pages now use cropped real
screenshots and the overnight live room record: 118 turns from 10 bots on 5
providers, 8 tasks delivered after review, the owner's account dashboards and
their limits. The terms page carries the PolyForm Noncommercial 1.0.0 license
and the Apache 2.0 note for 2.0.3 and earlier. Version copy, install links and
JSON-LD identify 2.1.0, and the vhost CSP carries the new JSON-LD hashes.

Static checks for 6 pages passed. Public checks matched all 18 file responses
to source, including the genuine 404. HTTP redirect, security headers and
direct-origin HTTP and HTTPS denial passed. Nginx validation and reload
succeeded with the four pre-existing shared-vhost warnings. Live browser checks
at 375 and 1440 pixels for home, results and install reported no document
overflow, broken images or console errors. GitHub website and installer checks
`40810982134` and `40810982314` passed. A new Lighthouse or field-performance
audit was not run for this content update. The analytics flag remains false.

## 2.2.0 evidence-led site, 29 October 2026

Committed source `6b6ee9d39190fc178258d028a7df5ff91e913b7f` is on `main` and
`master`; immutable website release `6b6ee9d` is active. The homepage now opens
with the benefit and its proof (one bot alone passed 3 of 10 hidden checks, a
reviewed room 10 of 10), adds a section on owner control, and trims long copy.
The results page leads with the experiment, adds 2.2.0 observations from the
live room with the room's own cleanup notice, and keeps the earlier records.
Titles, descriptions, social tags and structured data were updated, and the
vhost CSP carries the new JSON-LD hashes.

Static checks for 6 pages and the consent test passed. Public checks matched all
19 file responses to source, including the genuine 404. HTTP redirect, security
headers and direct-origin HTTP and HTTPS denial passed. Live browser checks at
375 and 1440 pixels for home, results and install found no overflow, broken
images or console errors. GitHub website checks `40911820438` and
`40911821485` passed. A new Lighthouse or field-performance audit was not run.
Analytics remains off.

