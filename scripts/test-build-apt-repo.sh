#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

make_key() {
  local home=$1 name=$2
  mkdir -m700 "$home"
  cat > "$TMP/key-$name.conf" <<EOF
Key-Type: RSA
Key-Length: 2048
Name-Real: $name test
Name-Email: $name@example.invalid
Expire-Date: 0
%no-protection
%commit
EOF
  gpg --homedir "$home" --batch --generate-key "$TMP/key-$name.conf" >/dev/null 2>&1
  gpg --homedir "$home" --batch --with-colons --list-secret-keys \
    2>/dev/null | awk -F: '$1 == "fpr" { print $10; exit }'
}

mkdir -p "$TMP/package/DEBIAN" "$TMP/package/usr/share/orgbots" "$TMP/lists/partial"
cat > "$TMP/package/DEBIAN/control" <<'CONTROL'
Package: orgbots
Version: 2.0.0
Architecture: amd64
Breaks: orgbot (<< 2.0.0)
Replaces: orgbot (<< 2.0.0)
Section: misc
Priority: optional
Maintainer: Test <test@example.invalid>
Description: synthetic package for isolated apt repository checks
CONTROL
printf 'synthetic install payload\n' > "$TMP/package/usr/share/orgbots/installed"
mkdir -p "$TMP/transitional/DEBIAN"
cat > "$TMP/transitional/DEBIAN/control" <<'CONTROL'
Package: orgbot
Version: 2.0.0
Architecture: all
Depends: orgbots (= 2.0.0)
Section: oldlibs
Priority: optional
Maintainer: Test <test@example.invalid>
Description: transitional package for apt rename acceptance
CONTROL
dpkg-deb --build --root-owner-group "$TMP/transitional" "$TMP/orgbot_2.0.0_all.deb" >/dev/null
mkdir -p "$TMP/old/DEBIAN" "$TMP/old/usr/lib/orgbot"
cat > "$TMP/old/DEBIAN/control" <<'CONTROL'
Package: orgbot
Version: 1.8.6
Architecture: amd64
Section: misc
Priority: optional
Maintainer: Test <test@example.invalid>
Description: synthetic prior orgbot package
CONTROL
printf 'prior package file\n' > "$TMP/old/usr/lib/orgbot/prior"
dpkg-deb --build --root-owner-group "$TMP/old" "$TMP/orgbot_1.8.6_amd64.deb" >/dev/null
mkdir -p "$TMP/package/usr/lib/orgbot" "$TMP/package/usr/bin"
printf 'new package file\n' > "$TMP/package/usr/lib/orgbot/current"
ln -s /usr/lib/orgbot/current "$TMP/package/usr/bin/orgbot"
ln -s /usr/lib/orgbot/current "$TMP/package/usr/bin/orgbots"
dpkg-deb --build --root-owner-group "$TMP/package" "$TMP/orgbots_2.0.0_amd64.deb" >/dev/null

KEY_HOME="$TMP/signing-key"
WRONG_HOME="$TMP/wrong-key"
KEY=$(make_key "$KEY_HOME" orgbots)
WRONG_KEY=$(make_key "$WRONG_HOME" wrong)
REPO="$TMP/repo"
if "$ROOT/scripts/build-apt-repo.sh" --output "$TMP/incomplete" --gnupghome "$KEY_HOME" \
  --key "$KEY" "$TMP/orgbots_2.0.0_amd64.deb" >"$TMP/incomplete.out" 2>&1; then
  echo "repository generator accepted orgbots without its transitional package" >&2
  exit 1
fi
rg -q 'missing orgbot transitional package' "$TMP/incomplete.out"
test ! -e "$TMP/incomplete"
"$ROOT/scripts/build-apt-repo.sh" --output "$REPO" --gnupghome "$KEY_HOME" \
  --key "$KEY" "$TMP/orgbots_2.0.0_amd64.deb" "$TMP/orgbot_2.0.0_all.deb" >/dev/null
rg -Fqx 'Architectures: amd64' "$REPO/dists/stable/Release"
rg -Fqx 'Components: main' "$REPO/dists/stable/Release"
rg -q '^Valid-Until: ' "$REPO/dists/stable/Release"
gpg --homedir "$KEY_HOME" --verify "$REPO/dists/stable/Release.gpg" \
  "$REPO/dists/stable/Release" >/dev/null 2>&1

apt_state() {
  local name=$1 repository=$2 keyring=$3
  local state="$TMP/state-$name"
  mkdir -p "$state/lists/partial" "$state/cache/archives/partial"
  printf 'deb [check-valid-until=yes signed-by=%s] %s stable main\n' "$keyring" "$repository" > "$state/sources.list"
  printf '%s' "$state"
}

apt_args() {
  local state=$1
  printf '%s\0' -o Debug::NoLocking=1 -o Acquire::Check-Valid-Until=true \
    -o "Dir::Etc::sourcelist=$state/sources.list" -o Dir::Etc::sourceparts=- \
    -o "Dir::State::lists=$state/lists" -o "Dir::State::status=$state/status" \
    -o "Dir::Cache::archives=$state/cache/archives" \
    -o "Dir::Cache::pkgcache=$state/pkgcache.bin" -o "Dir::Cache::srcpkgcache=$state/srcpkgcache.bin" \
    -o APT::Architecture=amd64 \
    -o APT::Get::List-Cleanup=0
}

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$TMP" >"$TMP/http.log" 2>&1 &
HTTP_PID=$!
trap 'kill "$HTTP_PID" 2>/dev/null || true; rm -rf "$TMP"' EXIT
for _ in {1..40}; do
  python3 -c "from urllib.request import urlopen; urlopen('http://127.0.0.1:$PORT/repo/dists/stable/InRelease', timeout=1)" 2>/dev/null && break
  sleep 0.1
done

STATE=$(apt_state good "http://127.0.0.1:$PORT/repo" "$REPO/orgbots-archive-keyring.gpg")
mapfile -d '' -t ARGS < <(apt_args "$STATE")
apt-get "${ARGS[@]}" -qq update
mkdir "$TMP/download"
(cd "$TMP/download" && apt-get "${ARGS[@]}" -qq download orgbots)
test -s "$TMP/download/orgbots_2.0.0_amd64.deb"
mkdir -p "$TMP/install-root/home/test/.local/share/orgbot"
printf 'user state\n' > "$TMP/install-root/home/test/.local/share/orgbot/state"
dpkg --force-not-root --root="$TMP/install-root" --log="$TMP/dpkg.log" \
  --install "$TMP/orgbot_1.8.6_amd64.deb" >/dev/null
test -s "$TMP/install-root/usr/lib/orgbot/prior"
STATE=$(apt_state transition "http://127.0.0.1:$PORT/repo" "$REPO/orgbots-archive-keyring.gpg")
mapfile -d '' -t ARGS < <(apt_args "$STATE")
ARGS+=( -o "Dir::State::status=$TMP/install-root/var/lib/dpkg/status")
apt-get "${ARGS[@]}" -qq update
apt-get "${ARGS[@]}" --simulate install orgbot > "$TMP/transition.out"
rg -q 'orgbot.*2\.0\.0' "$TMP/transition.out"
rg -q 'orgbots.*2\.0\.0' "$TMP/transition.out"
dpkg --force-not-root --root="$TMP/install-root" --log="$TMP/apt-dpkg.log" \
  --install "$TMP/orgbot_2.0.0_all.deb" "$TMP/download/orgbots_2.0.0_amd64.deb" >/dev/null
test ! -e "$TMP/install-root/usr/lib/orgbot/prior"
test -s "$TMP/install-root/usr/lib/orgbot/current"
test -s "$TMP/install-root/home/test/.local/share/orgbot/state"

gpg --homedir "$WRONG_HOME" --batch --export "$WRONG_KEY" > "$TMP/wrong-keyring.gpg"
STATE=$(apt_state untrusted "http://127.0.0.1:$PORT/repo" "$TMP/wrong-keyring.gpg")
mapfile -d '' -t ARGS < <(apt_args "$STATE")
if apt-get "${ARGS[@]}" update >"$TMP/untrusted.out" 2>&1; then
  echo "APT accepted a repository signed by an untrusted key" >&2
  exit 1
fi
rg -q 'NO_PUBKEY|not signed|The following signatures couldn.t be verified' "$TMP/untrusted.out"

cp -a "$REPO" "$TMP/tampered"
printf 'tamper' >> "$TMP/tampered/dists/stable/main/binary-amd64/Packages.gz"
STATE=$(apt_state tampered "http://127.0.0.1:$PORT/tampered" "$REPO/orgbots-archive-keyring.gpg")
mapfile -d '' -t ARGS < <(apt_args "$STATE")
if apt-get "${ARGS[@]}" update >"$TMP/tampered.out" 2>&1; then
  echo "APT accepted a tampered package index" >&2
  exit 1
fi
rg -qi 'Hash Sum mismatch|unexpected size|Failed to fetch' "$TMP/tampered.out"

cp -a "$REPO" "$TMP/stale"
OLD_DATE=$(date -u -R -d '10 days ago')
OLD_UNTIL=$(date -u -R -d '9 days ago')
sed -i -e "s/^Date:.*/Date: $OLD_DATE/" -e "s/^Valid-Until:.*/Valid-Until: $OLD_UNTIL/" "$TMP/stale/dists/stable/Release"
gpg --homedir "$KEY_HOME" --batch --yes --local-user "$KEY" --clearsign \
  --output "$TMP/stale/dists/stable/InRelease" "$TMP/stale/dists/stable/Release"
gpg --homedir "$KEY_HOME" --batch --yes --local-user "$KEY" --armor --detach-sign \
  --output "$TMP/stale/dists/stable/Release.gpg" "$TMP/stale/dists/stable/Release"
STATE=$(apt_state stale "http://127.0.0.1:$PORT/stale" "$REPO/orgbots-archive-keyring.gpg")
mapfile -d '' -t ARGS < <(apt_args "$STATE")
if apt-get "${ARGS[@]}" update >"$TMP/stale.out" 2>&1; then
  echo "APT accepted an expired Release file" >&2
  exit 1
fi
rg -qi 'expired|invalid since' "$TMP/stale.out"

if "$ROOT/scripts/build-apt-repo.sh" --output "$REPO" --gnupghome "$KEY_HOME" \
  --key "$KEY" "$TMP/orgbots_2.0.0_amd64.deb" >"$TMP/existing.out" 2>&1; then
  echo "repository generator overwrote an existing output" >&2
  exit 1
fi
rg -q 'output already exists' "$TMP/existing.out"
echo "APT repository signature, freshness, tamper, download, and isolated install checks passed"
