#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT}/dist"
APPIMAGE_NAME="OneHundo-x86_64.AppImage"
APPIMAGE_PATH="${DIST_DIR}/${APPIMAGE_NAME}"
SHA_PATH="${APPIMAGE_PATH}.sha256"

VERSION="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "${ROOT}/Cargo.toml" | head -n 1)"
if [[ -z "${VERSION}" ]]; then
  echo "Error: could not read package version from ${ROOT}/Cargo.toml" >&2
  exit 1
fi

ARCH="$(uname -m)"
if [[ "${ARCH}" != "x86_64" ]]; then
  echo "Warning: building AppImage on host architecture ${ARCH}; set ARCH=x86_64 if cross-compiling." >&2
fi

echo "==> Building OneHundo Linux x86_64 AppImage v${VERSION}"

CARGO_HOME_HOST="${CARGO_HOME:-${HOME}/.cargo}"
OFFLINE_FLAG=""
if [[ -d "${CARGO_HOME_HOST}/registry" ]]; then
  OFFLINE_FLAG="--offline"
fi

(
  cd "${ROOT}"
  cargo build --release ${OFFLINE_FLAG} --bins
)

BIN_DIR="${ROOT}/target/release"
DESKTOP_BIN="${BIN_DIR}/onehundo-desktop"
DAEMON_BIN="${BIN_DIR}/onehundod"

for bin in "${DESKTOP_BIN}" "${DAEMON_BIN}"; do
  if [[ ! -x "${bin}" ]]; then
    echo "Error: missing compiled binary ${bin}" >&2
    exit 1
  fi
done

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "${STAGE_DIR}"' EXIT
APPDIR="${STAGE_DIR}/OneHundo.AppDir"

mkdir -p \
  "${APPDIR}/usr/bin" \
  "${APPDIR}/usr/share/onehundo/packs" \
  "${APPDIR}/usr/share/icons/hicolor/256x256/apps" \
  "${APPDIR}/usr/share/applications"

cp "${DESKTOP_BIN}" "${APPDIR}/usr/bin/onehundo-desktop"
cp "${DAEMON_BIN}" "${APPDIR}/usr/bin/onehundod"
chmod 0755 "${APPDIR}/usr/bin/onehundo-desktop" "${APPDIR}/usr/bin/onehundod"

if command -v strip >/dev/null 2>&1; then
  strip --strip-all "${APPDIR}/usr/bin/onehundo-desktop" "${APPDIR}/usr/bin/onehundod"
fi

if [[ -d "${ROOT}/packs" ]] && compgen -G "${ROOT}/packs/*.onehundo" >/dev/null; then
  cp "${ROOT}/packs/"*.onehundo "${APPDIR}/usr/share/onehundo/packs/"
  chmod 0644 "${APPDIR}/usr/share/onehundo/packs/"*.onehundo
fi

ICON_SRC="${ROOT}/ui/desktop/assets/onehundo_256.png"
if [[ ! -f "${ICON_SRC}" ]]; then
  echo "Error: missing application icon at ${ICON_SRC}" >&2
  exit 1
fi
cp "${ICON_SRC}" "${APPDIR}/onehundo.png"
cp "${ICON_SRC}" "${APPDIR}/.DirIcon"
cp "${ICON_SRC}" "${APPDIR}/usr/share/icons/hicolor/256x256/apps/onehundo.png"
chmod 0644 "${APPDIR}/onehundo.png" "${APPDIR}/.DirIcon" "${APPDIR}/usr/share/icons/hicolor/256x256/apps/onehundo.png"

cat > "${APPDIR}/onehundo.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=OneHundo
GenericName=Achievement Tracker
Comment=Automatic achievement tracking for PC ports, decomps, and emulators
Exec=onehundo-desktop
Icon=onehundo
Terminal=false
Categories=Game;Utility;
StartupWMClass=OneHundo
EOF
chmod 0644 "${APPDIR}/onehundo.desktop"
cp "${APPDIR}/onehundo.desktop" "${APPDIR}/usr/share/applications/onehundo.desktop"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "${APPDIR}/onehundo.desktop"
fi

cat > "${APPDIR}/AppRun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

APPDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export APPDIR
export PATH="${APPDIR}/usr/bin:${PATH}"
if [[ -z "${ONEHUNDO_PACKS_DIR:-}" && -d "${APPDIR}/usr/share/onehundo/packs" ]]; then
  export ONEHUNDO_PACKS_DIR="${APPDIR}/usr/share/onehundo/packs"
fi
exec "${APPDIR}/usr/bin/onehundo-desktop" "$@"
EOF
chmod 0755 "${APPDIR}/AppRun"

resolve_appimagetool() {
  if [[ -n "${APPIMAGETOOL:-}" && -x "${APPIMAGETOOL}" ]]; then
    echo "${APPIMAGETOOL}"
    return 0
  fi
  if command -v appimagetool >/dev/null 2>&1; then
    command -v appimagetool
    return 0
  fi

  local cache_dir="${XDG_CACHE_HOME:-${HOME}/.cache}/onehundo"
  local cached="${cache_dir}/appimagetool-x86_64.AppImage"
  if [[ ! -x "${cached}" ]]; then
    mkdir -p "${cache_dir}"
    local url="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
    echo "==> Downloading appimagetool-x86_64.AppImage to ${cached}" >&2
    curl -fSL --retry 3 --connect-timeout 15 "${url}" -o "${cached}.tmp"
    chmod 0755 "${cached}.tmp"
    mv "${cached}.tmp" "${cached}"
  fi
  echo "${cached}"
}

APPIMAGETOOL_BIN="$(resolve_appimagetool)"
mkdir -p "${DIST_DIR}"
rm -f "${APPIMAGE_PATH}" "${SHA_PATH}"

echo "==> Packing ${APPIMAGE_PATH} with ${APPIMAGETOOL_BIN}"
if ! ARCH=x86_64 "${APPIMAGETOOL_BIN}" --no-appstream "${APPDIR}" "${APPIMAGE_PATH}" 2>/dev/null; then
  ARCH=x86_64 "${APPIMAGETOOL_BIN}" --appimage-extract-and-run --no-appstream "${APPDIR}" "${APPIMAGE_PATH}"
fi
chmod 0755 "${APPIMAGE_PATH}"

(
  cd "${DIST_DIR}"
  sha256sum "${APPIMAGE_NAME}" > "${APPIMAGE_NAME}.sha256"
)

PACK_COUNT="$(find "${APPDIR}/usr/share/onehundo/packs" -maxdepth 1 -name '*.onehundo' | wc -l | tr -d ' ')"
echo "==> Packaged ${APPIMAGE_PATH} ($(wc -c < "${APPIMAGE_PATH}") bytes, ${PACK_COUNT} bundled packs)"
cat "${SHA_PATH}"
