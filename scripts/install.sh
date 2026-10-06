#!/bin/sh
# Installs the onus binaries from a GitHub release.
#
#   curl -fsSL https://onushq.com/install.sh | sh
#
# Environment:
#   ONUS_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   ONUS_INSTALL_DIR  where to put the binaries (default: ~/.local/bin)
#
# The archive is checked against its published SHA-256 before anything is
# installed. Nothing else on the system is touched.

set -eu

repo="onushq/onus"
version="${ONUS_VERSION:-latest}"
dir="${ONUS_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'onus-install: %s\n' "$*" >&2; }
die() { say "$*"; exit 1; }

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) die "unsupported system $(uname -s); on Windows, download onus-x86_64-pc-windows-msvc.zip from https://github.com/$repo/releases" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac
# A shell running under Rosetta reports x86_64; prefer the native binary.
if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="aarch64"
fi
target="$arch-$os"
asset="onus-$target.tar.gz"

if [ "$version" = "latest" ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/$version"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --https-only -O "$2" "$1"; }
else
  die "needs curl or wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "needs sha256sum or shasum to verify the download"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $asset ($version)"
fetch "$base/$asset" "$tmp/$asset" || die "download failed: $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || die "download failed: $base/$asset.sha256"

want="$(cut -d' ' -f1 <"$tmp/$asset.sha256")"
got="$(sha256 "$tmp/$asset")"
[ "$want" = "$got" ] || die "checksum mismatch for $asset: expected $want, got $got"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
for bin in onus onus-plugin-svelte; do
  if [ -f "$tmp/onus-$target/$bin" ]; then
    install -m 755 "$tmp/onus-$target/$bin" "$dir/$bin"
  fi
done

say "installed $("$dir/onus" --version) to $dir"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) say "add $dir to your PATH, for example: export PATH=\"$dir:\$PATH\"" ;;
esac
