#!/usr/bin/env bash
# Read-only preflight for a real run.
#
#   ./scripts/preflight.sh [bbcp.toml]
#
# Checks tools, configuration, filesystems and free space. It does not modify
# media or the inventory (it may create the .bbcp directory to test the
# filesystem).
set -uo pipefail

CONFIG="${1:-bbcp.toml}"
fail=0

ok()   { printf '  ok    %s\n' "$*"; }
warn() { printf '  warn  %s\n' "$*"; }
bad()  { printf '  FAIL  %s\n' "$*"; fail=$((fail + 1)); }

echo "tools"
version_of() {
  case "$1" in
    cargo) "$1" --version 2>/dev/null | head -1 ;;
    *)     "$1" -version 2>/dev/null | head -1 ;;
  esac
}
for tool in cargo ffmpeg ffprobe; do
  if command -v "$tool" >/dev/null 2>&1; then
    ok "$tool ($(version_of "$tool"))"
  else
    bad "$tool not found"
  fi
done
if command -v sqlite3 >/dev/null 2>&1; then
  ok "sqlite3 ($(sqlite3 --version | cut -d' ' -f1))"
else
  warn "sqlite3 CLI not found (optional; bbcp bundles SQLite)"
fi

echo
echo "configuration"
if [ ! -f "$CONFIG" ]; then
  bad "$CONFIG not found (run: bbcp init --album /path/to/Album)"
  echo
  echo "preflight failed"
  exit "$fail"
fi
ok "$CONFIG"

if command -v bbcp >/dev/null 2>&1; then
  if bbcp --config "$CONFIG" doctor; then
    ok "bbcp doctor"
  else
    bad "bbcp doctor reported a problem"
  fi
else
  warn "bbcp not on PATH; build it with ./scripts/install.sh"
fi

# Extract a few paths without a TOML parser.
album="$(sed -n 's/^[[:space:]]*album[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' "$CONFIG" | head -1)"
db="$(sed -n 's/^[[:space:]]*db[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' "$CONFIG" | head -1)"
base="$(cd "$(dirname "$CONFIG")" && pwd)"

resolve() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *)  printf '%s\n' "$base/$1" ;;
  esac
}

echo
echo "filesystems"
if [ -n "$db" ]; then
  db_path="$(resolve "$db")"
  db_dir="$(dirname "$db_path")"
  mkdir -p "$db_dir" 2>/dev/null
  fs_type="$(stat -f -c '%T' "$db_dir" 2>/dev/null || echo unknown)"
  case "$fs_type" in
    9p|nfs|nfs4|cifs|smb3|fuse*)
      bad "database directory is on $fs_type; move it to a local disk"
      ;;
    *)
      ok "database directory on $fs_type"
      ;;
  esac
fi

if [ -n "$album" ]; then
  album_path="$(resolve "$album")"
  if [ -d "$album_path" ]; then
    ok "album exists: $album_path"
    if [ -w "$album_path" ]; then
      ok "album is writable"
    else
      bad "album is not writable: $album_path"
    fi
    free="$(df -h "$album_path" | awk 'NR==2 {print $4}')"
    ok "free space at album: $free"
  else
    bad "album does not exist: $album_path"
  fi
fi

echo
echo "roots"
awk '
  function value(s) {
    sub(/^[^=]*=[[:space:]]*"/, "", s)
    sub(/".*$/, "", s)
    return s
  }
  function flush() {
    if (label != "" || path != "") {
      printf "  %-20s %-8s %s\n", label, kind, path
    }
    label = ""; path = ""; kind = ""
  }
  /^\[\[roots\]\]/ { flush(); next }
  /^[[:space:]]*label[[:space:]]*=/ { label = value($0); next }
  /^[[:space:]]*path[[:space:]]*=/  { path  = value($0); next }
  /^[[:space:]]*kind[[:space:]]*=/  { kind  = value($0); next }
  END { flush() }
' "$CONFIG"

echo
if [ "$fail" -gt 0 ]; then
  echo "preflight failed ($fail problem(s))"
  exit "$fail"
fi
echo "preflight ok"
