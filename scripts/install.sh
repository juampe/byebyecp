#!/usr/bin/env bash
# Build bbcp and install the binary.
#
#   ./scripts/install.sh                  # ~/.local/bin
#   PREFIX=/usr/local ./scripts/install.sh
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local/bin}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install Rust: https://rustup.rs" >&2
  exit 1
fi

echo "building bbcp (release)"
cargo build --release -p bbcp-cli

install -d "$PREFIX"
install -m 0755 "$ROOT/target/release/bbcp" "$PREFIX/bbcp"

echo "installed $PREFIX/bbcp"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) echo "note: $PREFIX is not on PATH" ;;
esac
