#!/usr/bin/env bash
# Cross-compile clayform.exe for Windows from Linux without rustup's
# Windows target download: builds the Windows std from Rust's source
# (fetched from GitHub) with -Zbuild-std and links with MinGW.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v x86_64-w64-mingw32-gcc >/dev/null || sudo apt-get install -y gcc-mingw-w64-x86-64

VER=$(rustc --version | awk '{print $2}')
SYS=$(rustc --print sysroot)
SRC="$SYS/lib/rustlib/src/rust"
if [ ! -f "$SRC/library/std/Cargo.toml" ]; then
  WORK="${RUST_SRC_CACHE:-$HOME/.cache/rust-src-$VER}"
  if [ ! -d "$WORK/library" ]; then
    git clone -q --depth 1 --branch "$VER" --filter=blob:none --sparse https://github.com/rust-lang/rust "$WORK"
    git -C "$WORK" sparse-checkout set library
    BT=$(git -C "$WORK" ls-tree HEAD library/backtrace | awk '{print $3}')
    rmdir "$WORK/library/backtrace" 2>/dev/null || true
    git init -q "$WORK/library/backtrace"
    git -C "$WORK/library/backtrace" fetch -q --depth 1 https://github.com/rust-lang/backtrace-rs.git "$BT"
    git -C "$WORK/library/backtrace" checkout -q FETCH_HEAD
  fi
  mkdir -p "$SRC"
  ln -sfn "$WORK/library" "$SRC/library"
fi

RUSTC_BOOTSTRAP=1 \
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
cargo build --release -Zbuild-std=std,panic_unwind --target x86_64-pc-windows-gnu

mkdir -p dist
x86_64-w64-mingw32-strip -o dist/clayform.exe target/x86_64-pc-windows-gnu/release/clayform.exe
echo "Built dist/clayform.exe"
