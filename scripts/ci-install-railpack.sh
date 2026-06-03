#!/usr/bin/env bash
set -euo pipefail

version="${HOSTLET_RAILPACK_VERSION:-0.25.0}"
install_dir="${HOSTLET_RAILPACK_INSTALL_DIR:-${RUNNER_TEMP:-/tmp}/hostlet-railpack-bin}"

case "$(uname -m)" in
  x86_64) arch="x86_64"; sha="1a3e471b8b5a2f214164fe2217a3e834ef921ee1a277fd70108a51c8cb42b6cf" ;;
  aarch64|arm64) arch="arm64"; sha="e428c9a3bd7d237f4b53d683e67204c366868304c4f1b4d01befdfff15215b5c" ;;
  *)
    echo "unsupported Railpack CI architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

mkdir -p "${install_dir}"
archive="${install_dir}/railpack.tgz"
url="https://github.com/railwayapp/railpack/releases/download/v${version}/railpack-v${version}-${arch}-unknown-linux-musl.tar.gz"

curl -fsSL "${url}" -o "${archive}"
echo "${sha}  ${archive}" | sha256sum -c -
tar -xzf "${archive}" -C "${install_dir}"
chmod +x "${install_dir}/railpack"
echo "HOSTLET_RAILPACK_BIN=${install_dir}/railpack" >> "${GITHUB_ENV}"
"${install_dir}/railpack" --version
