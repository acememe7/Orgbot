# OrgBots website hosting

The public site is `https://orgbots.theaecestudio.com/`. Its source is
`website/`; there is no build step. The signed APT archive stays at
`https://acememe7.github.io/Orgbot/apt/`. Changing the product site must not
change the GitHub Pages archive domain or its `apt` branch.

## Host configuration

`deploy/website/nginx-orgbots.https.conf` is the installed Nginx virtual host.
It redirects HTTP to HTTPS, serves a real custom 404 and sets security headers.
Scripts and styles come from this origin. Optional analytics remains disabled. The two inline JSON-LD blocks have
explicit CSP SHA-256 hashes, checked by `scripts/check-website.py`; update those
hashes when changing structured data. The bootstrap configuration is retained
only for rebuilding the ACME webroot setup.

Cloudflare has a proxied DNS record and an active configuration rule named
`OrgBots strict origin HTTPS`, matching only
`http.host eq "orgbots.theaecestudio.com"` with SSL set to Strict. This rule is
required: inheriting the zone's other TLS mode caused a redirect loop during
initial setup. Other domains and rules were not changed.

The origin uses a separate Let's Encrypt certificate issued on 26 October 2026,
expiring 24 January 2027, with Certbot's existing scheduled renewal. HTTP-01
uses `/var/www/letsencrypt`. The public document root is
`/var/www/orgbots/current`, pointing to a release beneath
`/var/www/orgbots/releases/`.

## Publish an update

1. Edit the static pages and any installation/evidence documents. Run
   `python3 scripts/test-install-orgbots.py`, `python3 scripts/check-website.py`,
   `node --check website/site.js` and `bash -n scripts/install-orgbots.sh` as a
   normal user. Check responsive layouts and changed interactions in a browser.
2. Commit and push reviewed source to GitHub. Copy `website/` to a new release
   directory on the owner-controlled host using `rsync -a --chmod=D755,F644`.
   Keep the previous release. Compare a SHA-256 manifest before activation.
3. Create a new symlink beside `current`, then atomically rename that symlink
   over `current`. Static-content updates need no Nginx reload. Configuration
   changes require a preserved previous config, successful `nginx -t`, and a
   reload. Do not run site builds or tests as root.
4. Check HTTPS, HTTP redirection, `/install/`, `/results/`, `/privacy/`,
   `/terms/`, a nonexistent URL, assets and browser errors. If reusing asset
   names, publish a new URL version or clear only this site's cached assets;
   Cloudflare may extend the origin's asset cache lifetime.

Rollback changes only `current` to the retained prior release through another
atomic symlink switch. No release pruning is part of publishing. Keep backups
and deployment manifests outside the public document root.

## Verification boundaries

Nginx serves static files through its existing non-root worker. Dedicated
access/error logs use the existing daily rotation policy with 14 retained
rotations. These statements describe the checked host configuration, not a
complete security or legal audit.

The website has no account database, forms, active analytics or provider
credentials. Cloudflare may process security cookies and network error reports.
Search Console ownership was verified during initial deployment, when its
sitemap reported five discovered pages. The current five-URL sitemap passes
local and public checks; Search Console indexing was not refreshed for this
content release. A private Google Drive snapshot has passed a download and
isolated restore check; the owner explicitly selected Drive instead of Cloud Storage.
See `orgbots-recovery.md` for its scope and the pending recurring-upload setup.
See `WEBSITE_PRIVACY_ASSESSMENT.md` for the legal assessment and analytics gate.

## Origin restriction and analytics gate

The HTTPS vhost defines an OrgBots-specific `geo` using `$realip_remote_addr`:
Nginx's shared real-IP module changes `$remote_addr` to the visitor's IP. Only
Cloudflare peers can use the OrgBots HTTP/HTTPS servers; other vhosts are not
changed. The listed ranges were checked against Cloudflare on 26 October 2026.
Refresh them against `https://www.cloudflare.com/ips/` when Cloudflare changes
its origin ranges. Direct requests now return 403; proxied pages and an actual
HTTP ACME fixture returned 200. This preserves the certificate renewal path.

`Cache-Control: public, no-transform` prevents the parent zone's automatic
analytics injection. The CSP additionally denies external scripts and analytics
connections. HTML keeps `data-analytics-enabled="false"`; the prepared consent
code is not collecting data. Enabling it requires the legal determination,
notices, CSP, and real consent checks described in `WEBSITE_DELIVERY.md`.

The shared SSH exposure and TLS 1.2 support remain pre-existing open items;
this scoped change is not a complete shared-server security certification.


Current release: `3bd0a91`, activated on 10 November 2026: every page names
OrgBots 2.2.2 as the current package, the install note links the 2.2.2 release
record, the Evidence page links it beside the 2.2.1 record, and the build log
adds the 2.2.2 entry. The vhost's CSP carries the home and build pages' new
JSON-LD hashes; the previous vhost is kept in `/root` and `nginx -t` passed
before the reload. The 28 uploaded files matched the source manifest, the live
home, install, results and build pages matched source, every public page
returned 200, a missing page returned 404, HTTP redirected to HTTPS, and the
pages showed no overflow or console errors at 375 pixels. A snapshot of the
release and vhost passed a restore check. Earlier: `71db3a8`, activated on 1 November 2026: the Evidence page
adds a 2.2.1 section and links the 2.2.1 release record, and the install page's
release note lines up with the page (its margin had cancelled the centring);
the live pages matched source. Earlier: `58cfbd7`, the same day: the site names
OrgBots 2.2.1 as the current release; the live home, install, results and log
pages matched source and the vhost CSP carries the home page's new JSON-LD
hash. Earlier: `61e0c90`, activated the same day with the 31 October
daily log (`/evidence/2026-10-31/`). The previous `ce3c7ce` release and its
vhost are retained for rollback. Static checks for 9 pages passed, the new page
and the log index showed no overflow or console errors at 375 and 1440 pixels,
the live pages, index and sitemap matched source, and the vhost CSP carries both
new JSON-LD hashes. Earlier: `ce3c7ce`, activated on 30 October 2026 with the
daily log of the live room (`/evidence/`). Before that: `6b6ee9d`, activated on 29 October 2026 for
OrgBots 2.2.0 and the reviewed-room evidence. Public file/configuration identity, responsive browser
checks and a snapshot restore check passed; see
[the release record](RELEASE_2_2_0_VERIFICATION.md).
