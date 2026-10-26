# Linux APT distribution

The Linux desktop packages are `orgbots` (amd64) and `orgbot` (an `all`
transitional package), versioned from `app/packaging/version`. `orgbots` keeps
the existing `/usr/lib/orgbot` files, `/usr/bin/orgbot` command, and
`orgbot.desktop` launcher, and also provides the `orgbots` command. The
transitional package depends on `orgbots`, so apt upgrades existing `orgbot`
installs without moving their files or user data.

The current binary targets **Ubuntu 24.04 amd64**. The archive's signing-key
fingerprint is `81445722C3B9265312C12039F831B11611E6D198` (expires 2028-10-25).
Only the public key is published; private signing material stays on the owner's
machine outside the source tree.

## Build the signed archive

Build `app/orgbots_<version>_amd64.deb` and
`app/orgbot_<version>_all.deb`, then use a dedicated GNUPG home that contains
the archive signing key. The key fingerprint is required explicitly; the
private key must stay outside this repository and the generated archive.

```sh
scripts/build-apt-repo.sh \
  --output /tmp/orgbots-apt \
  --gnupghome /secure/path/orgbots-archive-key \
  --key FULL_40_OR_64_CHARACTER_FINGERPRINT \
  app/orgbots_2.0.1_amd64.deb \
  app/orgbot_2.0.1_all.deb
```

The output path must not already exist. The script emits both packages under
`pool/`, compressed and uncompressed amd64 `Packages` indices, `Release`,
`InRelease`, detached `Release.gpg`, and the public
`orgbots-archive-keyring.gpg`. `Release` expires after 90 days. Refresh the signed archive with each release
and at least every 60 days so clients retain a freshness check without a
weekly dependency on the signing machine. The signing key stays off GitHub. The repository uses the `stable` suite and
`main` component.

Each invocation creates a complete archive snapshot. Pass every `orgbots`
package version and matching `orgbot` transitional package you intend to keep
available; the generator does not merge an existing output directory.

## GitHub Pages publishing layout

The manual-only workflow `.github/workflows/publish-apt-pages.yml` reads an
`apt/` directory from a dedicated `apt` branch and deploys it to Pages. Put the
generated repository contents at the root of that directory, so
`apt/dists/stable/InRelease` and `apt/pool/` exist in the branch. Before using
the workflow, an owner with repository administration must enable Pages with
GitHub Actions as its source. The workflow verifies both signatures and runs
an isolated APT metadata refresh before deploying. It preserves the `apt/`
tree and adds a root page forwarding visitors to `orgbots.theaecestudio.com`.
The forwarder source is taken from the workflow's exact source revision.
It has no automatic push trigger and holds no
signing secret; signing happens before the branch update.

To prepare a publication directory, invoke the builder with a fresh destination
then place that directory under `apt/` on the dedicated branch. Keep the
archive key private material in its separate signing environment; only the
exported public keyring belongs in `apt/`.

## One-time client setup

The signed archive is published at
`https://acememe7.github.io/Orgbot/apt/`. GitHub Pages uses the manually
dispatched Actions workflow and the dedicated `apt` branch.

The installer supports Ubuntu 24.04 amd64. Review the
[installer source](../scripts/install-orgbots.sh), download it, then run it
with Bash:

```sh
curl -fsSLo install-orgbots.sh \
  https://raw.githubusercontent.com/acememe7/Orgbot/main/scripts/install-orgbots.sh
sed -n '1,220p' install-orgbots.sh
bash install-orgbots.sh
```

The script downloads the public key over HTTPS and verifies its full pinned
fingerprint before changing system files. It installs the key into
`/etc/apt/keyrings/` and writes a Deb822 source scoped with `Signed-By`. If an
existing key or source file differs, it preserves the original beside that
file before replacing it. Identical configuration is left untouched. It then
runs `apt-get update` and `apt-get install orgbots` with APT's normal
confirmation prompt. Missing prerequisites are reported; the script does not
install them automatically. APT verifies repository metadata only with the
scoped key, and the package hashes are covered by that signed metadata.

## Published release and maintenance

OrgBots **2.0.1** was published on 2026-10-26 with
[GitHub Actions run 40387218167](https://github.com/acememe7/Orgbot/actions/runs/40387218167).
A fresh isolated APT metadata refresh and download passed against the public
archive, without installing on the owner's machine. The downloaded
`orgbots_2.0.1_amd64.deb` SHA-256 matches the reviewed local package:

```text
ece4bc912a9847d0922cc5032d9aa7a4ed6e678540f6cf23db427d730ea512c5
```

This release adds configurable recovery storage and the worker/QA/reviewer
verification guidance. The 2.0.0 package files remain in the archive pool;
the package index selects the current version. The root Pages address forwards
to the product website while `/apt/` remains the package archive.


OrgBots **2.0.0** was published on 2026-10-26 with
[GitHub Actions run 40348136526](https://github.com/acememe7/Orgbot/actions/runs/40348136526).
The public repository passes an isolated APT metadata refresh and package
download using the scoped archive key; the downloaded package hash matches. The expected `orgbots_2.0.0_amd64.deb` SHA-256 is:

```text
7597e7729aabb629735fd71aa05a287a0e255b2c79c817d497560e468baa8da1
```

Keep existing package files when publishing later archive snapshots. Refresh
metadata at least every 60 days and rotate the signing key before its expiry;
expired metadata is intentionally rejected. Private signing material and its
revocation certificate stay outside GitHub. Publishing the archive does not
add an APT source or install/upgrade software on the owner's machine.

This is a signed third-party repository, not an entry in Ubuntu's built-in
archive. A new computer needs the one-time setup above before
`sudo apt install orgbots` can find the package.

From 2.0.1, recovery storage is configurable and defaults to local user storage
for new installations. Existing HDD selections are preserved; an ambiguous
upgrade from the older package requires explicit selection. Missing configured
storage stops integration instead of discarding originals. Packaging does not
mount disks or enable an automatic cleanup timer. See
[workspace safety](WORKSPACE_SAFETY.md).
