#!/usr/bin/env bash
# Build the engine as an Android shared library.
#
# The NDK path is machine-specific, so it is discovered here rather than
# checked into .cargo/config.toml where it would only work on one machine.
# Override with ANDROID_NDK_HOME if yours lives somewhere else.
set -euo pipefail
cd "$(dirname "$0")/.."

NDK_ROOT=${ANDROID_NDK_HOME:-}
if [ -z "$NDK_ROOT" ]; then
  # Newest NDK under the usual SDK location.
  NDK_ROOT=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1 || true)
fi
[ -n "$NDK_ROOT" ] || { echo "no NDK found; set ANDROID_NDK_HOME" >&2; exit 1; }

BIN="$NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin"
[ -d "$BIN" ] || { echo "no toolchain at $BIN" >&2; exit 1; }

# Minimum Android version the library supports.
API=${ANDROID_API:-24}

build() {
  local target=$1 clang=$2 env_target=$3
  rustup target list --installed | grep -qx "$target" || {
    echo "skipping $target (run: rustup target add $target)" >&2
    return 0
  }
  echo "building $target"
  env \
    "CC_${env_target}=$BIN/${clang}${API}-clang" \
    "AR_${env_target}=$BIN/llvm-ar" \
    "CARGO_TARGET_$(echo "$env_target" | tr '[:lower:]' '[:upper:]')_LINKER=$BIN/${clang}${API}-clang" \
    cargo build -p orgbot-ffi --release --target "$target"
}

build aarch64-linux-android aarch64-linux-android aarch64_linux_android
build armv7-linux-androideabi armv7a-linux-androideabi armv7_linux_androideabi
build x86_64-linux-android x86_64-linux-android x86_64_linux_android

echo
echo "libraries:"
for t in aarch64-linux-android armv7-linux-androideabi x86_64-linux-android; do
  so="target/$t/release/liborgbot_ffi.so"
  [ -f "$so" ] && echo "  $so ($(du -h "$so" | cut -f1))"
done
