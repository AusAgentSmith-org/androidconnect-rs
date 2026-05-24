#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/apps/android/app/src/main/jniLibs"

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "cargo-ndk is required: cargo install cargo-ndk" >&2
    exit 1
fi

mkdir -p "$OUT"

cargo ndk \
    --manifest-path "$ROOT/Cargo.toml" \
    -t arm64-v8a \
    -t armeabi-v7a \
    -t x86_64 \
    -o "$OUT" \
    build \
    -p androidconnect-android-native \
    --release

echo "native libraries written to $OUT"
