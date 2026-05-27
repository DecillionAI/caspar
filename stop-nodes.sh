#!/usr/bin/env bash
# =============================================================================
# stop-nodes.sh — gracefully shut down everything started by run-nodes.sh
#
# Stops, in order:
#   1. Docker containers   caspar-node1/2/3   (graceful SIGTERM, 10s grace)
#   2. Sibling VM containers caspar-node spawned (anything matching a Caspar
#      label / name pattern) — best-effort
#   3. Local caspar-node binaries (SIGTERM, then SIGKILL after 5s)
#   4. QuestDB processes
#
# Usage:
#   ./stop-nodes.sh                shutdown everything
#   ./stop-nodes.sh --clean        also wipe /tmp/caspar/node*/{db,storage,…}
#                                  state (keeps .env keys)
#   ./stop-nodes.sh --purge        wipe ALL of /tmp/caspar (incl. keys)
#   ./stop-nodes.sh --keep-questdb leave QuestDB running
# =============================================================================

set -euo pipefail

DATA_ROOT="/tmp/caspar"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info() { echo -e "${CYAN}[stop]${NC} $*"; }
ok()   { echo -e "${GREEN}[stop]${NC} $*"; }
warn() { echo -e "${YELLOW}[stop]${NC} $*"; }

CLEAN=false
PURGE=false
KEEP_QUESTDB=false

for arg in "$@"; do
  case "$arg" in
    --clean)         CLEAN=true ;;
    --purge)         PURGE=true ;;
    --keep-questdb)  KEEP_QUESTDB=true ;;
    --help|-h)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      warn "Unknown argument: $arg"
      ;;
  esac
done

# ─── 1. Docker caspar-node containers ────────────────────────────────────────
if command -v docker &>/dev/null; then
  for n in 1 2 3; do
    container="caspar-node${n}"
    if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^${container}$"; then
      info "Stopping $container (SIGTERM, 10s grace)…"
      docker stop --time 10 "$container" >/dev/null 2>&1 || true
      docker rm "$container" >/dev/null 2>&1 || true
      ok "$container removed"
    fi
  done

  # ─── 2. Sibling VM containers spawned by caspar (best-effort) ──────────────
  # Caspar names docker-backed VMs with predictable prefixes; clean those up
  # so they don't leak across runs. Adjust the grep if your naming differs.
  vm_containers=$(docker ps -aq --filter "label=caspar.vm=1" 2>/dev/null || true)
  if [[ -n "$vm_containers" ]]; then
    info "Removing $(echo "$vm_containers" | wc -l) sibling VM container(s)…"
    docker rm -f $vm_containers >/dev/null 2>&1 || true
  fi
else
  warn "docker not installed — skipping container shutdown"
fi

# ─── 3. Local caspar-node processes ──────────────────────────────────────────
pids=$(ps -eo pid,cmd 2>/dev/null \
  | awk '/caspar-node/ && !/awk/ && !/grep/ && !/run-nodes/ && !/stop-nodes/ {print $1}')
if [[ -n "$pids" ]]; then
  info "Sending SIGTERM to local caspar-node processes: $pids"
  for p in $pids; do kill "$p" 2>/dev/null || true; done
  sleep 5
  # Anything still alive gets SIGKILL
  still=$(ps -eo pid,cmd 2>/dev/null \
    | awk '/caspar-node/ && !/awk/ && !/grep/ && !/run-nodes/ && !/stop-nodes/ {print $1}')
  if [[ -n "$still" ]]; then
    warn "SIGKILLing stragglers: $still"
    for p in $still; do kill -9 "$p" 2>/dev/null || true; done
  fi
  ok "Local caspar-node processes stopped"
else
  info "No local caspar-node processes"
fi

# ─── 4. QuestDB ──────────────────────────────────────────────────────────────
if ! $KEEP_QUESTDB; then
  qpids=$(ps -eo pid,cmd 2>/dev/null | awk '/questdb/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$qpids" ]]; then
    info "Stopping QuestDB processes: $qpids"
    for p in $qpids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    still=$(ps -eo pid,cmd 2>/dev/null | awk '/questdb/ && !/awk/ && !/grep/ {print $1}')
    if [[ -n "$still" ]]; then
      for p in $still; do kill -9 "$p" 2>/dev/null || true; done
    fi
    ok "QuestDB stopped"
  else
    info "No QuestDB processes"
  fi
else
  info "Leaving QuestDB running (--keep-questdb)"
fi

# ─── State cleanup ───────────────────────────────────────────────────────────
if $PURGE; then
  warn "--purge: wiping $DATA_ROOT entirely (including .env keys)"
  rm -rf "$DATA_ROOT"
  ok "All state purged"
elif $CLEAN; then
  warn "--clean: wiping per-node state (keeping .env / keys)"
  for n in 1 2 3; do
    node_dir="$DATA_ROOT/node${n}"
    [[ -d "$node_dir" ]] || continue
    rm -rf "$node_dir"/{db,applet,search,store_logs,storage,telemetry,babble,*.log,*.log.prev,*.pid,.env.docker}
    ok "Cleaned $node_dir"
  done
  # QuestDB data
  rm -rf "$DATA_ROOT/questdb" "$DATA_ROOT/questdb.log" 2>/dev/null || true
fi

echo ""
ok "Shutdown complete."
