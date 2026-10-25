#!/usr/bin/env bash
set -euo pipefail

RUNNER_DIR=$(cd "$(dirname "$0")" && pwd)
APP_DIR=$(cd "$RUNNER_DIR/../.." && pwd)
BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/orgbot-editor-recovery-build-XXXXXX")
trap 'rm -rf "$BUILD_DIR"' EXIT

read -r -a CFLAGS <<<"$(pkg-config --cflags gio-2.0 gtk+-3.0)"
read -r -a LIBS <<<"$(pkg-config --libs gio-2.0 gtk+-3.0)"
${CXX:-c++} -std=c++17 -ffunction-sections -fdata-sections \
  -I"$APP_DIR/linux/flutter/ephemeral" -I"$RUNNER_DIR" \
  "${CFLAGS[@]}" "$RUNNER_DIR/desktop_editor_recovery_test.cc" \
  -Wl,--gc-sections "${LIBS[@]}" -o "$BUILD_DIR/editor-recovery-test"
"$BUILD_DIR/editor-recovery-test"
