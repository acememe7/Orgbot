#!/usr/bin/env bash
# Build AND package the requested native engine. arm64 is the Android default.
set -euo pipefail
cd "$(dirname "$0")/.."
ndk_root=${ANDROID_NDK_HOME:-}
if [ -z "$ndk_root" ]; then
  ndk_root=$(ls -d "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/* 2>/dev/null | sort -V | tail -1 || true)
fi
[ -n "$ndk_root" ] || { echo "No NDK found; set ANDROID_NDK_HOME" >&2; exit 1; }
ndk_bin="$ndk_root/toolchains/llvm/prebuilt/linux-x86_64/bin"
api=${ANDROID_API:-24}
for target in "${@:-aarch64-linux-android}"; do
  case "$target" in
    aarch64-linux-android) clang=aarch64-linux-android; abi=arm64-v8a ;;
    armv7-linux-androideabi) clang=armv7a-linux-androideabi; abi=armeabi-v7a ;;
    x86_64-linux-android) clang=x86_64-linux-android; abi=x86_64 ;;
    *) echo "Unsupported Android target: $target" >&2; exit 1 ;;
  esac
  rustup target list --installed | rg -qx "$target" || { echo "Run: rustup target add $target" >&2; exit 1; }
  env_target=${target//-/_}
  upper_target=${env_target^^}
  env "CC_${env_target}=$ndk_bin/${clang}${api}-clang" \
    "AR_${env_target}=$ndk_bin/llvm-ar" \
    "CARGO_TARGET_${upper_target}_LINKER=$ndk_bin/${clang}${api}-clang" \
    cargo build --locked -p orgbot-ffi --release --target "$target"
  dest="../app/android/app/src/main/jniLibs/$abi"
  mkdir -p "$dest"
  cp "target/$target/release/liborgbot_ffi.so" "$dest/liborgbot_ffi.so"
  "$ndk_bin/llvm-readelf" -Ws "$dest/liborgbot_ffi.so" | rg 'orgbot_room_control$' >/dev/null
  echo "Packaged $abi engine"
done
