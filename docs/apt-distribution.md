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
  app/orgbots_2.0.0_amd64.deb \
  app/orgbot_2.0.0_all.deb
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
an isolated APT metadata refresh before deploying. It copies only the `apt/`
tree into the published artifact. It has no automatic push trigger and holds no
signing secret; signing happens before the branch update.

To prepare a publication directory, invoke the builder with a fresh destination
then place that directory under `apt/` on the dedicated branch. Keep the
archive key private material in its separate signing environment; only the
exported public keyring belongs in `apt/`.

## One-time client setup

The signed archive is published at
`https://acememe7.github.io/Orgbot/apt/`. GitHub Pages uses the manually
dispatched Actions workflow and the dedicated `apt` branch.

Compare the downloaded key with the fingerprint above, install its scoped
keyring, and create the Deb822 source once:

```sh
curl -fsSLo /tmp/orgbots-archive-keyring.gpg \
  https://acememe7.github.io/Orgbot/apt/orgbots-archive-keyring.gpg
gpg --show-keys --with-fingerprint /tmp/orgbots-archive-keyring.gpg
# Expected fingerprint: 81445722C3B9265312C12039F831B11611E6D198
sudo install -D -m 0644 /tmp/orgbots-archive-keyring.gpg \
  /etc/apt/keyrings/orgbots-archive-keyring.gpg
sudo tee /etc/apt/sources.list.d/orgbots.sources >/dev/null <<'EOF'
Types: deb
URIs: https://acememe7.github.io/Orgbot/apt
Suites: stable
Components: main
Architectures: amd64
Signed-By: /etc/apt/keyrings/orgbots-archive-keyring.gpg
EOF
sudo apt update
sudo apt install orgbots
```

APT verifies repository metadata with only the key named by `Signed-By`; the
package files are covered by hashes in that signed metadata. Do not use
`apt-key`, `trusted=yes`, or an unscoped global keyring for this archive.

## Published release and maintenance

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

Protected replacement/removal also requires the owner's writable ext4 HDD
mounted at `/mnt/hdd`; originals are retained in `/mnt/hdd/OrgBots Trash`.
If that mount is absent, integration stops rather than discarding originals.
Packaging does not configure disks or enable an automatic cleanup timer on
other people's computers. See [workspace safety](WORKSPACE_SAFETY.md).
