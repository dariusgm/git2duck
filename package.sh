#!/usr/bin/env bash
set -euo pipefail

# Package the git2duck DuckDB extension by:
# 1) Building the shared library with the loadable entrypoint
# 2) Ensuring duckdb/extension-ci-tools is present (auto-clone if missing)
# 3) Appending metadata to produce git2duck.duckdb_extension
#
# Usage:
#   ./package.sh            # auto-detect DuckDB version
#   DUCKDB_VERSION=v1.1.3 ./package.sh  # specify version explicitly

REPO_ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ext_tools_dir="/tmp/extension-ci-tools"
lib_path="$REPO_ROOT_DIR/target/release/libgit2duck.so"
ext_out="$REPO_ROOT_DIR/target/release/git2duck.duckdb_extension"

# 0) Ensure repo exists
if [[ ! -d "$REPO_ROOT_DIR/.git" ]]; then
  echo "ERROR: This script must be run from the git2duck repository root." >&2
  exit 1
fi

# 1) Build the shared library with entrypoint
echo "[git2duck] Building release library (with loadable entrypoint)..."
cargo build --release --features ext

if [[ ! -f "$lib_path" ]]; then
  echo "ERROR: Built library not found at: $lib_path" >&2
  exit 1
fi

# 2) Ensure extension-ci-tools present
if [[ ! -d "$ext_tools_dir" ]]; then
  echo "[git2duck] Cloning duckdb/extension-ci-tools into $ext_tools_dir ..."
  git clone https://github.com/duckdb/extension-ci-tools.git "$ext_tools_dir"
fi

# 3) Determine DuckDB version
if [[ -n "${DUCKDB_VERSION:-}" ]]; then
  dv="$DUCKDB_VERSION"
else
  # Try CLI
  if command -v duckdb >/dev/null 2>&1; then
    # First token is like v1.1.3
    dv="$(duckdb --version | awk '{print $1}')"
  else
    # Try Python duckdb module
    if python3 -c "import duckdb,sys; sys.stdout.write('v'+duckdb.__version__)" >/dev/null 2>&1; then
      dv="$(python3 -c "import duckdb,sys; sys.stdout.write('v'+duckdb.__version__)")"
    else
      echo "ERROR: Could not detect DuckDB version. Set DUCKDB_VERSION env, e.g. DUCKDB_VERSION=v1.1.3" >&2
      exit 1
    fi
  fi
fi

if [[ -z "$dv" ]]; then
  echo "ERROR: DuckDB version empty. Set DUCKDB_VERSION=vX.Y.Z" >&2
  exit 1
fi

echo "[git2duck] Using DuckDB version: $dv"

# 4) Package with append_extension_metadata.py
appender="$ext_tools_dir/scripts/append_extension_metadata.py"
if [[ ! -f "$appender" ]]; then
  echo "ERROR: append_extension_metadata.py not found at $appender" >&2
  exit 1
fi

# Determine DuckDB platform string (can override via DUCKDB_PLATFORM)
if [[ -n "${DUCKDB_PLATFORM:-}" ]]; then
  plat="$DUCKDB_PLATFORM"
else
  sys="$(uname -s | tr '[:upper:]' '[:lower:]')"
  mach="$(uname -m)"
  case "$sys" in
    linux)
      case "$mach" in
        x86_64|amd64) arch="amd64" ;;
        aarch64|arm64) arch="arm64" ;;
        *) echo "ERROR: Unsupported linux arch: $mach" >&2; exit 1 ;;
      esac
      if ldd --version 2>&1 | grep -qi musl; then
        libc="musl"
      else
        libc="gnu"
      fi
      plat="linux_${arch}_${libc}"
      ;;
    darwin)
      case "$mach" in
        arm64) plat="osx_arm64" ;;
        x86_64) plat="osx_amd64" ;;
        *) echo "ERROR: Unsupported macOS arch: $mach" >&2; exit 1 ;;
      esac
      ;;
    msys*|mingw*|cygwin*|windows*)
      plat="windows_amd64"
      ;;
    *)
      echo "ERROR: Unsupported OS: $sys" >&2; exit 1 ;;
  esac
fi

echo "[git2duck] Using DuckDB platform: $plat"

# Determine extension version (overridable via EXT_VERSION)
if [[ -n "${EXT_VERSION:-}" ]]; then
  ev="$EXT_VERSION"
else
  ev="$(cat Cargo.toml | grep version | head -n 1 | sed -E 's/.*version\s*=\s*"([^"]+)".*/\1/')"
fi

if [[ -z "$ev" ]]; then
  echo "ERROR: Could not determine extension version from Cargo.toml. Set EXT_VERSION explicitly." >&2
  exit 1
fi

echo "[git2duck] Using Extension version: $ev"

echo "[git2duck] Packaging $lib_path -> $ext_out"
python3 "$appender" \
  -l "$lib_path" \
  -n git \
  -o "$ext_out" \
  -dv "$dv" \
  -ev "$ev" \
  -p "$plat"

echo "[git2duck] Done. Packaged extension at: $ext_out"
