#!/usr/bin/env bash
# =============================================================================
# bench-all.sh — Caspar creature deployment + benchmark runner
#
# Assumes nodes are ALREADY running (started via run-nodes.sh).
# This script owns only the benchmarking phase:
#
#   1. Verify node connectivity
#   2. Deploy WASM creatures to each node  (deploy.py)
#   3. Run the full benchmark suite        (workflow_tests.py)
#   4. Print a KPI summary and paths to the generated report
#
# Usage:
#   ./bench-all.sh [single|triple] [OPTIONS]
#
# Modes:
#   single   — target node1 only  (ports 8074/8076/8077)
#   triple   — target all 3 nodes (default)
#
# Options:
#   --skip-deploy    Skip creature deployment; use the last deployment_report.json
#   --help           Show this help
#
# Outputs (written to $HOME):
#   ~/workflow_results.json   — machine-readable benchmark results
#   ~/workflow_report.md      — professional KPI report
#   ~/deployment_report.json  — deployment manifest (by deploy.py)
#
# Scripts location (auto-detected, override with DECILLIONAI_SERVER env var):
#   <caspar-repo>/../decillionai-server/bench/deploy.py
#   <caspar-repo>/../decillionai-server/bench/workflow_tests.py
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ─── Locate decillionai-server/bench ─────────────────────────────────────────
# Resolution order (first match wins):
#   1. DECILLIONAI_SERVER env var  (explicit override)
#   2. Sibling directory next to the caspar repo
#   3. Legacy /home/user/decillionai-server path
_find_bench_dir() {
  if [[ -n "${DECILLIONAI_SERVER:-}" ]]; then
    echo "$DECILLIONAI_SERVER/bench"
    return
  fi
  # Walk up from SCRIPT_DIR to find a sibling decillionai-server repo
  local parent; parent="$(dirname "$SCRIPT_DIR")"
  if [[ -d "$parent/decillionai-server/bench" ]]; then
    echo "$parent/decillionai-server/bench"
    return
  fi
  # Legacy hard-coded path
  if [[ -d "/home/user/decillionai-server/bench" ]]; then
    echo "/home/user/decillionai-server/bench"
    return
  fi
  echo ""
}

BENCH_DIR="$(_find_bench_dir)"
if [[ -z "$BENCH_DIR" ]]; then
  echo -e "\033[0;31m[bench] FATAL:\033[0m Cannot find decillionai-server/bench directory." >&2
  echo "  Clone it next to the caspar repo:  git clone https://github.com/DecillionAI/decillionai-server.git" >&2
  echo "  Or set: export DECILLIONAI_SERVER=/path/to/decillionai-server" >&2
  exit 1
fi

HOME_DIR="${HOME:-/home/user}"
DEPLOY_SCRIPT="$BENCH_DIR/deploy.py"
BENCH_SCRIPT="$BENCH_DIR/workflow_tests.py"
DEPLOY_REPORT="$HOME_DIR/deployment_report.json"
RESULT_JSON="$HOME_DIR/workflow_results.json"
RESULT_MD="$HOME_DIR/workflow_report.md"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
info()  { echo -e "${CYAN}[bench]${NC} $*"; }
ok()    { echo -e "${GREEN}[bench]${NC} $*"; }
warn()  { echo -e "${YELLOW}[bench]${NC} $*"; }
die()   { echo -e "${RED}[bench] FATAL:${NC} $*" >&2; exit 1; }
step()  { echo -e "\n${BOLD}${CYAN}━━━ $* ━━━${NC}"; }

# ─── Arg parsing ─────────────────────────────────────────────────────────────
MODE="triple"
SKIP_DEPLOY=false

for arg in "$@"; do
  case "$arg" in
    single)           MODE="single" ;;
    triple)           MODE="triple" ;;
    --skip-deploy)    SKIP_DEPLOY=true ;;
    --help|-h)
      sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "Unknown argument: $arg" ;;
  esac
done

[[ "$MODE" == "single" ]] && NODES=(1) || NODES=(1 2 3)

declare -A NODE_TCP=([1]=8074 [2]=8174 [3]=8274)

START_TIME=$SECONDS

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║           Caspar — Creature Deployment & Benchmark          ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
info "Mode: $MODE (${#NODES[@]} node(s))"
echo ""

# =============================================================================
# STEP 1 — Prerequisites
# =============================================================================
step "Step 1: Prerequisites"

command -v python3 &>/dev/null || die "python3 not found (apt-get install -y python3)"
ok "python3 → $(command -v python3)"

if ! python3 -c "from Crypto.PublicKey import RSA" 2>/dev/null; then
  info "pycryptodome not found — installing…"
  pip3 install pycryptodome --quiet || die "pip3 install pycryptodome failed"
fi
ok "pycryptodome available"

[[ -f "$DEPLOY_SCRIPT" ]] || die "deploy.py not found at $DEPLOY_SCRIPT"
ok "deploy.py    → $DEPLOY_SCRIPT"

[[ -f "$BENCH_SCRIPT"  ]] || die "workflow_tests.py not found at $BENCH_SCRIPT"
ok "workflow_tests.py → $BENCH_SCRIPT"

# =============================================================================
# STEP 2 — Verify nodes are reachable
# =============================================================================
step "Step 2: Verify node connectivity"

check_port() {
  local host="$1" port="$2"
  python3 -c "
import socket, sys
s = socket.socket(); s.settimeout(2)
try: s.connect(('$host', $port)); s.close(); sys.exit(0)
except: sys.exit(1)
" 2>/dev/null
}

all_up=true
for n in "${NODES[@]}"; do
  port=${NODE_TCP[$n]}
  if check_port 127.0.0.1 "$port"; then
    ok "node$n reachable on TCP port $port"
  else
    warn "node$n NOT reachable on port $port"
    all_up=false
  fi
done

if ! $all_up; then
  echo ""
  die "One or more nodes not reachable. Start them first with:
  $SCRIPT_DIR/run-nodes.sh $MODE"
fi

# Ensure /tmp/caspar exists for log files (may be absent if bench is run standalone)
mkdir -p /tmp/caspar

# =============================================================================
# STEP 3 — Deploy WASM creatures
# =============================================================================
step "Step 3: Deploy WASM creatures"

if $SKIP_DEPLOY; then
  if [[ -f "$DEPLOY_REPORT" ]]; then
    ok "Skipping deployment (--skip-deploy) — using $DEPLOY_REPORT"
  else
    warn "--skip-deploy set but $DEPLOY_REPORT not found; workflow_tests may fail"
  fi
else
  DEPLOY_START=$SECONDS
  info "Running deploy.py — creates machine creatures, programs, and deploys WASM modules"
  info "Expected duration: ~3 min (single node) to ~9 min (triple)…"
  echo ""

  python3 "$DEPLOY_SCRIPT" 2>&1 | tee /tmp/caspar/deploy.log \
    || die "deploy.py exited with error — check /tmp/caspar/deploy.log"

  DEPLOY_ELAPSED=$((SECONDS - DEPLOY_START))

  if [[ -f "$DEPLOY_REPORT" ]]; then
    MODULE_COUNT=$(python3 -c "
import json
reps = json.load(open('$DEPLOY_REPORT'))
total = sum(r.get('ok_count', 0) for r in reps if isinstance(r, dict))
print(total)
" 2>/dev/null || echo "?")
    ok "Deployment complete in ${DEPLOY_ELAPSED}s — $MODULE_COUNT modules deployed"
  else
    warn "deploy.py finished but $DEPLOY_REPORT not found"
  fi

  info "Waiting 10s for deployment transactions to settle…"
  sleep 10
fi

# =============================================================================
# STEP 4 — Run benchmark suite
# =============================================================================
step "Step 4: Run benchmark suite (workflow_tests.py)"

BENCH_START=$SECONDS
info "Running workflow_tests.py — 9 creature workflows + TPS burst…"
echo ""

python3 "$BENCH_SCRIPT" 2>&1 | tee /tmp/caspar/bench.log \
  || warn "workflow_tests.py exited non-zero — check output above"

BENCH_ELAPSED=$((SECONDS - BENCH_START))
TOTAL_ELAPSED=$((SECONDS - START_TIME))

# =============================================================================
# Summary
# =============================================================================
echo ""
echo -e "${BOLD}${GREEN}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}${GREEN}║                   Benchmark Complete                        ║${NC}"
echo -e "${BOLD}${GREEN}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "  ${BOLD}Timing${NC}"
echo "    Total elapsed:   ${TOTAL_ELAPSED}s  ($(( TOTAL_ELAPSED / 60 ))m $(( TOTAL_ELAPSED % 60 ))s)"
echo "    Deploy:          ${DEPLOY_ELAPSED:-0}s"
echo "    Benchmark:       ${BENCH_ELAPSED}s"
echo ""
echo -e "  ${BOLD}Output files${NC}"
[[ -f "$RESULT_JSON" ]] && echo "    Results JSON :  $RESULT_JSON"
[[ -f "$RESULT_MD"   ]] && echo "    KPI Report   :  $RESULT_MD"
[[ -f "$DEPLOY_REPORT" ]] && echo "    Deploy report:  $DEPLOY_REPORT"
echo "    Node logs    :  /tmp/caspar/node*/node.log"
echo "    Bench log    :  /tmp/caspar/bench.log"
echo ""

# Quick KPI summary from JSON
if [[ -f "$RESULT_JSON" ]]; then
  python3 - "$RESULT_JSON" <<'PYEOF'
import json, sys, statistics

data   = json.load(open(sys.argv[1]))
total  = data["summary"]["total"]
passed = data["summary"]["passed"]
failed = data["summary"]["failed"]
rate   = passed / total * 100 if total else 0

print(f"  {'Workflow steps':22s}  {passed}/{total}  ({rate:.1f}% pass rate)")

# latency across all results
lats = [r["elapsed_ms"] for r in data.get("results", []) if r.get("elapsed_ms", 0) > 0]
if lats:
    lats.sort()
    n = len(lats)
    p = lambda q: lats[min(int(n * q / 100), n-1)]
    print(f"  {'Latency p50/p95/p99':22s}  {p(50):.1f} / {p(95):.1f} / {p(99):.1f} ms")
    print(f"  {'Latency mean/max':22s}  {statistics.mean(lats):.1f} / {max(lats):.1f} ms")

# TPS
tps_rows = data.get("tps_benchmarks", [])
if tps_rows:
    best  = max(tps_rows, key=lambda x: x["tps"])
    worst = min(tps_rows, key=lambda x: x["tps"])
    print(f"  {'Best TPS':22s}  {best['tps']:.3f} ops/s  ({best['label']})")
    print(f"  {'Worst TPS':22s}  {worst['tps']:.3f} ops/s  ({worst['label']})")

# Failures
failures = [r for r in data.get("results", []) if not r.get("ok")]
if failures:
    print(f"\n  {'Failed steps':22s}  {len(failures)}")
    for r in failures[:5]:
        err = (r.get("error") or "")[:70]
        print(f"    ✗ [{r.get('node','?')}] {r.get('step','?')}: {err}")
    if len(failures) > 5:
        print(f"    … and {len(failures)-5} more — see {sys.argv[1]}")
PYEOF
fi

echo ""
ok "Report: $RESULT_MD"
echo ""
