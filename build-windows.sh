#!/bin/bash
# Cross-compile the Windows binaries into dist/. Needs: rustup target x86_64-pc-windows-gnu + mingw-w64.
set -e
cd "$(dirname "$0")"
rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true
cargo build --release --target x86_64-pc-windows-gnu -p relume -p relume-cli
mkdir -p dist
cp target/x86_64-pc-windows-gnu/release/Relume.exe target/x86_64-pc-windows-gnu/release/relume-cli.exe dist/
cp README.md LICENSE dist/
(cd dist && rm -f Relume-windows-x64.zip && zip -q Relume-windows-x64.zip Relume.exe relume-cli.exe README.md LICENSE)
ls -la dist
