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

The intended static host is the project GitHub Pages site at
`https://acememe7.github.io/Orgbot/apt/`. Read-only checks on 2026-10-26 found
that path returns 404 and the GitHub Pages API reports no configured site. Do
not give these commands to users until Pages serves the generated archive and
the archive key fingerprint is published through a separately trusted
channel.

After publication, users compare the downloaded key's fingerprint with the
trusted fingerprint, install the keyring, and create the Deb822 source:

```sh
curl -fsSLo /tmp/orgbots-archive-keyring.gpg \
  https://acememe7.github.io/Orgbot/apt/orgbots-archive-keyring.gpg
gpg --show-keys --with-fingerprint /tmp/orgbots-archive-keyring.gpg
# Confirm the full fingerprint through the separately trusted release channel.
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

## Hosting gate

GitHub Pages can serve this static archive, but its project site must first be
enabled and configured to publish the `apt/` subtree or a generated Pages
artifact. The repository must also retain old package files while publishing
new indices, and the owner must provide the dedicated signing key through a
secure signing environment. No live Pages settings, GitHub workflow, or
publication is configured by this preparation.
