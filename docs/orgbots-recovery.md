# OrgBots site backup and recovery

OrgBots is a static site. Its recoverable production state is the active site
release and the OrgBots Nginx virtual-host file. The TLS certificate can be
reissued with Certbot; private keys, host logs, other sites, databases, and
whole-host state are deliberately excluded.

## Initial recovery evidence

On 2026-10-26, Hermes pointed `/var/www/orgbots/current` to release
`releases/414104c`. Three release directories were present. The active vhost is
`/etc/nginx/sites-available/orgbots.theaecestudio.com`; the enabled entry is a
symlink to it. Nginx workers serve the files as `www-data`. No recurring
website backup job or configured `rclone` remote was found. This establishes
local release rollback history, not a scheduled backup service.

The public site responds with HTTP 200 and valid HTTPS. Forced TLS 1.2 and
TLS 1.3 handshakes both succeeded through Cloudflare, so a TLS-1.3-only edge
remains unconfigured. After the scoped origin restriction was installed on
2026-10-26, direct HTTP/HTTPS origin requests return 403. Public pages and a
real HTTP ACME challenge fixture still return 200. Nginx uses
`$realip_remote_addr` to check the original Cloudflare socket peer because
`$remote_addr` is rewritten to the visitor's IP. The checked origin ranges
match Cloudflare's published list. Other site vhosts were not changed.

Hermes also allows SSH/22 from Anywhere over IPv4 and IPv6. Effective `sshd`
configuration permits root login and password authentication; the configured
key works in BatchMode, and the Fail2ban SSH jail is active. No Tailscale,
WireGuard, or OpenVPN administrative path was found. Do not change SSH or
firewall policy until a second owner-controlled SSH session and provider
console recovery are verified.

## Snapshot format

`deploy/website/orgbots_snapshot.py` creates one gzip tar archive containing:

- every regular, non-hidden file in one selected site release;
- the selected `orgbots.theaecestudio.com` Nginx vhost file;
- a manifest with path, size, format version, creation time, and SHA-256 for
every included file.

It rejects hidden files, symlinks, non-regular files, oversized archives, and
existing output paths. It excludes Nginx logs, certificates/private keys,
secrets, SSH files, other sites, and server state. A restore check verifies
every entry before writing to a new destination and refuses to overwrite an
existing path. It does not activate the restored vhost or change the live
release link.

A read-only copy of active release `414104c` and its live vhost was snapshotted
and restored into isolated local temporary directories on 2026-10-26. The
archive contains 20 files, is 323,564 bytes, and has SHA-256
`17fd6bdbb40132621471be9f5ed49a752f72bb5643c353a9106419cca193ffb8`. The
temporary artifact was `/tmp/orgbots-live-snapshot-20261026/orgbots-414104c-verified.tar.gz`
(mode 0600, owned by `theaece`); it is not committed to this repository. The
owner selected Google Drive for the independent copy. A copy was uploaded to
an owner-only Drive folder on 2026-10-26. The remote copy was subsequently
downloaded and verified: its SHA-256 matches the local archive, and the
manifest restored all 20 files into `/tmp/orgbots-drive-restore-414104c`.
This verifies this archive's independent copy and restore path; it does not
establish scheduled backups or full production recovery.

Example after arranging a new local staging directory and resolving the active
release path with read-only SSH:

```sh
python3 deploy/website/orgbots_snapshot.py snapshot \
  --site-root /path/to/staged/live-release \
  --nginx-config /path/to/staged/orgbots.theaecestudio.com \
  --output /path/to/private-backup/orgbots-2026-10-26.tar.gz

python3 deploy/website/orgbots_snapshot.py restore-check \
  --archive /path/to/private-backup/orgbots-2026-10-26.tar.gz \
  --destination /path/to/isolated-restore
```

The restore directory contains `site/` and `hosting/orgbots.nginx.conf`. Check
the extracted configuration against the repository copy before an authorized
production recovery. Use `nginx -t` before any reload. Keep the current release
and its previous link target until the restored site passes acceptance.

## Google Drive backup status and operating procedure

Google Drive is the owner-selected backup destination. The owner created a
private Drive folder with owner-only access and uploaded the verified snapshot
archive. The verified archive SHA-256 is
`17fd6bdbb40132621471be9f5ed49a752f72bb5643c353a9106419cca193ffb8`.
The folder and archive identifiers are intentionally kept out of this
repository; use the owner's Drive link/record.

No recurring upload or failure alert is configured. The inspected workstation
has no `rclone`, Drive mount, or backup timer; Hermes has `rclone` but no
configured remote. Do not add a scheduler until an approved Drive automation
identity and secure credential mechanism exist, and an owner-controlled alert
destination is identified. No OAuth client, new account, paid service, or
server-side Drive credential was created. Credentials must not be stored in
this repository or included in the snapshot.

Until recurring automation is separately configured, create and upload a
publish-time recovery point after each accepted release:

1. Snapshot the exact immutable release and matching active OrgBots vhost with
   the commands above; use a new timestamped archive name and a private local
   staging directory.
2. Upload that archive to the owner's private Google Drive folder. Keep
   sharing restricted to the owner; do not create public or link-accessible
   permissions.
3. Read the Drive object back, download it, compare its SHA-256 with the local
   file, and run `restore-check` into a new isolated path. Record the archive
   hash, release identifier, Drive verification result, operator, and date.
4. Retain the previous known-good Drive recovery point until the new download
   and restore check pass. Retention and deletion policy remain owner decisions.

The helper creates and verifies local archives only. It does not upload to
Drive, schedule jobs, alert on failures, or manage Drive retention. Recovery
point and recovery time objectives are not yet measured.

## Recovery sequence

1. Download the selected archive from the owner's private Google Drive folder.
   Compare its SHA-256 with the recorded value and verify its archive manifest.
2. Run `restore-check` into a new isolated directory. Confirm the site tree and
   vhost are complete; do not restore logs, credentials, certificates, or
   another site.
3. Stage the site as a new immutable release. Compare the vhost with the
   repository version, confirm the certificate path, and run `nginx -t`.
4. Reissue the OrgBots certificate through the existing Certbot webroot flow
   and current Cloudflare-proxied DNS. The snapshot intentionally has no
   private certificate material.
5. After explicit production-recovery authorization, switch the `current`
   symlink atomically and install/reload the reviewed vhost if needed. Verify
   Cloudflare HTTPS, HTTP redirect, a real ACME challenge path, the custom 404,
   and direct-origin denial once that protection is implemented.
6. Keep the previous release and record the recovered object, manifest hash,
   operator, time, and verification results. Roll back by restoring the prior
   `current` symlink if acceptance fails.

The initial Drive upload has passed readback, hash comparison, and isolated
restore verification. No recurring schedule, failure notice, RPO/RTO
measurement, or full disaster recovery test has been performed. The archive
verifier passed synthetic fixture tests and this archive's independent restore;
this does not establish full production recoverability.

## Previous release backup

Release `ecc3628` and its active vhost were copied from the deployed host after
manifest verification, then uploaded to the same private owner-only Drive folder
on 26 October 2026. The downloaded archive is 327,581 bytes with SHA-256
`165c7fac6dce6733e6247e7856df2e2ac40e4eff3081eb51948b183d1e197a6c`.
All 20 files passed manifest checks and restored into
`/tmp/orgbots-drive-restore-ecc3628`. This backup includes the origin peer
restriction; the older `414104c` archive is retained for historical rollback.
No recurring upload, automatic retention deletion, or failure-alert job was enabled.

## Previous 2.0.1 website backup

Website release `13622b2` was activated on 26 October 2026 after the signed
OrgBots 2.0.1 archive became available. Every immutable release file was checked
against the local manifest; the active Nginx configuration was read back and
matched the reviewed source. The previous release and vhost remain available
for rollback. Home/install/robots/sitemap responses match source, the custom
404 works, and direct-origin access remains denied.

The matching snapshot was uploaded to the existing private owner-only Google
Drive folder. Readback reports no sharing. Its independent download is 328,181
bytes with SHA-256
`198b044ce869fdfe1f1d53fcf5090646a82f6ad029f40a335d25c072c36fd106`.
The downloaded hash matches the local archive, and all 20 files restored into
`/tmp/orgbots-2.0.1-release-z2j4jja7/drive-restore-13622b2`.
Earlier recovery points were retained. This is a verified publish-time recovery
point; recurring backup scheduling and a full production disaster-recovery test
remain unconfigured/unperformed.


## Previous 2.0.2 proof-page release backup

On 27 October 2026, release `06153e4` was activated with the OrgBots 2.0.2
proof and usage table. All 19 public site files match the committed source,
and the active vhost matches the reviewed CSP update. Previous release
`13622b2` and its matching vhost remain available for rollback. HTTP redirect,
HTTPS routes, custom 404 and direct-origin denial were verified.

The exact deployed release and active vhost were snapshotted and uploaded to
the existing owner-only Google Drive folder. Readback confirms no sharing
and one owner. The independently downloaded archive is 329,433 bytes, with
SHA-256 `e684673b7b0992e917d49d9b7b35f8177588463d90456ab0b3bc2afe727d9f30`.
Its bytes match the local archive, and all 20 manifest entries restored into
a new isolated directory. The operator was the owner's Codex session.
Earlier snapshots remain retained. Recurring uploads, failure alerts and a
full host disaster-recovery exercise remain unconfigured/unperformed.


## Current 2.0.3 Gemini-proof release backup

On 27 October 2026, release `f1931cf` replaced `06153e4` after the signed
2.0.3 packages passed independent public verification. All 19 public files
and the active vhost match committed source. The previous release and its
vhost remain retained. HTTP redirect, custom 404, security headers and
HTTPS direct-origin denial passed.

The exact deployed files and vhost were uploaded to the existing owner-only
Drive folder. Metadata readback confirms no sharing and one owner. The
independently downloaded archive is 330,706 bytes with SHA-256
`62bd3e18d1739a0bb7bff70bd6ccede6061ef700d1c5322b77e7687697f6fcf6`.
It matches the local archive, and all 20 manifest entries restored into a new
isolated directory. Earlier recovery points remain. Recurring uploads and
failure alerts remain unconfigured; no full-machine recovery is claimed.
