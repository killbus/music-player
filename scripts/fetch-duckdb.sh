#!/usr/bin/env bash
#
# Fetch the prebuilt static DuckDB libraries into vendor/duckdb/lib.
#
# This cannot live in analytics/build.rs. `libduckdb-sys` resolves
# `-l static=duckdb_static` when *its own* rlib is compiled, and Cargo gives no
# way to order another crate's build script ahead of that, so by the time our
# build script could download anything the build has already failed. The fetch
# therefore has to happen before Cargo is invoked at all.
#
# Idempotent: re-running with the archives already in place does nothing.
set -euo pipefail

# Must match DUCKDB_VERSION in analytics/build.rs, which must in turn match the
# DuckDB wrapped by the `duckdb` crate (1.<major><minor><patch> => 1.10505.x is
# DuckDB 1.5.5). The Rust bindings are pregenerated against that exact C API.
VERSION="1.5.5"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lib_dir="$root/vendor/duckdb/lib"
stamp="$lib_dir/.stamp"

# The platform of the *target* matters, not the machine running this script:
# cross-compiling x86_64-apple-darwin on an arm64 macOS runner must link the
# amd64 archives, or the link fails and the release job dies (v0.4.5).
# CARGO_BUILD_TARGET is how release workflows pin the target; the platform is
# only resolved from the host when no explicit target is set.
target="${CARGO_BUILD_TARGET:-}"

platform_from_target() {
  case "$1" in
    x86_64-apple-darwin)       echo "osx-amd64"   ;;
    aarch64-apple-darwin)      echo "osx-arm64"   ;;
    x86_64-unknown-linux-gnu)  echo "linux-amd64" ;;
    aarch64-unknown-linux-gnu) echo "linux-arm64" ;;
    *) return 1 ;;
  esac
}

platform_from_host() {
  case "$1" in
    Darwin/arm64)  echo "osx-arm64"   ;;
    Darwin/x86_64) echo "osx-amd64"   ;;
    Linux/aarch64) echo "linux-arm64" ;;
    Linux/x86_64)  echo "linux-amd64" ;;
    *) return 1 ;;
  esac
}

platform="$(platform_from_target "$target")" || {
  if [ -n "$target" ]; then
    echo "fetch-duckdb: no prebuilt static DuckDB for target $target." >&2
    echo "  Build against a shared library instead:" >&2
    echo "    DUCKDB_LIB_DIR= DUCKDB_STATIC= DUCKDB_DOWNLOAD_LIB=1 cargo build" >&2
    exit 1
  fi
  platform="$(platform_from_host "$(uname -s)/$(uname -m)")" || {
    echo "fetch-duckdb: no prebuilt static DuckDB for $(uname -s)/$(uname -m)." >&2
    echo "  Build against a shared library instead:" >&2
    echo "    DUCKDB_LIB_DIR= DUCKDB_STATIC= DUCKDB_DOWNLOAD_LIB=1 cargo build" >&2
    exit 1
  }
}

want="$VERSION-$platform"
if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
  echo "duckdb $want already vendored"
  exit 0
fi

url="https://github.com/duckdb/duckdb/releases/download/v$VERSION/static-libs-$platform.zip"
echo "fetching duckdb $VERSION ($platform)"
echo "  $url"

# A stale directory holds archives for a different version or platform; mixing
# those links cleanly and then misbehaves at runtime.
rm -rf "$lib_dir"
mkdir -p "$lib_dir"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl --fail --location --progress-bar --output "$tmp/static-libs.zip" "$url"
# -j flattens: the archives sit at the root of the zip already, and this also
# defuses any leading path components.
unzip -q -j "$tmp/static-libs.zip" '*.a' 'duckdb.h' -d "$lib_dir"

if [ ! -f "$lib_dir/libduckdb_static.a" ]; then
  echo "fetch-duckdb: $url did not contain libduckdb_static.a" >&2
  exit 1
fi

# Written last: analytics/build.rs trusts the stamp to mean the directory holds
# a complete set of archives for this platform.
printf '%s' "$want" > "$stamp"

echo "duckdb $want vendored into vendor/duckdb/lib"
