#!/usr/bin/env bash
# run-nodes.sh — unified Caspar node runner
#
# Usage:
#   ./run-nodes.sh [single|triple] [--no-questdb] [--fresh] [--no-gvisor] [--help]
#
# Modes:
#   single  — run only node1 (default when --single or no arg given for quick dev)
#   triple  — run node1 + node2 + node3 (default with no args)
#
# Options:
#   --no-questdb   skip QuestDB startup (useful if you manage it separately)
#   --fresh        wipe /tmp/caspar/* before starting (clean-slate run)
#   --no-gvisor    skip gVisor (runsc) install / configuration. By default
#                  gVisor is installed and registered with Docker so all
#                  caspar VMs run sandboxed. Auto-skipped if Docker is absent.
#   --help         show this help
#
# Requirements (auto-checked, missing deps printed with install hint):
#   - Rust + cargo            (build node binary)
#   - Java 11+                (run QuestDB)
#   - /opt/questdb/questdb.jar (QuestDB server jar, placed by provisioning)

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NODE_DIR="$REPO_DIR/node"
BINARY="$NODE_DIR/target/release/caspar-node"
DATA_ROOT="/tmp/caspar"
QUESTDB_JAR="/opt/questdb/questdb.jar"
QUESTDB_DATA="$DATA_ROOT/questdb"
QUESTDB_PORT=8812

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info()  { echo -e "${CYAN}[caspar]${NC} $*"; }
ok()    { echo -e "${GREEN}[caspar]${NC} $*"; }
warn()  { echo -e "${YELLOW}[caspar]${NC} $*"; }
die()   { echo -e "${RED}[caspar] FATAL:${NC} $*" >&2; exit 1; }

# ─── Arg parsing ─────────────────────────────────────────────────────────────
MODE="triple"
START_QUESTDB=true
FRESH=false
SETUP_GVISOR=true

for arg in "$@"; do
  case "$arg" in
    single)        MODE="single" ;;
    triple)        MODE="triple" ;;
    --no-questdb)  START_QUESTDB=false ;;
    --fresh)       FRESH=true ;;
    --no-gvisor)   SETUP_GVISOR=false ;;
    --help|-h)
      sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "Unknown argument: $arg" ;;
  esac
done

NODES=(1)
[[ "$MODE" == "triple" ]] && NODES=(1 2 3)

# ─── Dependency checks ───────────────────────────────────────────────────────
check_dep() {
  local name="$1" cmd="$2" hint="$3"
  if ! command -v "$cmd" &>/dev/null; then
    die "Missing dependency: $name\n  Install: $hint"
  fi
}

info "Checking dependencies…"
check_dep "Rust/cargo" "cargo" "curl https://sh.rustup.rs -sSf | sh"

if $START_QUESTDB; then
  check_dep "java" "java" "apt-get install -y default-jre  OR  sdk install java"
  if [[ ! -f "$QUESTDB_JAR" ]]; then
    # Try to download if curl is available
    if command -v curl &>/dev/null; then
      warn "QuestDB jar not found at $QUESTDB_JAR — downloading…"
      mkdir -p /opt/questdb
      QDB_VER="8.3.1"
      QDB_URL="https://github.com/questdb/questdb/releases/download/$QDB_VER/questdb-$QDB_VER-no-jre-bin.tar.gz"
      curl -fsSL "$QDB_URL" -o /tmp/questdb.tar.gz
      tar -xzf /tmp/questdb.tar.gz -C /opt/questdb --strip-components=1
      mv /opt/questdb/questdb.jar "$QUESTDB_JAR" 2>/dev/null || true
      rm -f /tmp/questdb.tar.gz
      ok "QuestDB downloaded to $QUESTDB_JAR"
    else
      die "QuestDB jar not found: $QUESTDB_JAR\n  Download from https://questdb.io/get-questdb/ and place at $QUESTDB_JAR"
    fi
  fi
fi

java_ok=false
if $START_QUESTDB && command -v java &>/dev/null; then
  jver=$(java -version 2>&1 | grep -oP '(?<=version ")[0-9]+' | head -1)
  if [[ -z "$jver" ]]; then
    jver=$(java -version 2>&1 | grep -oP '"[0-9]+\.' | grep -oP '[0-9]+')
  fi
  if [[ "${jver:-0}" -ge 11 ]]; then
    java_ok=true
  else
    warn "Java $jver found but QuestDB needs Java 11+. Skipping QuestDB."
    START_QUESTDB=false
  fi
fi

# ─── gVisor (runsc) check / install (default ON) ─────────────────────────────
# Docker-backed VMs spawned by caspar-node run under --runtime=runsc for
# kernel-level sandboxing. gVisor is installed by default; pass --no-gvisor
# to skip. No-op when Docker is not installed.
if ! $SETUP_GVISOR; then
  info "Skipping gVisor setup (--no-gvisor)"
elif ! command -v docker &>/dev/null; then
  warn "Docker not installed — skipping gVisor setup (nothing to register runsc with)"
elif command -v runsc &>/dev/null && docker info 2>/dev/null | grep -q runsc; then
  ok "gVisor (runsc) installed and registered with Docker"
else
  info "Installing gVisor (runsc)…"
  if [[ "$EUID" -eq 0 ]]; then
    bash "$REPO_DIR/node/scripts/install-gvisor.sh"
  elif command -v sudo &>/dev/null; then
    sudo bash "$REPO_DIR/node/scripts/install-gvisor.sh"
  else
    warn "Cannot install gVisor: need root (no sudo available)."
    warn "  Run as root, or pass --no-gvisor to skip."
  fi
fi

ok "All dependency checks passed"

# ─── Fresh start ─────────────────────────────────────────────────────────────
if $FRESH; then
  warn "--fresh: wiping $DATA_ROOT"
  rm -rf "$DATA_ROOT"
fi

mkdir -p "$DATA_ROOT"

# ─── Stop existing processes ─────────────────────────────────────────────────
stop_existing() {
  local pids
  pids=$(ps -eo pid,cmd 2>/dev/null | awk '/caspar-node/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$pids" ]]; then
    info "Stopping existing caspar-node processes: $pids"
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    for p in $pids; do kill -9 "$p" 2>/dev/null || true; done
  fi

  local jpids
  jpids=$(ps -eo pid,cmd 2>/dev/null | awk '/questdb/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$jpids" ]]; then
    info "Stopping existing QuestDB processes: $jpids"
    for p in $jpids; do kill "$p" 2>/dev/null || true; done
    sleep 1
  fi
}
stop_existing

# ─── Build binary if needed ──────────────────────────────────────────────────
build_needed=false
if [[ ! -f "$BINARY" ]]; then
  build_needed=true
  info "Binary not found — building (this takes ~3 min first time)…"
elif [[ "$BINARY" -ot "$NODE_DIR/src/main.rs" ]]; then
  build_needed=true
  info "Source newer than binary — rebuilding…"
fi

if $build_needed; then
  cd "$NODE_DIR"
  cargo build --release 2>&1 | grep -E "^error|Compiling caspar|Finished" || true
  cd "$REPO_DIR"
fi

[[ -f "$BINARY" ]] || die "Build failed: binary not found at $BINARY"
ok "Binary ready: $BINARY ($(ls -lh "$BINARY" | awk '{print $5}'))"

# ─── Start QuestDB ───────────────────────────────────────────────────────────
wait_for_port() {
  local host="$1" port="$2" name="$3" timeout="${4:-30}"
  local elapsed=0
  while ! python3 -c "import socket,sys; s=socket.socket(); s.settimeout(1); s.connect(('$host',$port)); s.close()" 2>/dev/null; do
    sleep 1; elapsed=$((elapsed+1))
    if [[ $elapsed -ge $timeout ]]; then
      return 1
    fi
  done
  return 0
}

if $START_QUESTDB; then
  mkdir -p "$QUESTDB_DATA"
  info "Starting QuestDB on port $QUESTDB_PORT…"
  java -jar "$QUESTDB_JAR" -m io.questdb/io.questdb.ServerMain \
       -d "$QUESTDB_DATA" >> "$DATA_ROOT/questdb.log" 2>&1 &
  QUESTDB_PID=$!

  if wait_for_port localhost $QUESTDB_PORT QuestDB 40; then
    ok "QuestDB ready (pid $QUESTDB_PID)"
  else
    warn "QuestDB did not come up within 40s — nodes may log telemetry errors but will still function"
  fi
fi

# ─── Per-node config generator ───────────────────────────────────────────────
# Each node has its own keypair and ports. In a real network these would be
# on separate machines; here we offset ports by 100 per node.
#
# NODE LAYOUT:
#  node1: TCP=8074  WS=8076  FED=8077  CHAIN=8078  ENTITY=8079  VM=8080  TELEMETRY=9099
#  node2: TCP=8174  WS=8176  FED=8177  CHAIN=8178  ENTITY=8179  VM=8180  TELEMETRY=9199
#  node3: TCP=8274  WS=8276  FED=8277  CHAIN=8278  ENTITY=8279  VM=8280  TELEMETRY=9299

start_node() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  local log_file="$node_dir/node.log"

  mkdir -p "$node_dir"/{storage,db,applet,search,store_logs,telemetry,babble}

  # Source node .env if it exists (created during provisioning), otherwise
  # rely on the environment already being set by the caller / CI.
  if [[ -f "$env_file" ]]; then
    # shellcheck disable=SC1090
    set -a; source "$env_file"; set +a
  else
    warn "No $env_file found — node$n may not have keys configured"
  fi

  info "Starting node$n (TCP=CLIENT_TCP_API_PORT)…"
  BABBLE_DIR="$node_dir/babble" \
    "$BINARY" >> "$log_file" 2>&1 &
  echo $! > "$node_dir/caspar.pid"
  echo $!
}

# ─── Launch nodes ────────────────────────────────────────────────────────────
STARTED_PIDS=()
for n in "${NODES[@]}"; do
  pid=$(start_node "$n")
  STARTED_PIDS+=("$pid")
done

# ─── Wait for nodes to accept connections ────────────────────────────────────
info "Waiting for node(s) to accept connections…"
declare -A NODE_PORTS=([1]=8074 [2]=8174 [3]=8274)
all_up=true
for n in "${NODES[@]}"; do
  port=${NODE_PORTS[$n]}
  if wait_for_port localhost "$port" "node$n" 20; then
    ok "node$n up on TCP port $port"
  else
    warn "node$n did not listen on port $port within 20s — check $DATA_ROOT/node${n}/node.log"
    all_up=false
  fi
done

echo ""
if $all_up; then
  ok "All ${#NODES[@]} node(s) running."
else
  warn "Some node(s) may not have started correctly. Check logs in $DATA_ROOT/node*/node.log"
fi

echo ""
echo "  Mode:    $MODE (${#NODES[@]} node(s))"
echo "  Binary:  $BINARY"
echo "  Data:    $DATA_ROOT"
[[ $START_QUESTDB == true ]] && echo "  QuestDB: localhost:$QUESTDB_PORT (http: localhost:9000)"
echo ""
echo "  Logs:"
for n in "${NODES[@]}"; do
  echo "    node$n → $DATA_ROOT/node${n}/node.log"
done
echo ""
echo "  Stop all:  kill \$(cat $DATA_ROOT/node*/caspar.pid 2>/dev/null)"
echo ""

# Keep script alive so Ctrl-C stops everything cleanly
trap 'info "Shutting down…"; for p in "${STARTED_PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; exit 0' INT TERM
info "Press Ctrl-C to stop all nodes."
wait
