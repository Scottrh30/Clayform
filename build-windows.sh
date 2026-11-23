#!/usr/bin/env bash
# Builds the Windows app from Linux (no GitHub, no Windows PC needed):
#   ./build-windows.sh           dist/Clayform-portable.zip (+ dist/Clayform-setup.exe when NSIS works)
#   ./build-windows.sh --test    run the engine tests first
#
# Uses the x86_64-pc-windows-gnu target (MinGW). The portable build needs
# WebView2Loader.dll beside Clayform.exe, so it comes as a zip of both.
#
# If rustup can't download the wasm32 / windows-gnu standard libraries
# (e.g. static.rust-lang.org is blocked), the script builds them from
# Rust's source on GitHub instead (-Zbuild-std).
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"

need() { command -v "$1" >/dev/null 2>&1; }
if ! need x86_64-w64-mingw32-gcc || ! need zip; then
  echo "Installing MinGW and zip (first time only)..."
  if need sudo && [ "$(id -u)" != 0 ]; then SUDO=sudo; else SUDO=; fi
  $SUDO apt-get install -y mingw-w64 zip nsis >/dev/null
fi

# ---------------------------------------------------------------- toolchains
WASM_STD=(); WIN_STD=()
if ! rustup target add wasm32-unknown-unknown x86_64-pc-windows-gnu >/dev/null 2>&1; then
  echo "rustup can't download the targets; building the standard library from source instead."
  VER=$(rustc --version | awk '{print $2}')
  SRC="$(rustc --print sysroot)/lib/rustlib/src/rust"
  if [ ! -f "$SRC/library/std/Cargo.toml" ]; then
    WORK="${RUST_SRC_CACHE:-$HOME/.cache/rust-src-$VER}"
    if [ ! -d "$WORK/library/std" ]; then
      rm -rf "$WORK"
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
  export RUSTC_BOOTSTRAP=1
  WASM_STD=(env CARGO_UNSTABLE_BUILD_STD=std,panic_abort)
  WIN_STD=(env CARGO_UNSTABLE_BUILD_STD=std,panic_unwind)
fi
need trunk || cargo install trunk --locked
WBG=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/version = "\(.*\)"/\1/p')
if ! need wasm-bindgen || [ "$(wasm-bindgen --version | awk '{print $2}')" != "$WBG" ]; then
  cargo install wasm-bindgen-cli --version "$WBG" --locked
fi
need npx || { echo "Node.js (npx) is needed: https://nodejs.org"; exit 1; }

if [ "${1:-}" = "--test" ]; then
  cargo test --release -p clayform-core
fi

# --------------------------------------------------------------------- build
echo "Building the interface (WebAssembly)..."
(cd app && "${WASM_STD[@]}" trunk build --release)

echo "Building Clayform for Windows (release)..."
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
NO_HOOK='{"build":{"beforeBuildCommand":""}}'
if ! "${WIN_STD[@]}" npx --yes @tauri-apps/cli@2 build --target x86_64-pc-windows-gnu --config "$NO_HOOK"; then
  echo "Installer step failed; building the portable app only."
  "${WIN_STD[@]}" npx --yes @tauri-apps/cli@2 build --target x86_64-pc-windows-gnu --config "$NO_HOOK" --no-bundle
fi

out=target/x86_64-pc-windows-gnu/release
rm -rf dist && mkdir -p dist/Clayform
cp "$out/clayform.exe" dist/Clayform/Clayform.exe
cp "$out/WebView2Loader.dll" dist/Clayform/
(cd dist && zip -qr Clayform-portable.zip Clayform)
setup=$(ls "$out"/bundle/nsis/*-setup.exe 2>/dev/null | head -1 || true)
[ -n "$setup" ] && cp "$setup" dist/Clayform-setup.exe
echo
echo "Done:"
[ -n "$setup" ] && echo "  $(pwd)/dist/Clayform-setup.exe      (installer)"
echo "  $(pwd)/dist/Clayform-portable.zip   (unzip, run Clayform.exe)"
