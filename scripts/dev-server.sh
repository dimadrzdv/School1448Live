#!/usr/bin/env bash
# Local development server. Data lives in ./data-dev (git-ignored).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export DATA_DIR="${DATA_DIR:-./data-dev}"
export ADMIN_CODE="${ADMIN_CODE:-school1448}"   # dev only; production uses ADMIN_CODE_HASH
export COOKIE_SECURE="${COOKIE_SECURE:-false}"
export HTTP_ADDR="${HTTP_ADDR:-0.0.0.0:8080}"
export EGRESS_LIMIT_MBPS="${EGRESS_LIMIT_MBPS:-200}"
# The default route may go through a VPN; media must use the real LAN interface.
if [[ -z "${MEDIA_LAN_IP:-}" ]] && command -v ipconfig >/dev/null; then
  MEDIA_LAN_IP="$(ipconfig getifaddr en0 || ipconfig getifaddr en1 || true)"
  export MEDIA_LAN_IP
fi
export RUST_LOG="${RUST_LOG:-schoollive=info,str0m=warn}"
exec cargo run --release -p schoollive
