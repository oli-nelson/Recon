#!/usr/bin/env bash
# Package only the tested native executable and user documentation.
set -Eeuo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source "$script_dir/release-version.sh"
[[ $# == 2 ]] || { echo 'Usage: package-release.sh <version> <platform>' >&2; exit 1; }
version=$(release_version "$1")
platform=$2
cd "$script_dir/.."
binary=recon
case "$platform" in
  macos-x64) target=x86_64-apple-darwin ;;
  macos-arm64) target=aarch64-apple-darwin ;;
  windows-x64) target=x86_64-pc-windows-msvc; binary=recon.exe ;;
  linux-x64) target=x86_64-unknown-linux-gnu ;;
  *) echo 'error: unsupported release platform' >&2; exit 1 ;;
esac
[[ -s "target/$target/release/$binary" ]] || { echo 'error: missing release executable' >&2; exit 1; }
mkdir -p artifacts/release
tar -czf "artifacts/release/recon-$version-$platform.tar.gz" \
  -C "target/$target/release" "$binary" -C "$PWD" README.md config.default.toml
tar -tzf "artifacts/release/recon-$version-$platform.tar.gz"
