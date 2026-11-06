#!/bin/sh
# Install the latest hprof-slurp release on Linux or macOS.
#
#   curl -fsSL https://agourlay.github.io/hprof-slurp/install.sh | sh
#
# Downloads the release archive for this machine from GitHub and puts the
# `hprof-slurp` binary in ~/.local/bin, or in
# $HPROF_SLURP_INSTALL_DIR when set. It is one self-contained binary,
# so that is the whole install; to uninstall, delete it.
#
# Everything lives inside main, called on the last line, so a download cut
# off halfway runs nothing rather than half a script.
set -eu

repo="agourlay/hprof-slurp"

say() { printf '%s\n' "$*"; }
die() { printf 'hprof-slurp install: %s\n' "$*" >&2; exit 1; }

main() {
  command -v curl >/dev/null 2>&1 || die "curl is required"
  command -v tar >/dev/null 2>&1 || die "tar is required"

  case "$(uname -s)" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) die "unsupported system $(uname -s); on Windows, use the PowerShell command instead" ;;
  esac

  case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) die "unsupported processor $(uname -m)" ;;
  esac

  # An x86_64 shell under Rosetta reports x86_64 on an Apple Silicon Mac;
  # the native build is the one to want there.
  if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
    [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = "1" ]; then
    arch="aarch64"
  fi

  target="$arch-$os"
  # The six targets the release workflow builds.
  case "$target" in
    x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | \
    x86_64-apple-darwin | aarch64-apple-darwin) ;;
    *) die "no prebuilt release for $target; build it with: cargo install hprof-slurp" ;;
  esac

  # The latest tag, read off the redirect GitHub answers releases/latest
  # with, rather than from the API, which rate-limits anonymous callers.
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
    "https://github.com/$repo/releases/latest") ||
    die "could not reach GitHub"
  tag=${latest##*/}
  case "$tag" in
    v[0-9]*) ;;
    *) die "could not find the latest release (got $latest)" ;;
  esac

  archive="hprof-slurp-$target.tar.gz"
  url="https://github.com/$repo/releases/download/$tag/$archive"
  dir="${HPROF_SLURP_INSTALL_DIR:-$HOME/.local/bin}"

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  say "Downloading hprof-slurp $tag for $target"
  curl -fL --progress-bar -o "$tmp/$archive" "$url" ||
    die "download failed: $url"
  tar -xzf "$tmp/$archive" -C "$tmp"

  mkdir -p "$dir"
  # Moved into place in one step, so a run already using the old binary keeps
  # its file and the new one appears whole.
  cp "$tmp/hprof-slurp" "$dir/.hprof-slurp.new"
  chmod 755 "$dir/.hprof-slurp.new"
  mv -f "$dir/.hprof-slurp.new" "$dir/hprof-slurp"

  say "Installed $dir/hprof-slurp"
  case ":$PATH:" in
    *":$dir:"*) say "Run it with: hprof-slurp" ;;
    *)
      say "$dir is not on your PATH. Run it with:"
      say "  $dir/hprof-slurp"
      say "or add this line to your shell profile:"
      say "  export PATH=\"$dir:\$PATH\""
      ;;
  esac
}

main "$@"
