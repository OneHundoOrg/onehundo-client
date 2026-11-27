#!/usr/bin/env bash
# Fail-open bridge wrapper for community recompilation launchers (Linux / SteamOS / macOS).
# Example:
#   onehundo-recomp-bridge.sh --launcher longshot --game-id nintendo.oot.switch2 \
#     --save-dir "$HOME/.local/share/longshot" -- ./longshot "$@"

set -u

HEALTH_URL="${ONEHUNDO_HEALTH_URL:-http://127.0.0.1:19850}"
LAUNCHER="recomp_launcher"
GAME_ID=""
APP_ID=""
SAVE_DIR=""
INSTALL_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --launcher) LAUNCHER="${2:-}"; shift 2 ;;
    --game-id) GAME_ID="${2:-}"; shift 2 ;;
    --app-id) APP_ID="${2:-}"; shift 2 ;;
    --save-dir) SAVE_DIR="${2:-}"; shift 2 ;;
    --install-dir) INSTALL_DIR="${2:-}"; shift 2 ;;
    --) shift; break ;;
    *) break ;;
  esac
done

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 [--launcher name] [--game-id id] [--save-dir path] -- <command...>" >&2
  exit 2
fi

EXE_PATH="$1"
OS_TAG="linux"
case "$(uname -s)" in
  Darwin) OS_TAG="macos" ;;
  Linux)
    if [[ "${SteamDeck:-0}" == "1" || -f /etc/os-release && "$(grep -qi steamos /etc/os-release && echo 1 || echo 0)" == "1" ]]; then
      OS_TAG="steamos"
    fi
    ;;
esac

post_report() {
  local active_flag="$1"
  command -v curl >/dev/null 2>&1 || return 0
  python3 -c '
import json, sys, urllib.request
payload = {
    "launcher": sys.argv[1],
    "canonical_game_id": sys.argv[2] or None,
    "app_id": sys.argv[3] or None,
    "executable": sys.argv[4] or None,
    "install_dir": sys.argv[5] or None,
    "save_dir": sys.argv[6] or None,
    "os": sys.argv[7],
    "active": sys.argv[8] == "true",
}
req = urllib.request.Request(
    sys.argv[9].rstrip("/") + "/v1/launchers/report",
    data=json.dumps(payload).encode("utf-8"),
    headers={"Content-Type": "application/json"},
    method="POST",
)
try:
    urllib.request.urlopen(req, timeout=1.5).read()
except Exception:
    pass
' "$LAUNCHER" "$GAME_ID" "$APP_ID" "$EXE_PATH" "$INSTALL_DIR" "$SAVE_DIR" "$OS_TAG" "$active_flag" "$HEALTH_URL" >/dev/null 2>&1 || true
}

post_report "true"
"$@"
RC=$?
post_report "false"
if command -v curl >/dev/null 2>&1; then
  curl -fsS -m 1 -X POST -H "Content-Type: application/json" -d '{}' "${HEALTH_URL}/watches/poll" >/dev/null 2>&1 || true
fi
exit $RC
