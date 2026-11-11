#!/bin/sh
# Builds the desktop engine library (target/release/libop_native.*) and the MCP server
# for AI agents (target/release/openpixels-mcp).
# Works whether cargo is on PATH or only reachable through rustup.
set -e
cd "$(dirname "$0")/.."
if command -v cargo >/dev/null 2>&1; then
  CARGO="$(command -v cargo)"
else
  CARGO="$(rustup which cargo)"
  PATH="$(dirname "$CARGO"):$PATH"
  export PATH
fi
# Homebrew's rustup leaves the toolchain's LLVM off the library path, which the
# release build needs to strip debug info.
DYLD_FALLBACK_LIBRARY_PATH="$("$(dirname "$CARGO")/rustc" --print sysroot)/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
export DYLD_FALLBACK_LIBRARY_PATH
"$CARGO" build -p op-native -p op-mcp --release
