#!/usr/bin/env sh
set -eu

repo="${HIVEMIND_REPO:-alexknips/hivemind}"
install_dir="${HIVEMIND_INSTALL_DIR:-$HOME/.local/bin}"
version="${HIVEMIND_VERSION:-}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux) platform="linux" ;;
  Darwin) platform="macos" ;;
  *)
    echo "unsupported OS: $os" >&2
    exit 1
    ;;
esac

case "$arch" in
  x86_64 | amd64) cpu="x86_64" ;;
  aarch64 | arm64) cpu="arm64" ;;
  *)
    echo "unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

asset="hivemind-${platform}-${cpu}.tar.gz"
if [ -n "$version" ]; then
  base_url="https://github.com/${repo}/releases/download/${version}"
else
  base_url="https://github.com/${repo}/releases/latest/download"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

curl -fsSL "${base_url}/${asset}" -o "${tmp_dir}/${asset}"
curl -fsSL "${base_url}/${asset}.sha256" -o "${tmp_dir}/${asset}.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmp_dir" && sha256sum -c "${asset}.sha256")
else
  expected="$(awk '{print $1}' "${tmp_dir}/${asset}.sha256")"
  actual="$(shasum -a 256 "${tmp_dir}/${asset}" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for ${asset}" >&2
    exit 1
  fi
fi

tar -xzf "${tmp_dir}/${asset}" -C "$tmp_dir"
mkdir -p "$install_dir"
install -m 0755 "${tmp_dir}/hivemind" "${install_dir}/hivemind"

echo "installed hivemind to ${install_dir}/hivemind"
