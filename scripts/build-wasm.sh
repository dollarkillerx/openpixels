#!/usr/bin/env bash
# Builds crates/op-wasm and generates the JS bindings in web/src/wasm-pkg.
set -euo pipefail
cd "$(dirname "$0")/.."

# Homebrew's rustup does not put cargo/rustc on PATH.
if ! command -v cargo >/dev/null; then
  PATH="$(dirname "$(rustup which rustc)"):$PATH"
  export PATH
fi

# Without the rustup proxy, rust-lld cannot find libLLVM.dylib on its own. Set here
# unconditionally because macOS strips DYLD_* from the environment of this script.
DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print sysroot)/lib"
export DYLD_FALLBACK_LIBRARY_PATH

# Must match the wasm-bindgen version pinned in crates/op-wasm/Cargo.toml.
BINDGEN_VERSION=0.2.130
BINDGEN=.tools/bin/wasm-bindgen
if ! [ -x "$BINDGEN" ] || [ "$("$BINDGEN" --version)" != "wasm-bindgen $BINDGEN_VERSION" ]; then
  cargo install wasm-bindgen-cli --version "$BINDGEN_VERSION" --root .tools --locked
fi

RUSTFLAGS="-C target-feature=+simd128" \
  cargo build -p op-wasm --release --target wasm32-unknown-unknown

"$BINDGEN" --target web --out-dir web/src/wasm-pkg \
  target/wasm32-unknown-unknown/release/op_wasm.wasm

ls -lh web/src/wasm-pkg/op_wasm_bg.wasm
