#!/usr/bin/env bash
set -Eeuo pipefail

readonly REPOSITORY='https://acememe7.github.io/Orgbot/apt'
readonly KEY_URL="$REPOSITORY/orgbots-archive-keyring.gpg"
readonly KEY_FINGERPRINT='81445722C3B9265312C12039F831B11611E6D198'
readonly KEY_DESTINATION='/etc/apt/keyrings/orgbots-archive-keyring.gpg'
readonly SOURCE_DESTINATION='/etc/apt/sources.list.d/orgbots.sources'

usage() {
  printf '%s\n' \
    'Install OrgBots from its signed Ubuntu 24.04 amd64 APT repository.' \
    '' \
    'Usage: install-orgbots.sh [--help]' \
    '' \
    'The script downloads and verifies the pinned archive key before changing APT' \
    'configuration. It preserves changed existing key/source files as adjacent' \
    '.orgbots-backup.* files, then refreshes APT and installs orgbots. APT keeps its' \
    'normal confirmation prompt. Run with Bash; sudo is used only for system changes.'
}

if (($#)); then
  if (($# == 1)) && [[ $1 == --help || $1 == -h ]]; then
    usage
    exit 0
  fi
  printf 'Unsupported argument: %s\n' "$1" >&2
  usage >&2
  exit 2
fi

fail() {
  printf 'install-orgbots: %s\n' "$1" >&2
  exit 1
}

read_os_field() {
  local field=$1
  sed -n "s/^${field}=//p" /etc/os-release | head -n 1 | tr -d '"'
}

[[ -r /etc/os-release ]] || fail 'cannot identify this operating system'
[[ $(read_os_field ID) == ubuntu && $(read_os_field VERSION_ID) == 24.04 ]] || \
  fail 'supported platform: Ubuntu 24.04 amd64 only'
command -v dpkg >/dev/null || fail 'dpkg is required; install it with your system administrator'
[[ $(dpkg --print-architecture) == amd64 ]] || fail 'supported architecture: amd64 only'
for executable in apt-get curl gpg cmp install ln mktemp date stat; do
  command -v "$executable" >/dev/null || \
    fail "missing prerequisite '$executable'; install Ubuntu packages apt, curl, gnupg, coreutils, and ca-certificates first"
done
[[ -r /etc/ssl/certs/ca-certificates.crt ]] || \
  fail 'CA certificates are missing; install the Ubuntu ca-certificates package first'
if ((EUID != 0)); then
  command -v sudo >/dev/null || fail 'sudo is required for system changes; rerun as root or install sudo'
fi

as_root() {
  if ((EUID == 0)); then
    "$@"
  else
    sudo -- "$@"
  fi
}

scratch=$(mktemp -d "${TMPDIR:-/tmp}/orgbots-apt-setup.XXXXXXXX") || fail 'could not create a private temporary directory'
trap 'rm -rf -- "$scratch"' EXIT HUP INT TERM
public_key="$scratch/orgbots-archive-keyring.gpg"
source_file="$scratch/orgbots.sources"
gpg_home="$scratch/gnupg"
mkdir -m 0700 "$gpg_home"

curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
  --output "$public_key" "$KEY_URL" || fail 'could not download the archive key over HTTPS'
[[ -s $public_key ]] || fail 'the downloaded archive key is empty'
key_info=$(GNUPGHOME="$gpg_home" gpg --batch --no-options --show-keys --with-colons --fingerprint "$public_key" 2>/dev/null | \
  awk -F: '$1 == "pub" { count++; primary = 1; next } $1 == "sub" { primary = 0; next } primary && $1 == "fpr" && !fingerprint { fingerprint = toupper($10); primary = 0 } END { print count ":" fingerprint }') || \
  fail 'could not inspect the downloaded archive key'
key_count=${key_info%%:*}
fingerprint=${key_info#*:}
[[ $key_count == 1 ]] || fail 'archive key must contain exactly one primary key'
[[ $fingerprint == "$KEY_FINGERPRINT" ]] || \
  fail "archive key fingerprint mismatch (expected $KEY_FINGERPRINT)"

cat >"$source_file" <<EOF_SOURCE
Types: deb
URIs: $REPOSITORY
Suites: stable
Components: main
Architectures: amd64
Signed-By: $KEY_DESTINATION
EOF_SOURCE

preflight_destination() {
  local destination=$1
  if [[ -L $destination ]]; then
    fail "refusing to replace symlink: $destination"
  fi
  if [[ -e $destination && ! -f $destination ]]; then
    fail "refusing to replace non-regular file: $destination"
  fi
}

preflight_destination "$KEY_DESTINATION"
preflight_destination "$SOURCE_DESTINATION"
as_root install -d -m 0755 /etc/apt/keyrings /etc/apt/sources.list.d

install_if_changed() {
  local source=$1 destination=$2 mode=$3 stamp backup index
  if [[ -f $destination ]] && as_root cmp -s -- "$source" "$destination" && \
    [[ $(as_root stat -c '%u:%g:%a' -- "$destination") == "0:0:${mode#0}" ]]; then
    printf 'unchanged: %s\n' "$destination"
    return 0
  fi
  if [[ -e $destination ]]; then
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    index=0
    while :; do
      backup="${destination}.orgbots-backup.${stamp}.${index}"
      if [[ ! -e $backup && ! -L $backup ]]; then
        as_root ln -- "$destination" "$backup" || fail "could not preserve existing file: $destination"
        printf 'preserved existing file: %s\n' "$backup"
        break
      fi
      index=$((index + 1))
    done
  fi
  as_root install -D -m "$mode" -- "$source" "$destination" || \
    fail "could not install $destination"
  printf 'installed: %s\n' "$destination"
}

install_if_changed "$public_key" "$KEY_DESTINATION" 0644
install_if_changed "$source_file" "$SOURCE_DESTINATION" 0644
as_root apt-get update
as_root apt-get install orgbots
