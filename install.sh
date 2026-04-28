#!/bin/sh
# WaveKat CLI installer.
#
#   curl -fsSL https://github.com/wavekat/wavekat-cli/releases/latest/download/install.sh | sh
#
# Environment overrides:
#   WK_VERSION     pin a specific tag (e.g. v0.0.3); default: latest release
#   WK_INSTALL_DIR install directory; default: $HOME/.local/bin (or /usr/local/bin if writable)

set -eu

REPO="wavekat/wavekat-cli"
BIN="wk"

err()  { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }

need() { command -v "$1" >/dev/null 2>&1 || err "missing required command: $1"; }
need uname
need tar
need mkdir
need install

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_stdout() { wget -qO- "$1"; }
else
  err "need either curl or wget"
fi

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        x86_64)        echo "x86_64-apple-darwin" ;;
        *) err "unsupported macOS arch: $arch" ;;
      esac ;;
    Linux)
      case "$arch" in
        x86_64|amd64)  echo "x86_64-unknown-linux-musl" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-musl" ;;
        *) err "unsupported Linux arch: $arch" ;;
      esac ;;
    *) err "unsupported OS: $os (try the cargo install path)" ;;
  esac
}

resolve_version() {
  if [ -n "${WK_VERSION:-}" ]; then
    echo "$WK_VERSION"
    return
  fi
  # The /releases/latest endpoint redirects to /releases/tag/<tag>; pick the tag
  # off the redirect. Avoids needing jq.
  if command -v curl >/dev/null 2>&1; then
    tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/${REPO}/releases/latest" | sed 's|.*/tag/||')
  else
    tag=$(wget -qS --max-redirect=0 "https://github.com/${REPO}/releases/latest" 2>&1 \
      | awk '/Location:/ {print $2}' | tail -1 | sed 's|.*/tag/||')
  fi
  [ -n "$tag" ] || err "could not resolve latest version"
  echo "$tag"
}

pick_install_dir() {
  if [ -n "${WK_INSTALL_DIR:-}" ]; then
    echo "$WK_INSTALL_DIR"
    return
  fi
  if [ -w /usr/local/bin ] 2>/dev/null; then
    echo "/usr/local/bin"
  else
    echo "$HOME/.local/bin"
  fi
}

verify_sha256() {
  # POSIX sh has no `local`, so use distinct names to avoid clobbering
  # the caller's $archive / $expected.
  _vs_file="$1"
  _vs_expected="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    _vs_actual=$(sha256sum "$_vs_file" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    _vs_actual=$(shasum -a 256 "$_vs_file" | awk '{print $1}')
  else
    info "warning: no sha256 tool found, skipping checksum verification"
    return 0
  fi
  [ "$_vs_actual" = "$_vs_expected" ] || err "checksum mismatch (expected $_vs_expected, got $_vs_actual)"
}

main() {
  target=$(detect_target)
  tag=$(resolve_version)
  case "$tag" in v*) ;; *) tag="v$tag" ;; esac
  install_dir=$(pick_install_dir)

  archive="${BIN}-${tag}-${target}.tar.gz"
  base="https://github.com/${REPO}/releases/download/${tag}"
  url="${base}/${archive}"
  # taiki-e/upload-rust-binary-action uploads the checksum as <bin>-<tag>-<target>.sha256
  # (without the .tar.gz suffix), so build that name explicitly.
  sha_url="${base}/${BIN}-${tag}-${target}.sha256"

  info "Installing wk $tag for $target"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  fetch "$url" "$tmp/$archive"
  sha_text=$(fetch_stdout "$sha_url") || err "could not download checksum from $sha_url"
  sha=$(printf '%s' "$sha_text" | awk '{print $1}')
  [ -n "$sha" ] || err "checksum file at $sha_url was empty"
  verify_sha256 "$tmp/$archive" "$sha"

  tar -xzf "$tmp/$archive" -C "$tmp"
  src=$(find "$tmp" -type f -name "$BIN" -perm -u+x | head -n1)
  [ -n "$src" ] || err "binary $BIN not found in archive"

  mkdir -p "$install_dir"
  install -m 0755 "$src" "$install_dir/$BIN"

  info "Installed $install_dir/$BIN"
  case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) info "Note: $install_dir is not on your PATH; add it to your shell rc." ;;
  esac
  info "Run: $BIN --version"
}

main "$@"
