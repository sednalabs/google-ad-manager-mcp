#!/bin/sh
set -eu

repository="sednalabs/google-ad-manager-mcp"
version="latest"
install_dir="${XDG_BIN_HOME:-${HOME}/.local/bin}"

usage() {
  cat <<'EOF'
Install a checksum-verified google-ad-manager-mcp GitHub release.

Usage: install.sh [--version v0.1.1] [--install-dir DIRECTORY]
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || { echo "--version requires a value" >&2; exit 2; }
      version="$2"
      shift 2
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || { echo "--install-dir requires a value" >&2; exit 2; }
      install_dir="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "tar is required" >&2; exit 1; }

if [ "$version" = "latest" ]; then
  latest_url=$(curl --proto '=https' --proto-redir '=https' -fsSIL -o /dev/null -w '%{url_effective}' \
    "https://github.com/${repository}/releases/latest")
  version=${latest_url##*/}
fi

if ! printf '%s\n' "$version" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?(\+[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$'; then
  echo "release version must be a complete semantic version such as v0.1.1" >&2
  exit 2
fi

os=$(uname -s)
arch=$(uname -m)
case "${os}/${arch}" in
  Linux/x86_64|Linux/amd64) target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin/arm64|Darwin/aarch64) target="aarch64-apple-darwin" ;;
  Darwin/x86_64|Darwin/amd64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported platform: ${os}/${arch}" >&2; exit 1 ;;
esac

asset="google-ad-manager-mcp-${version}-${target}.tar.gz"
bundle_dir="google-ad-manager-mcp-${version}-${target}"
base_url="https://github.com/${repository}/releases/download/${version}"
tmp_dir=$(mktemp -d)
cleanup() { rm -rf "$tmp_dir"; }
trap cleanup EXIT HUP INT TERM

curl --proto '=https' --proto-redir '=https' -fsSL "${base_url}/${asset}" -o "${tmp_dir}/${asset}"
curl --proto '=https' --proto-redir '=https' -fsSL "${base_url}/SHA256SUMS" -o "${tmp_dir}/SHA256SUMS"

expected=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "${tmp_dir}/SHA256SUMS")
[ -n "$expected" ] || { echo "checksum for ${asset} not found" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "${tmp_dir}/${asset}" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "${tmp_dir}/${asset}" | awk '{print $1}')
else
  echo "sha256sum or shasum is required" >&2
  exit 1
fi
[ "$actual" = "$expected" ] || { echo "checksum verification failed for ${asset}" >&2; exit 1; }

tar -xzf "${tmp_dir}/${asset}" -C "$tmp_dir" "${bundle_dir}/google-ad-manager-mcp"
binary="${tmp_dir}/${bundle_dir}/google-ad-manager-mcp"
[ -f "$binary" ] || { echo "release archive did not contain google-ad-manager-mcp" >&2; exit 1; }
mkdir -p "$install_dir"
install -m 0755 "$binary" "${install_dir}/google-ad-manager-mcp"

echo "Installed google-ad-manager-mcp ${version} to ${install_dir}/google-ad-manager-mcp"
echo "Use this executable as the command in your MCP client configuration."
case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) echo "Add ${install_dir} to PATH to run google-ad-manager-mcp." ;;
esac
