#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-linux-musl"
BUNDLE_NAME="onehundo-linux-x86_64"
DIST_DIR="${ROOT}/dist"
ARCHIVE_PATH="${DIST_DIR}/${BUNDLE_NAME}.tar.gz"
SHA_PATH="${ARCHIVE_PATH}.sha256"
CONTAINER_IMAGE="${ONEHUNDO_MUSL_IMAGE:-docker.io/library/rust:alpine}"

VERSION="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "${ROOT}/Cargo.toml" | head -n 1)"
if [[ -z "${VERSION}" ]]; then
  echo "Error: could not read package version from ${ROOT}/Cargo.toml" >&2
  exit 1
fi

echo "==> Building OneHundo Linux x86_64 release bundle v${VERSION} (${TARGET})"

CARGO_HOME_HOST="${CARGO_HOME:-${HOME}/.cargo}"
OFFLINE_FLAG=""
if [[ -d "${CARGO_HOME_HOST}/registry" ]]; then
  OFFLINE_FLAG="--offline"
fi

build_native_musl() {
  local cc_bin=""
  if command -v x86_64-linux-musl-gcc >/dev/null 2>&1; then
    cc_bin="x86_64-linux-musl-gcc"
  elif command -v musl-gcc >/dev/null 2>&1; then
    cc_bin="musl-gcc"
  else
    return 1
  fi

  if ! command -v rustup >/dev/null 2>&1; then
    return 1
  fi
  if ! rustup target list --installed 2>/dev/null | grep -qx "${TARGET}"; then
    return 1
  fi

  echo "==> Using native musl toolchain (${cc_bin})"
  (
    cd "${ROOT}"
    CC_x86_64_unknown_linux_musl="${cc_bin}" \
    RUSTFLAGS="-C target-feature=+crt-static" \
      cargo build --release ${OFFLINE_FLAG} --target "${TARGET}" --bins
  )
}

build_container_musl() {
  local runtime=""
  if command -v podman >/dev/null 2>&1; then
    runtime="podman"
  elif command -v docker >/dev/null 2>&1; then
    runtime="docker"
  else
    echo "Error: neither native musl toolchain (rustup + musl-gcc) nor podman/docker was found." >&2
    exit 1
  fi

  echo "==> Using ${runtime} container (${CONTAINER_IMAGE}) for reproducible static musl build"
  local extra_flags=()
  if [[ "${runtime}" == "podman" ]]; then
    extra_flags+=(--userns=keep-id --security-opt label=disable)
  fi

  local cargo_mounts=()
  if [[ -d "${CARGO_HOME_HOST}" ]]; then
    cargo_mounts+=(-v "${CARGO_HOME_HOST}:/tmp/cargo-home:ro" -e "CARGO_HOME=/tmp/cargo-home")
  fi

  "${runtime}" run --rm "${extra_flags[@]}" \
    -v "${ROOT}:/work" \
    "${cargo_mounts[@]}" \
    -e RUSTFLAGS="-C target-feature=+crt-static" \
    -w /work \
    "${CONTAINER_IMAGE}" \
    cargo build --release ${OFFLINE_FLAG} --target "${TARGET}" --bins
}

if [[ "${ONEHUNDO_USE_CONTAINER:-0}" != "1" ]] && build_native_musl; then
  true
else
  build_container_musl
fi

BIN_DIR="${ROOT}/target/${TARGET}/release"
DAEMON_BIN="${BIN_DIR}/onehundod"
DESKTOP_BIN="${BIN_DIR}/onehundo-desktop"

for bin in "${DAEMON_BIN}" "${DESKTOP_BIN}"; do
  if [[ ! -x "${bin}" ]]; then
    echo "Error: missing compiled binary ${bin}" >&2
    exit 1
  fi
done

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "${STAGE_DIR}"' EXIT

cp "${DAEMON_BIN}" "${STAGE_DIR}/onehundod"
cp "${DESKTOP_BIN}" "${STAGE_DIR}/onehundo-desktop"
chmod 0755 "${STAGE_DIR}/onehundod" "${STAGE_DIR}/onehundo-desktop"

if command -v strip >/dev/null 2>&1; then
  strip --strip-all "${STAGE_DIR}/onehundod" "${STAGE_DIR}/onehundo-desktop"
fi

echo "==> Verifying static binary linkage"
for bin in "${STAGE_DIR}/onehundod" "${STAGE_DIR}/onehundo-desktop"; do
  file_out="$(file "${bin}")"
  ldd_out="$(ldd "${bin}" 2>&1 || true)"
  echo "  $(basename "${bin}"): ${file_out}"
  echo "    ldd: ${ldd_out}"
  if ! grep -Eq 'statically linked|static-pie linked' <<<"${file_out}"; then
    echo "Error: ${bin} is not statically linked!" >&2
    exit 1
  fi
  if grep -Eq 'GLIBC_|libc\.so\.6|=> /' <<<"${ldd_out}"; then
    echo "Error: ${bin} has unexpected dynamic glibc dependencies!" >&2
    exit 1
  fi
done

mkdir -p "${STAGE_DIR}/packs" "${STAGE_DIR}/deploy/systemd"
if [[ -d "${ROOT}/packs" ]]; then
  cp "${ROOT}/packs/"*.onehundo "${STAGE_DIR}/packs/"
  chmod 0644 "${STAGE_DIR}/packs/"*.onehundo
fi
cp "${ROOT}/deploy/systemd/onehundod.service" "${STAGE_DIR}/deploy/systemd/onehundod.service"
chmod 0644 "${STAGE_DIR}/deploy/systemd/onehundod.service"

cat > "${STAGE_DIR}/install.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/onehundo"
PACKS_DIR="${DATA_DIR}/packs"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"

mkdir -p "${BIN_DIR}" "${PACKS_DIR}" "${SYSTEMD_USER_DIR}"

install -m 0755 "${SCRIPT_DIR}/onehundod" "${BIN_DIR}/onehundod"
install -m 0755 "${SCRIPT_DIR}/onehundo-desktop" "${BIN_DIR}/onehundo-desktop"

if compgen -G "${SCRIPT_DIR}/packs/*.onehundo" >/dev/null; then
  install -m 0644 "${SCRIPT_DIR}/packs/"*.onehundo "${PACKS_DIR}/"
fi

install -m 0644 "${SCRIPT_DIR}/deploy/systemd/onehundod.service" "${SYSTEMD_USER_DIR}/onehundod.service"

echo "Installed binaries to ${BIN_DIR}/onehundod and ${BIN_DIR}/onehundo-desktop"
echo "Installed packs to ${PACKS_DIR}"
echo "Installed systemd user unit to ${SYSTEMD_USER_DIR}/onehundod.service"

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
  systemctl --user daemon-reload
  systemctl --user enable --now onehundod.service
  systemctl --user restart onehundod.service || true
  echo "Enabled and started onehundod.service via systemd --user"
else
  echo "Note: systemd --user session is not active; skipped automatic unit enable."
  echo "Enable manually later with: systemctl --user enable --now onehundod.service"
  echo "Or start directly with: ${BIN_DIR}/onehundo-desktop start"
fi

case ":${PATH}:" in
  *":${BIN_DIR}:"*) ;;
  *)
    echo ""
    echo "Add ${BIN_DIR} to your PATH to run onehundod / onehundo-desktop directly:"
    echo "  export PATH=\"${BIN_DIR}:\$PATH\""
    ;;
esac

echo ""
echo "Next steps:"
echo "  1. Check daemon status: ${BIN_DIR}/onehundo-desktop status"
echo "  2. Open Game Health UI: http://127.0.0.1:19850"
echo "  3. Set your sync key:   ${BIN_DIR}/onehundo-desktop set-api-key <oh_live_...>"
EOF
chmod 0755 "${STAGE_DIR}/install.sh"

PACK_COUNT="$(find "${STAGE_DIR}/packs" -maxdepth 1 -name '*.onehundo' | wc -l | tr -d ' ')"

cat > "${STAGE_DIR}/README.txt" <<EOF
OneHundo Linux x86_64 Release Bundle (v${VERSION})
==================================================

Contents:
  - onehundod                         Local achievement daemon (static musl x86_64)
  - onehundo-desktop                  Desktop service & Game Health CLI (static musl x86_64)
  - packs/*.onehundo                  Bundled achievement packs (${PACK_COUNT} packs)
  - deploy/systemd/onehundod.service  systemd --user service unit
  - install.sh                        Installer script (~/.local/bin + packs + systemd unit)

Quick Install:
  tar -xzf ${BUNDLE_NAME}.tar.gz && ./install.sh

  (Or extract into a subfolder first:
   mkdir -p ${BUNDLE_NAME} && tar -xzf ${BUNDLE_NAME}.tar.gz -C ${BUNDLE_NAME} && cd ${BUNDLE_NAME} && ./install.sh)

Runtime Notes:
  - Both binaries are statically linked (zero glibc runtime dependency).
  - HTTPS cloud sync uses the system \`curl\` CLI when syncing with https://onehundo.org.
  - Desktop unlock notifications use \`notify-send\` (libnotify) when available.
  - Generate a daemon API key in your OneHundo account page, then run:
      ~/.local/bin/onehundo-desktop set-api-key <oh_live_...>
  - Local Game Health dashboard runs at: http://127.0.0.1:19850
EOF
chmod 0644 "${STAGE_DIR}/README.txt"

mkdir -p "${DIST_DIR}"
TAR_FLAGS=(--format=ustar)
if tar --version 2>/dev/null | grep -qi 'GNU tar'; then
  TAR_FLAGS+=(--sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner)
fi

# Archive the named members (never "."), so extracting into an existing folder such as
# ~/Downloads does not rewrite that folder's permissions with the mktemp 0700 mode.
chmod 0755 "${STAGE_DIR}"
MEMBERS=(README.txt install.sh onehundod onehundo-desktop deploy packs)
tar -C "${STAGE_DIR}" "${TAR_FLAGS[@]}" -cf - "${MEMBERS[@]}" | gzip -n -9 > "${ARCHIVE_PATH}"
(
  cd "${DIST_DIR}"
  sha256sum "${BUNDLE_NAME}.tar.gz" > "${BUNDLE_NAME}.tar.gz.sha256"
)

echo "==> Packaged ${ARCHIVE_PATH} ($(wc -c < "${ARCHIVE_PATH}") bytes)"
cat "${SHA_PATH}"
