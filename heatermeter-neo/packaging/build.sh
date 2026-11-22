#!/bin/sh
# Build the web UI, cross-compile linkmeterd and package .deb files.
#
#   packaging/build.sh [target...]
#
# Default targets: aarch64 (Pi 3/4/5/Zero 2 W, 64-bit OS), armv7 (32-bit OS on
# Pi 2/3/4) and armv6 (Pi Zero W / Pi 1, 32-bit OS). Requires node, zig,
# cargo-zigbuild and cargo-deb.
set -eu
cd "$(dirname "$0")/.."

TARGETS=${*:-"aarch64-unknown-linux-gnu armv7-unknown-linux-gnueabihf arm-unknown-linux-gnueabihf"}

(cd web && npm ci && npm run build)

for t in $TARGETS; do
  rustup target add "$t" >/dev/null 2>&1 || true
  # glibc 2.31 keeps the binary compatible with Raspberry Pi OS Bullseye and later
  cargo zigbuild --release -p linkmeterd --target "$t.2.31"
  cargo deb -p linkmeterd --no-build --no-strip --target "$t" --output target/deb/
done
ls -l target/deb/
