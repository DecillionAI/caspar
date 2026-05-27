#!/usr/bin/env bash
# run-nodes.sh — unified Caspar node runner
#
# Usage:
#   ./run-nodes.sh [single|triple] [OPTIONS]
#
# Modes:
#   single  — run only node1
#   triple  — run node1 + node2 + node3 (default)
#
# Options:
#   --no-docker      Run nodes as local binaries instead of docker containers.
#                    (Default: docker. Local mode supports triple-node without
#                    port or data-dir conflicts — each node has its own ports.)
#   --no-questdb     Skip QuestDB startup (manage it separately)
#   --fresh          Wipe /tmp/caspar/* before starting (clean-slate run)
#   --no-gvisor      Skip gVisor (runsc) install / configuration. By default
#                    gVisor is installed and registered with Docker so all
#                    caspar VMs run sandboxed.
#   --rebuild-image  Force rebuild of the caspar-node:latest docker image
#                    (runs build-dist.sh + docker build)
#   --foreground     (docker mode) keep tailing container logs until Ctrl-C
#                    instead of returning immediately
#   --help           show this help
#
# Companion script:
#   ./stop-nodes.sh  — gracefully shuts down everything started by this script

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NODE_DIR="$REPO_DIR/node"
BINARY="$NODE_DIR/target/release/caspar-node"
DATA_ROOT="/tmp/caspar"
QUESTDB_JAR="/opt/questdb/questdb.jar"
QUESTDB_DATA="$DATA_ROOT/questdb"
QUESTDB_PORT=8812
DOCKER_IMAGE="caspar-node:latest"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info()  { echo -e "${CYAN}[caspar]${NC} $*"; }
ok()    { echo -e "${GREEN}[caspar]${NC} $*"; }
warn()  { echo -e "${YELLOW}[caspar]${NC} $*"; }
die()   { echo -e "${RED}[caspar] FATAL:${NC} $*" >&2; exit 1; }

# ─── Arg parsing ─────────────────────────────────────────────────────────────
MODE="triple"
USE_DOCKER=true
START_QUESTDB=true
FRESH=false
SETUP_GVISOR=true
REBUILD_IMAGE=false
FOREGROUND=false

for arg in "$@"; do
  case "$arg" in
    single)          MODE="single" ;;
    triple)          MODE="triple" ;;
    --no-docker)     USE_DOCKER=false ;;
    --no-questdb)    START_QUESTDB=false ;;
    --fresh)         FRESH=true ;;
    --no-gvisor)     SETUP_GVISOR=false ;;
    --rebuild-image) REBUILD_IMAGE=true ;;
    --foreground)    FOREGROUND=true ;;
    --help|-h)
      sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "Unknown argument: $arg" ;;
  esac
done

NODES=(1)
[[ "$MODE" == "triple" ]] && NODES=(1 2 3)

declare -A NODE_TCP=([1]=8074 [2]=8174 [3]=8274)

# ─── Dependency checks ───────────────────────────────────────────────────────
check_dep() {
  local name="$1" cmd="$2" hint="$3"
  if ! command -v "$cmd" &>/dev/null; then
    die "Missing dependency: $name\n  Install: $hint"
  fi
}

info "Checking dependencies…"

if $USE_DOCKER; then
  check_dep "docker" "docker" "https://docs.docker.com/engine/install/"
  docker info >/dev/null 2>&1 \
    || die "Docker daemon is not reachable (try: sudo systemctl start docker)"
else
  check_dep "Rust/cargo" "cargo" "curl https://sh.rustup.rs -sSf | sh"
fi

if $START_QUESTDB; then
  check_dep "java" "java" "apt-get install -y default-jre"
  if [[ ! -f "$QUESTDB_JAR" ]]; then
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
      die "QuestDB jar not found: $QUESTDB_JAR"
    fi
  fi

  jver=$(java -version 2>&1 | grep -oP '(?<=version ")[0-9]+' | head -1)
  [[ -z "$jver" ]] && jver=$(java -version 2>&1 | grep -oP '"[0-9]+\.' | grep -oP '[0-9]+')
  if [[ "${jver:-0}" -lt 11 ]]; then
    warn "Java $jver found but QuestDB needs Java 11+. Skipping QuestDB."
    START_QUESTDB=false
  fi
fi

# ─── gVisor (runsc) check / install (default ON) ─────────────────────────────
if ! $SETUP_GVISOR; then
  info "Skipping gVisor setup (--no-gvisor)"
elif ! command -v docker &>/dev/null; then
  warn "Docker not installed — skipping gVisor setup"
elif command -v runsc &>/dev/null && docker info 2>/dev/null | grep -q runsc; then
  ok "gVisor (runsc) installed and registered with Docker"
else
  info "Installing gVisor (runsc)…"
  if [[ "$EUID" -eq 0 ]]; then
    bash "$REPO_DIR/node/scripts/install-gvisor.sh"
  elif command -v sudo &>/dev/null; then
    sudo bash "$REPO_DIR/node/scripts/install-gvisor.sh"
  else
    warn "Cannot install gVisor: need root. Re-run as root, or pass --no-gvisor to skip."
  fi
fi

ok "All dependency checks passed"

# ─── Fresh start ─────────────────────────────────────────────────────────────
if $FRESH; then
  warn "--fresh: wiping $DATA_ROOT"
  rm -rf "$DATA_ROOT"
fi

mkdir -p "$DATA_ROOT"

# ─── Stop existing processes / containers ───────────────────────────────────
stop_existing() {
  # Local caspar-node processes
  local pids
  pids=$(ps -eo pid,cmd 2>/dev/null | awk '/caspar-node/ && !/awk/ && !/grep/ && !/run-nodes/ {print $1}')
  if [[ -n "$pids" ]]; then
    info "Stopping existing caspar-node processes: $pids"
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    for p in $pids; do kill -9 "$p" 2>/dev/null || true; done
  fi

  # Docker containers
  if command -v docker &>/dev/null; then
    for n in 1 2 3; do
      if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^caspar-node${n}$"; then
        info "Removing existing container: caspar-node${n}"
        docker rm -f "caspar-node${n}" >/dev/null 2>&1 || true
      fi
    done
  fi

  # QuestDB
  local jpids
  jpids=$(ps -eo pid,cmd 2>/dev/null | awk '/questdb/ && !/awk/ && !/grep/ {print $1}')
  if [[ -n "$jpids" ]]; then
    info "Stopping existing QuestDB processes: $jpids"
    for p in $jpids; do kill "$p" 2>/dev/null || true; done
    sleep 1
  fi
}
stop_existing

# ─── Build / fetch the artifact we need ──────────────────────────────────────
if $USE_DOCKER; then
  if $REBUILD_IMAGE || ! docker image inspect "$DOCKER_IMAGE" >/dev/null 2>&1; then
    info "Docker image $DOCKER_IMAGE not present — building (this requires build-dist.sh + docker build)…"
    bash "$REPO_DIR/build-dist.sh"
    docker build -f "$REPO_DIR/node/Dockerfile" -t "$DOCKER_IMAGE" "$REPO_DIR"
  fi
  ok "Docker image ready: $DOCKER_IMAGE"
else
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
fi

# ─── Helper: wait for a TCP port ────────────────────────────────────────────
wait_for_port() {
  local host="$1" port="$2" name="$3" timeout="${4:-30}"
  local elapsed=0
  while ! python3 -c "import socket; s=socket.socket(); s.settimeout(1); s.connect(('$host',$port)); s.close()" 2>/dev/null; do
    sleep 1; elapsed=$((elapsed+1))
    [[ $elapsed -ge $timeout ]] && return 1
  done
  return 0
}

# ─── Start QuestDB ───────────────────────────────────────────────────────────
QUESTDB_PID=""
if $START_QUESTDB; then
  mkdir -p "$QUESTDB_DATA"
  info "Starting QuestDB on port $QUESTDB_PORT…"
  java -jar "$QUESTDB_JAR" -m io.questdb/io.questdb.ServerMain \
       -d "$QUESTDB_DATA" >> "$DATA_ROOT/questdb.log" 2>&1 &
  QUESTDB_PID=$!
  if wait_for_port localhost $QUESTDB_PORT QuestDB 45; then
    ok "QuestDB ready (pid $QUESTDB_PID)"
  else
    warn "QuestDB did not come up within 45s — nodes may log telemetry errors but will function"
  fi
fi

# ─── Per-node config (re-used by both modes) ─────────────────────────────────
# NODE LAYOUT:
#  node1: TCP=8074  WS=8076  FED=8077  CHAIN=8078  ENTITY=8079  VM=8080  TEL=9099
#  node2: TCP=8174  WS=8176  FED=8177  CHAIN=8178  ENTITY=8179  VM=8180  TEL=9199
#  node3: TCP=8274  WS=8276  FED=8277  CHAIN=8278  ENTITY=8279  VM=8280  TEL=9299

ensure_node_config() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  mkdir -p "$node_dir"/{storage,db,applet,search,store_logs,telemetry,babble}

  if [[ ! -f "$env_file" ]]; then
    warn "No $env_file found — node$n may not have keys configured"
  fi
}

# ─── Local-mode launch ───────────────────────────────────────────────────────
local_start_node() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  local log_file="$node_dir/node.log"

  if [[ -f "$env_file" ]]; then
    set -a; source "$env_file"; set +a
  fi

  info "Starting node$n locally (TCP=${NODE_TCP[$n]})…"
  BABBLE_DIR="$node_dir/babble" \
    "$BINARY" >> "$log_file" 2>&1 &
  echo $! > "$node_dir/caspar.pid"
  echo $!
}

# ─── Docker-mode launch ──────────────────────────────────────────────────────
# Generates a container-paths version of .env (host /tmp/caspar/nodeN/ →
# in-container /app/data/), then launches the container with --network host
# so federation, client TCP, telemetry, and inter-node babble all use the
# same ports as local mode without any port-mapping translation.
docker_start_node() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  local docker_env="$node_dir/.env.docker"
  local container="caspar-node${n}"

  if [[ ! -f "$env_file" ]]; then
    warn "No $env_file for node$n — container may fail to start without keys"
    touch "$env_file"
  fi

  # Translate host paths inside .env to /app/data/* paths used in the container
  sed "s|${DATA_ROOT}/node${n}|/app/data|g" "$env_file" > "$docker_env"

  info "Starting node$n in docker (container=$container, TCP=${NODE_TCP[$n]})…"

  local docker_args=(
    --name "$container"
    --network host
    --restart unless-stopped
    -v "$docker_env":/app/.env:ro
    -v "$node_dir":/app/data
  )

  # Mount docker socket so caspar can spawn sibling VM containers via the host
  # daemon — required for the docker-backed VM runtime.
  if [[ -S /var/run/docker.sock ]]; then
    docker_args+=( -v /var/run/docker.sock:/var/run/docker.sock )
  fi

  docker run -d "${docker_args[@]}" "$DOCKER_IMAGE" >/dev/null
  echo "$container"
}

# ─── Launch nodes ────────────────────────────────────────────────────────────
declare -a STARTED_PIDS=()
declare -a STARTED_CONTAINERS=()

for n in "${NODES[@]}"; do
  ensure_node_config "$n"
  if $USE_DOCKER; then
    container=$(docker_start_node "$n")
    STARTED_CONTAINERS+=("$container")
  else
    pid=$(local_start_node "$n")
    STARTED_PIDS+=("$pid")
  fi
done

# ─── Wait for nodes to accept connections ────────────────────────────────────
info "Waiting for node(s) to accept connections…"
all_up=true
for n in "${NODES[@]}"; do
  port=${NODE_TCP[$n]}
  if wait_for_port localhost "$port" "node$n" 30; then
    ok "node$n up on TCP port $port"
  else
    if $USE_DOCKER; then
      warn "node$n did not listen on port $port within 30s"
      warn "  → docker logs caspar-node${n}"
    else
      warn "node$n did not listen on port $port within 30s"
      warn "  → tail $DATA_ROOT/node${n}/node.log"
    fi
    all_up=false
  fi
done

# ─── Summary ─────────────────────────────────────────────────────────────────
echo ""
if $all_up; then
  ok "All ${#NODES[@]} node(s) running."
else
  warn "Some node(s) may not have started correctly."
fi

echo ""
echo "  Mode:    $MODE (${#NODES[@]} node(s)) — $($USE_DOCKER && echo 'docker' || echo 'local')"
[[ $USE_DOCKER == true ]] && echo "  Image:   $DOCKER_IMAGE"
[[ $USE_DOCKER == false ]] && echo "  Binary:  $BINARY"
echo "  Data:    $DATA_ROOT"
[[ $START_QUESTDB == true ]] && echo "  QuestDB: localhost:$QUESTDB_PORT (http: localhost:9000)"

echo ""
if $USE_DOCKER; then
  echo "  Containers:"
  for c in "${STARTED_CONTAINERS[@]}"; do
    echo "    $c → docker logs -f $c"
  done
  echo ""
  echo "  Stop all:  $REPO_DIR/stop-nodes.sh"
else
  echo "  Logs:"
  for n in "${NODES[@]}"; do
    echo "    node$n → $DATA_ROOT/node${n}/node.log"
  done
  echo ""
  echo "  Stop all:  $REPO_DIR/stop-nodes.sh   (or Ctrl-C in this terminal)"
fi
echo ""

# ─── Foreground vs detached behaviour ────────────────────────────────────────
if $USE_DOCKER && ! $FOREGROUND; then
  # Docker containers run detached; safe to return now.
  info "Containers running detached. Run with --foreground to tail logs."
  exit 0
fi

# Otherwise (local mode OR docker --foreground) stay alive so Ctrl-C stops it
cleanup() {
  echo ""
  info "Shutting down…"
  for p in "${STARTED_PIDS[@]:-}"; do
    [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
  done
  for c in "${STARTED_CONTAINERS[@]:-}"; do
    [[ -n "$c" ]] && docker stop --time 10 "$c" >/dev/null 2>&1 || true
  done
  [[ -n "$QUESTDB_PID" ]] && kill "$QUESTDB_PID" 2>/dev/null || true
  exit 0
}
trap cleanup INT TERM

info "Press Ctrl-C to stop everything."
if $USE_DOCKER; then
  # Multiplex docker logs from all containers
  docker logs -f --tail=20 "${STARTED_CONTAINERS[0]}" &
  wait $!
else
  wait
fi
