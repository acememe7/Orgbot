# OrgBots site backup and recovery

OrgBots is a static site. Its recoverable production state is the active site
release and the OrgBots Nginx virtual-host file. The TLS certificate can be
reissued with Certbot; private keys, host logs, other sites, databases, and
whole-host state are deliberately excluded.

## Current recovery evidence

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
