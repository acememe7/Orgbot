#!/usr/bin/env bash
set -euo pipefail

RUNNER_DIR=$(cd "$(dirname "$0")" && pwd)
BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/orgbot-browser-policy-XXXXXX")
trap 'rm -rf "$BUILD_DIR"' EXIT

read -r -a CFLAGS <<<"$(pkg-config --cflags glib-2.0)"
read -r -a LIBS <<<"$(pkg-config --libs glib-2.0)"
${CXX:-c++} -std=c++17 "${CFLAGS[@]}" \
  "$RUNNER_DIR/native_browser_policy_test.cc" \
  "${LIBS[@]}" -o "$BUILD_DIR/native-browser-policy-test"
"$BUILD_DIR/native-browser-policy-test"
