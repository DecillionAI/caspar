#!/usr/bin/env bash
# =============================================================================
# bench-all.sh — Full Caspar stack setup + benchmark runner
#
# Performs the complete sequence in one command:
#   1. Dependency checks (auto-install where possible)
#   2. Build caspar-node binary if needed
#   3. Stop any stale processes
#   4. Start QuestDB and wait for readiness
#   5. Generate node key configs if missing
#   6. Start 1 or 3 Caspar nodes and wait for readiness
#   7. Deploy all WASM creature modules via deploy.py  (~8 min for 3 nodes)
#   8. Run the full benchmark suite (workflow_tests.py)
#   9. Print final results path
#
# Usage:
#   ./bench-all.sh [single|triple] [OPTIONS]
#
# Modes:
#   single   — run only node1 (quick dev/test, ~3 min deploy)
#   triple   — run node1 + node2 + node3 (default, full production-like setup)
#
# Options:
#   --fresh          Wipe /tmp/caspar before starting (forces re-deploy)
#   --no-questdb     Skip QuestDB startup
#   --skip-build     Skip cargo build even if binary is stale
#   --skip-deploy    Skip creature deployment (use existing deployment)
#   --skip-bench     Start nodes + deploy only, do not run benchmark
#   --deploy-only    Start nodes + deploy only (same as --skip-bench)
#   --help           Show this help
#
# Output:
#   /home/user/workflow_results.json   — raw results
#   /home/user/workflow_report.md      — human-readable report
# =============================================================================

set -euo pipefail

# ─── Paths ───────────────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$SCRIPT_DIR"
NODE_DIR="$REPO_DIR/node"
BINARY="$NODE_DIR/target/release/caspar-node"
DATA_ROOT="/tmp/caspar"
QUESTDB_JAR="/opt/questdb/questdb.jar"
QUESTDB_DATA="$DATA_ROOT/questdb"
QUESTDB_PORT=8812
HOME_DIR="/home/user"
DEPLOY_SCRIPT="$HOME_DIR/deploy.py"
BENCH_SCRIPT="$HOME_DIR/decillionai-server/bench/workflow_tests.py"
DEPLOY_REPORT="$HOME_DIR/deployment_report.json"
RESULT_JSON="$HOME_DIR/workflow_results.json"
RESULT_MD="$HOME_DIR/workflow_report.md"

# ─── Colours ─────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
info()  { echo -e "${CYAN}[bench-all]${NC} $*"; }
ok()    { echo -e "${GREEN}[bench-all]${NC} $*"; }
warn()  { echo -e "${YELLOW}[bench-all]${NC} $*"; }
die()   { echo -e "${RED}[bench-all] FATAL:${NC} $*" >&2; exit 1; }
step()  { echo -e "\n${BOLD}${CYAN}━━━ $* ━━━${NC}"; }

# ─── Arg parsing ─────────────────────────────────────────────────────────────
MODE="triple"
START_QUESTDB=true
FRESH=false
SKIP_BUILD=false
SKIP_DEPLOY=false
SKIP_BENCH=false

for arg in "$@"; do
  case "$arg" in
    single)        MODE="single" ;;
    triple)        MODE="triple" ;;
    --no-questdb)  START_QUESTDB=false ;;
    --fresh)       FRESH=true ;;
    --skip-build)  SKIP_BUILD=true ;;
    --skip-deploy) SKIP_DEPLOY=true ;;
    --skip-bench|--deploy-only) SKIP_BENCH=true ;;
    --help|-h)
      sed -n '2,35p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "Unknown argument: $arg" ;;
  esac
done

[[ "$MODE" == "single" ]] && NODES=(1) || NODES=(1 2 3)

# Ports for each node
declare -A NODE_TCP=([1]=8074 [2]=8174 [3]=8274)
declare -A NODE_WS=([1]=8076 [2]=8176 [3]=8276)
declare -A NODE_FED=([1]=8077 [2]=8177 [3]=8277)
declare -A NODE_CHAIN=([1]=8078 [2]=8178 [3]=8278)
declare -A NODE_ENTITY=([1]=8079 [2]=8179 [3]=8279)
declare -A NODE_VM=([1]=8080 [2]=8180 [3]=8280)
declare -A NODE_TEL=([1]=9099 [2]=9199 [3]=9299)

START_TIME=$SECONDS

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║         Caspar Full-Stack Benchmark Runner                  ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
info "Mode: $MODE (${#NODES[@]} node(s))"
info "Skip flags: build=$SKIP_BUILD deploy=$SKIP_DEPLOY bench=$SKIP_BENCH"
echo ""

# =============================================================================
# STEP 1 — Dependency checks
# =============================================================================
step "Step 1: Dependency checks"

check_cmd() {
  local name="$1" cmd="$2" hint="$3"
  if ! command -v "$cmd" &>/dev/null; then
    die "Missing dependency: $name\n  Install hint: $hint"
  fi
  ok "$name → $(command -v "$cmd")"
}

check_cmd "Rust/cargo" "cargo"  "curl https://sh.rustup.rs -sSf | sh"
check_cmd "Python 3"   "python3" "apt-get install -y python3"

# Python crypto library
if ! python3 -c "from Crypto.PublicKey import RSA" 2>/dev/null; then
  info "pycryptodome not found — installing…"
  pip3 install pycryptodome --quiet || die "pip3 install pycryptodome failed"
fi
ok "pycryptodome available"

if $START_QUESTDB; then
  check_cmd "java" "java" "apt-get install -y default-jre"

  jver=$(java -version 2>&1 | grep -oP '(?<=version ")[0-9]+' | head -1 || true)
  [[ -z "$jver" ]] && jver=$(java -version 2>&1 | grep -oP '"[0-9]+\.' | grep -oP '[0-9]+' || true)
  if [[ "${jver:-0}" -lt 11 ]]; then
    warn "Java $jver found but QuestDB needs Java 11+; skipping QuestDB."
    START_QUESTDB=false
  else
    ok "Java $jver"
  fi

  if [[ ! -f "$QUESTDB_JAR" ]]; then
    info "QuestDB jar not found at $QUESTDB_JAR — downloading…"
    mkdir -p /opt/questdb
    QDB_VER="8.3.1"
    curl -fsSL "https://github.com/questdb/questdb/releases/download/$QDB_VER/questdb-$QDB_VER-no-jre-bin.tar.gz" \
      -o /tmp/questdb.tar.gz
    tar -xzf /tmp/questdb.tar.gz -C /opt/questdb --strip-components=1
    mv /opt/questdb/questdb.jar "$QUESTDB_JAR" 2>/dev/null || true
    rm -f /tmp/questdb.tar.gz
    ok "QuestDB $QDB_VER downloaded to $QUESTDB_JAR"
  else
    ok "QuestDB jar at $QUESTDB_JAR"
  fi
fi

[[ -f "$DEPLOY_SCRIPT" ]] || die "deploy.py not found at $DEPLOY_SCRIPT"
ok "deploy.py at $DEPLOY_SCRIPT"

[[ -f "$BENCH_SCRIPT" ]] || die "workflow_tests.py not found at $BENCH_SCRIPT"
ok "workflow_tests.py at $BENCH_SCRIPT"

# =============================================================================
# STEP 2 — Build binary
# =============================================================================
step "Step 2: Build caspar-node binary"

if $SKIP_BUILD; then
  [[ -f "$BINARY" ]] || die "--skip-build given but binary not found at $BINARY"
  ok "Skipping build (--skip-build); binary: $BINARY"
else
  build_needed=false
  if [[ ! -f "$BINARY" ]]; then
    info "Binary not found — building from source (~3 min first time)…"
    build_needed=true
  elif find "$NODE_DIR/src" -name "*.rs" -newer "$BINARY" | grep -q .; then
    info "Source files changed since last build — rebuilding…"
    build_needed=true
  fi

  if $build_needed; then
    BUILD_START=$SECONDS
    cd "$NODE_DIR"
    cargo build --release 2>&1 | grep -E "^error|Compiling caspar|Finished" || true
    cd "$REPO_DIR"
    BUILD_ELAPSED=$((SECONDS - BUILD_START))
    ok "Build finished in ${BUILD_ELAPSED}s"
  else
    ok "Binary is up to date"
  fi

  [[ -f "$BINARY" ]] || die "Build failed: binary not found at $BINARY"
fi

BINARY_SIZE=$(ls -lh "$BINARY" | awk '{print $5}')
ok "Binary ready: $BINARY ($BINARY_SIZE)"

# =============================================================================
# STEP 3 — Stop existing processes
# =============================================================================
step "Step 3: Stop any existing processes"

stop_existing() {
  local pids
  pids=$(ps -eo pid,cmd 2>/dev/null | awk '/caspar-node/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$pids" ]]; then
    info "Stopping existing caspar-node processes: $pids"
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    for p in $pids; do kill -9 "$p" 2>/dev/null || true; done
    ok "Caspar nodes stopped"
  else
    ok "No existing caspar-node processes"
  fi

  local jpids
  jpids=$(ps -eo pid,cmd 2>/dev/null | awk '/questdb/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$jpids" ]]; then
    info "Stopping existing QuestDB processes: $jpids"
    for p in $jpids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    ok "QuestDB stopped"
  else
    ok "No existing QuestDB processes"
  fi
}
stop_existing

# =============================================================================
# STEP 4 — Fresh state (optional)
# =============================================================================
if $FRESH; then
  step "Step 4: Wipe state (--fresh)"
  warn "Wiping $DATA_ROOT — all node databases and deployment state cleared"
  warn "You will need to re-run creature deployment after this"
  rm -rf "$DATA_ROOT"
  rm -f "$DEPLOY_REPORT"
  ok "State wiped"
fi

mkdir -p "$DATA_ROOT"

# =============================================================================
# STEP 5 — Start QuestDB
# =============================================================================
step "Step 5: Start QuestDB"

wait_for_port() {
  local host="$1" port="$2" label="$3" timeout="${4:-45}"
  local elapsed=0
  while ! python3 -c "
import socket, sys
s = socket.socket()
s.settimeout(1)
try:
    s.connect(('$host', $port))
    s.close()
    sys.exit(0)
except:
    sys.exit(1)
" 2>/dev/null; do
    sleep 1
    elapsed=$((elapsed + 1))
    [[ $elapsed -ge $timeout ]] && return 1
  done
  return 0
}

QUESTDB_PID=""
if $START_QUESTDB; then
  mkdir -p "$QUESTDB_DATA"
  info "Starting QuestDB (port $QUESTDB_PORT)…"
  java -jar "$QUESTDB_JAR" -m io.questdb/io.questdb.ServerMain \
       -d "$QUESTDB_DATA" >> "$DATA_ROOT/questdb.log" 2>&1 &
  QUESTDB_PID=$!

  if wait_for_port localhost $QUESTDB_PORT QuestDB 60; then
    ok "QuestDB ready (pid $QUESTDB_PID, http: http://localhost:9000)"
  else
    warn "QuestDB did not come up in 60s — nodes will log telemetry errors but will function"
  fi
else
  info "Skipping QuestDB (--no-questdb)"
fi

# =============================================================================
# STEP 6 — Generate node configs (keys) if missing
# =============================================================================
step "Step 6: Configure nodes"

generate_node_env() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"

  mkdir -p "$node_dir"/{storage,db,applet,search,store_logs,telemetry,babble}

  if [[ -f "$env_file" ]]; then
    ok "node$n: config exists ($env_file)"
    return
  fi

  info "node$n: generating RSA keypair and config…"
  local priv_key
  priv_key=$(openssl genrsa 2048 2>/dev/null)

  local is_head="false"
  [[ "$n" -eq 1 ]] && is_head="true"

  cat > "$env_file" <<EOF
OWNER_ID=node${n}
OWNER_PRIVATE_KEY="$priv_key"
STORAGE_ROOT_PATH=$node_dir/storage
BASE_DB_PATH=$node_dir/db
APPLET_DB_PATH=$node_dir/applet
SEARCH_INDEX_PATH=$node_dir/search
STORE_LOGS_DB=$node_dir/store_logs
CLIENT_WS_API_PORT=${NODE_WS[$n]}
CLIENT_TCP_API_PORT=${NODE_TCP[$n]}
FEDERATION_API_PORT=${NODE_FED[$n]}
BLOCKCHAIN_API_PORT=${NODE_CHAIN[$n]}
ENTITY_API_PORT=${NODE_ENTITY[$n]}
VM_API_PORT=${NODE_VM[$n]}
ORIGIN=127.0.0.1
IPADDR=127.0.0.1
ROOT_NODE=127.0.0.1:${NODE_FED[1]}
IS_HEAD=$is_head
AdminPassword=admin123
TELEMETRY_API_PORT=${NODE_TEL[$n]}
TELEMETRY_DB_PATH=$node_dir/telemetry
EOF
  ok "node$n: config written to $env_file"
}

for n in "${NODES[@]}"; do
  generate_node_env "$n"
done

# =============================================================================
# STEP 7 — Start Caspar nodes
# =============================================================================
step "Step 7: Start Caspar nodes"

STARTED_PIDS=()

start_node() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  local log_file="$node_dir/node.log"

  # Rotate log
  [[ -f "$log_file" ]] && mv "$log_file" "${log_file}.prev"

  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a

  info "Starting node$n (TCP=${NODE_TCP[$n]})…"
  BABBLE_DIR="$node_dir/babble" \
    "$BINARY" >> "$log_file" 2>&1 &
  local pid=$!
  echo "$pid" > "$node_dir/caspar.pid"
  echo "$pid"
}

for n in "${NODES[@]}"; do
  pid=$(start_node "$n")
  STARTED_PIDS+=("$pid")
done

# Wait for all nodes to come up
info "Waiting for node(s) to accept TCP connections…"
ALL_UP=true
for n in "${NODES[@]}"; do
  port=${NODE_TCP[$n]}
  if wait_for_port localhost "$port" "node$n" 30; then
    ok "node$n up on port $port"
  else
    warn "node$n did not listen on port $port within 30s"
    warn "  → tail $DATA_ROOT/node${n}/node.log"
    ALL_UP=false
  fi
done

if ! $ALL_UP; then
  die "One or more nodes failed to start. Check logs in $DATA_ROOT/node*/node.log"
fi

# Extra stabilisation pause — let babble form the initial consensus ring
info "Waiting 5s for babble consensus initialisation…"
sleep 5
ok "All ${#NODES[@]} node(s) running"

# =============================================================================
# STEP 8 — Deploy WASM creatures
# =============================================================================
step "Step 8: Deploy WASM creatures"

if $SKIP_DEPLOY; then
  if [[ -f "$DEPLOY_REPORT" ]]; then
    ok "Skipping deployment (--skip-deploy); using existing $DEPLOY_REPORT"
  else
    warn "--skip-deploy given but no deployment report found at $DEPLOY_REPORT"
    warn "Benchmark may fail if creatures are not deployed. Continuing anyway…"
  fi
else
  DEPLOY_START=$SECONDS
  info "Running deploy.py — this deploys ~37 WASM creatures per node"
  info "Expected duration: ~3 min (single) to ~9 min (triple)…"

  python3 "$DEPLOY_SCRIPT" | tee "$DATA_ROOT/deploy.log" || {
    die "deploy.py exited with error. Check $DATA_ROOT/deploy.log"
  }

  DEPLOY_ELAPSED=$((SECONDS - DEPLOY_START))
  ok "Deployment complete in ${DEPLOY_ELAPSED}s"

  if [[ -f "$DEPLOY_REPORT" ]]; then
    MODULE_COUNT=$(python3 -c "
import json
d = json.load(open('$DEPLOY_REPORT'))
total = sum(len(v) for v in d.values() if isinstance(v, dict))
print(total)
" 2>/dev/null || echo "?")
    ok "Deployed $MODULE_COUNT module entries; report: $DEPLOY_REPORT"
  fi

  # Brief pause for nodes to finish processing deployment transactions
  info "Waiting 10s for deployment transactions to settle…"
  sleep 10
fi

# =============================================================================
# STEP 9 — Run benchmark
# =============================================================================
if $SKIP_BENCH; then
  echo ""
  ok "Skipping benchmark (--skip-bench / --deploy-only)."
  echo ""
  echo "  Nodes are running. To run the benchmark manually:"
  echo "    python3 $BENCH_SCRIPT"
  echo ""
  echo "  To stop all nodes:"
  echo "    kill \$(cat $DATA_ROOT/node*/caspar.pid 2>/dev/null)"
  echo ""
  trap 'info "Shutting down nodes…"; for p in "${STARTED_PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; [[ -n "$QUESTDB_PID" ]] && kill "$QUESTDB_PID" 2>/dev/null || true; exit 0' INT TERM
  info "Nodes running. Press Ctrl-C to stop all."
  wait
  exit 0
fi

step "Step 9: Run benchmark suite"

BENCH_START=$SECONDS
info "Running workflow_tests.py…"
echo ""

python3 "$BENCH_SCRIPT" 2>&1 | tee "$DATA_ROOT/bench.log" || {
  warn "workflow_tests.py exited non-zero — check output above"
}

BENCH_ELAPSED=$((SECONDS - BENCH_START))
TOTAL_ELAPSED=$((SECONDS - START_TIME))

# =============================================================================
# Final summary
# =============================================================================
echo ""
echo -e "${BOLD}${GREEN}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}${GREEN}║                  Benchmark Run Complete                     ║${NC}"
echo -e "${BOLD}${GREEN}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${BOLD}Timing${NC}"
echo "    Total elapsed:      ${TOTAL_ELAPSED}s ($(( TOTAL_ELAPSED / 60 ))m $(( TOTAL_ELAPSED % 60 ))s)"
echo "    Benchmark duration: ${BENCH_ELAPSED}s"
echo ""
echo -e "  ${BOLD}Output files${NC}"
[[ -f "$RESULT_JSON" ]] && echo "    Results JSON:  $RESULT_JSON"
[[ -f "$RESULT_MD"   ]] && echo "    Report MD:     $RESULT_MD"
echo "    Node logs:     $DATA_ROOT/node*/node.log"
echo "    Bench log:     $DATA_ROOT/bench.log"
[[ -f "$DEPLOY_REPORT" ]] && echo "    Deploy report: $DEPLOY_REPORT"
echo ""

# Quick pass/fail stats from JSON if available
if [[ -f "$RESULT_JSON" ]]; then
  python3 - "$RESULT_JSON" <<'PYEOF'
import json, sys
data = json.load(open(sys.argv[1]))
steps = data.get("steps", [])
passed = sum(1 for s in steps if s.get("status") == "ok")
failed = sum(1 for s in steps if s.get("status") != "ok")
total  = len(steps)
rate   = (passed / total * 100) if total else 0
print(f"  {'Success rate':20s}  {passed}/{total} ({rate:.1f}%)")
if failed:
    print(f"\n  Failed steps:")
    for s in steps:
        if s.get("status") != "ok":
            print(f"    ✗  [{s.get('node','?')}] {s.get('name','?')}: {s.get('error','')[:80]}")
PYEOF
fi

echo ""

# Graceful shutdown of nodes
trap '' INT TERM
info "Shutting down nodes…"
for p in "${STARTED_PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
[[ -n "${QUESTDB_PID:-}" ]] && kill "$QUESTDB_PID" 2>/dev/null || true
sleep 2

ok "Done. Goodbye."
