#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --output DIR --gnupghome DIR --key FINGERPRINT PACKAGE.deb [...]" >&2
  exit 2
}

OUTPUT=
GNUPGHOME_DIR=
SIGNING_KEY=
PACKAGES=()
while (($#)); do
  case "$1" in
    --output) (($# >= 2)) || usage; OUTPUT=$2; shift 2 ;;
    --gnupghome) (($# >= 2)) || usage; GNUPGHOME_DIR=$2; shift 2 ;;
    --key) (($# >= 2)) || usage; SIGNING_KEY=$2; shift 2 ;;
    --) shift; PACKAGES+=("$@"); break ;;
    -*) usage ;;
    *) PACKAGES+=("$1"); shift ;;
  esac
done

[[ -n "$OUTPUT" && -n "$GNUPGHOME_DIR" && -n "$SIGNING_KEY" && ${#PACKAGES[@]} -gt 0 ]] || usage
[[ "$SIGNING_KEY" =~ ^([A-Fa-f0-9]{40}|[A-Fa-f0-9]{64})$ ]] || {
  echo "--key must be a full OpenPGP fingerprint" >&2
  exit 2
}
[[ -d "$GNUPGHOME_DIR" ]] || { echo "missing GNUPG home: $GNUPGHOME_DIR" >&2; exit 1; }
[[ ! -e "$OUTPUT" ]] || { echo "output already exists; choose a new empty path: $OUTPUT" >&2; exit 1; }
gpg --homedir "$GNUPGHOME_DIR" --batch --list-secret-keys "$SIGNING_KEY" >/dev/null 2>&1 || {
  echo "signing secret key $SIGNING_KEY is unavailable in the selected GNUPG home" >&2
  exit 1
}

STAGE=$(mktemp -d "${OUTPUT}.tmp.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/pool/main/o/orgbot" "$STAGE/pool/main/o/orgbots" \
  "$STAGE/dists/stable/main/binary-amd64"
APP_VERSIONS=()
TRANSITION_VERSIONS=()
for package in "${PACKAGES[@]}"; do
  [[ -f "$package" && "$package" == *.deb ]] || { echo "not a .deb file: $package" >&2; exit 1; }
  name=$(dpkg-deb -f "$package" Package)
  [[ "$name" == orgbot || "$name" == orgbots ]] || {
    echo "expected an orgbot transitional or orgbots package: $package" >&2
    exit 1
  }
  version=$(dpkg-deb -f "$package" Version)
  [[ "$version" =~ ^[0-9][A-Za-z0-9.+:~-]*$ ]] || {
    echo "invalid Debian package version $version: $package" >&2
    exit 1
  }
  arch=$(dpkg-deb -f "$package" Architecture)
  if [[ ( "$name" == orgbots && "$arch" != amd64 ) || ( "$name" == orgbot && "$arch" != all ) ]]; then
    echo "unexpected architecture $arch for package $name: $package" >&2
    exit 1
  fi
  if [[ "$name" == orgbots ]]; then
    [[ ! " ${APP_VERSIONS[*]} " == *" $version "* ]] || {
      echo "duplicate orgbots version $version" >&2
      exit 1
    }
    APP_VERSIONS+=("$version")
  else
    depends=$(dpkg-deb -f "$package" Depends)
    [[ "$depends" == "orgbots (= $version)" ]] || {
      echo "orgbot transitional package must depend on orgbots (= $version): $package" >&2
      exit 1
    }
    [[ ! " ${TRANSITION_VERSIONS[*]} " == *" $version "* ]] || {
      echo "duplicate orgbot transitional version $version" >&2
      exit 1
    }
    TRANSITION_VERSIONS+=("$version")
  fi
  destination="$STAGE/pool/main/o/$name/$(basename "$package")"
  [[ ! -e "$destination" ]] || { echo "duplicate package filename: $package" >&2; exit 1; }
  install -m644 "$package" "$destination"
done
for version in "${APP_VERSIONS[@]}"; do
  [[ " ${TRANSITION_VERSIONS[*]} " == *" $version "* ]] || {
    echo "missing orgbot transitional package for orgbots $version" >&2
    exit 1
  }
done
for version in "${TRANSITION_VERSIONS[@]}"; do
  [[ " ${APP_VERSIONS[*]} " == *" $version "* ]] || {
    echo "missing orgbots package for transitional orgbot $version" >&2
    exit 1
  }
done

(cd "$STAGE" && dpkg-scanpackages --arch amd64 pool /dev/null > dists/stable/main/binary-amd64/Packages)
gzip -n -9 -c "$STAGE/dists/stable/main/binary-amd64/Packages" > "$STAGE/dists/stable/main/binary-amd64/Packages.gz"
(cd "$STAGE" && apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=OrgBots \
  -o APT::FTPArchive::Release::Label=OrgBots \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o APT::FTPArchive::Release::Architectures=amd64 \
  -o APT::FTPArchive::Release::Components=main \
  release dists/stable > Release.tmp)
mv "$STAGE/Release.tmp" "$STAGE/dists/stable/Release"
sed -i "/^Date:/a Valid-Until: $(date -u -R -d '+90 days')" "$STAGE/dists/stable/Release"
gpg --homedir "$GNUPGHOME_DIR" --batch --yes --local-user "$SIGNING_KEY" \
  --clearsign --output "$STAGE/dists/stable/InRelease" "$STAGE/dists/stable/Release"
gpg --homedir "$GNUPGHOME_DIR" --batch --yes --local-user "$SIGNING_KEY" \
  --armor --detach-sign --output "$STAGE/dists/stable/Release.gpg" "$STAGE/dists/stable/Release"
gpg --homedir "$GNUPGHOME_DIR" --batch --export "$SIGNING_KEY" > "$STAGE/orgbots-archive-keyring.gpg"
[[ -s "$STAGE/orgbots-archive-keyring.gpg" ]] || { echo "public key export was empty" >&2; exit 1; }
mv -T "$STAGE" "$OUTPUT"
trap - EXIT
echo "built signed OrgBots APT repository at $OUTPUT"
