#!/usr/bin/env bash
set -euo pipefail

RUNNER_DIR=$(cd "$(dirname "$0")" && pwd)
BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/orgbot-browser-lifecycle-XXXXXX")
trap 'rm -rf "$BUILD_DIR"' EXIT

read -r -a CFLAGS <<<"$(pkg-config --cflags gtk+-3.0 webkit2gtk-4.1)"
read -r -a LIBS <<<"$(pkg-config --libs gtk+-3.0 webkit2gtk-4.1)"
${CXX:-c++} -std=c++17 "${CFLAGS[@]}" \
  "$RUNNER_DIR/native_browser_lifecycle_test.cc" \
  "${LIBS[@]}" -o "$BUILD_DIR/native-browser-lifecycle-test"
"$BUILD_DIR/native-browser-lifecycle-test"
