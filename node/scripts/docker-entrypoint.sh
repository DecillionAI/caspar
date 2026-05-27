#!/usr/bin/env bash
# docker-entrypoint.sh — Container startup for caspar-node
#
# Mount a .env file at /app/.env, or pass all variables via `docker run -e`.
# Required env vars: OWNER_ID, OWNER_PRIVATE_KEY, CLIENT_TCP_API_PORT, etc.

set -e

# Source mounted env file if present
if [[ -f /app/.env ]]; then
  set -a
  # shellcheck disable=SC1091
  source /app/.env
  set +a
fi

# Create directories that must exist before the node starts
mkdir -p \
  "${STORAGE_ROOT_PATH:-/app/data/storage}" \
  "${BASE_DB_PATH:-/app/data/db}" \
  "${APPLET_DB_PATH:-/app/data/applet}" \
  "${SEARCH_INDEX_PATH:-/app/data/search}" \
  "${STORE_LOGS_DB:-/app/data/store_logs}" \
  "${TELEMETRY_DB_PATH:-/app/data/telemetry}" \
  "${BABBLE_DIR:-/app/data/babble}"

# Default BABBLE_DIR if not set (caspar-node requires it)
export BABBLE_DIR="${BABBLE_DIR:-/app/data/babble}"

exec /usr/local/bin/caspar-node "$@"
