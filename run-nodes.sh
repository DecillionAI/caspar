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
#   --no-firecracker Skip Firecracker install and network setup. By default
#                    Firecracker is installed and its host bridge is configured
#                    so microVM-backed workloads can run immediately.
#   --rebuild        Force rebuild even if binaries / image already exist.
#                    Docker mode: re-runs build-dist.sh + docker build.
#                    Local mode:  re-runs cargo build --release.
#                    (alias: --rebuild-image)
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
SETUP_FIRECRACKER=true
REBUILD_IMAGE=false
FOREGROUND=false

for arg in "$@"; do
  case "$arg" in
    single)            MODE="single" ;;
    triple)            MODE="triple" ;;
    --no-docker)       USE_DOCKER=false ;;
    --no-questdb)      START_QUESTDB=false ;;
    --fresh)           FRESH=true ;;
    --no-gvisor)       SETUP_GVISOR=false ;;
    --no-firecracker)  SETUP_FIRECRACKER=false ;;
    --rebuild|--rebuild-image) REBUILD_IMAGE=true ;;
    --foreground)      FOREGROUND=true ;;
    --help|-h)
      sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "Unknown argument: $arg" ;;
  esac
done

NODES=(1)
[[ "$MODE" == "triple" ]] && NODES=(1 2 3)

declare -A NODE_TCP=([1]=8074 [2]=8174 [3]=8274)

# ─── Helper: run a function body as root ─────────────────────────────────────
# Usage: _as_root <function_name>
# Calls the named function directly if already root, otherwise exports its
# definition and re-invokes it through sudo bash.  Colour helper functions
# (info/ok/warn) are exported alongside so output remains consistent.
_as_root() {
  local fn="$1"
  local helpers
  helpers="$(declare -f info ok warn die)"
  if [[ "$EUID" -eq 0 ]]; then
    "$fn"
  elif command -v sudo &>/dev/null; then
    sudo bash -c "${helpers}; $(declare -f "$fn"); $fn"
  else
    warn "Cannot run $fn: need root and sudo is not available."
    return 1
  fi
}

# ─── gVisor setup ─────────────────────────────────────────────────────────────
_setup_gvisor() {
  set -e
  info "Installing gVisor (runsc)…"
  apt-get update -qq
  apt-get install -y -qq apt-transport-https ca-certificates curl gnupg

  local keyring="/usr/share/keyrings/gvisor-archive-keyring.gpg"
  [[ -f "$keyring" ]] \
    || curl -fsSL https://gvisor.dev/archive.key | gpg --dearmor --yes -o "$keyring"

  local arch; arch=$(dpkg --print-architecture)
  echo "deb [arch=${arch} signed-by=${keyring}] https://storage.googleapis.com/gvisor/releases release main" \
    > /etc/apt/sources.list.d/gvisor.list

  apt-get update -qq
  apt-get install -y -qq runsc

  # Merge runsc into /etc/docker/daemon.json
  # --network=host: sandbox shares host netstack (no NAT, full reachability)
  # --platform=ptrace: works without /dev/kvm
  python3 -c "
import json, os, tempfile
path = '/etc/docker/daemon.json'
try:
    cfg = json.loads(open(path).read().strip() or '{}')
except FileNotFoundError:
    cfg = {}
cfg.setdefault('runtimes', {})['runsc'] = {
    'path': 'runsc',
    'runtimeArgs': ['--network=host', '--platform=ptrace'],
}
fd, tmp = tempfile.mkstemp(dir='/etc/docker', prefix='.daemon.')
with os.fdopen(fd, 'w') as f:
    json.dump(cfg, f, indent=2); f.write('\n')
os.replace(tmp, path)
print('  wrote /etc/docker/daemon.json')
"

  # Restart Docker and wait for daemon to come back
  if systemctl is-active --quiet docker 2>/dev/null; then
    systemctl restart docker
    local i
    for i in $(seq 1 20); do docker info >/dev/null 2>&1 && break; sleep 0.5; done
  fi
  ok "gVisor (runsc) installed and registered with Docker"
}

# ─── Firecracker setup ────────────────────────────────────────────────────────
FC_VERSION="v1.10.1"
FC_ARCH=$(uname -m)   # x86_64 or aarch64

_install_firecracker() {
  set -e
  local fc_version="${FC_VERSION:-v1.10.1}"
  local fc_arch="${FC_ARCH:-$(uname -m)}"

  info "Installing Firecracker ${fc_version} (${fc_arch})…"
  apt-get update -qq
  apt-get install -y -qq curl libelf-dev e2fsprogs

  mkdir -p /opt/firecracker/{vms,kernel,rootfs,snapshots}

  # Binary
  if ! command -v firecracker &>/dev/null; then
    local tgz="firecracker-${fc_version}-${fc_arch}.tgz"
    curl -fsSL \
      "https://github.com/firecracker-microvm/firecracker/releases/download/${fc_version}/${tgz}" \
      -o "/tmp/${tgz}"
    tar -xzf "/tmp/${tgz}" -C /tmp
    mv "/tmp/release-${fc_version}-${fc_arch}/firecracker-${fc_version}-${fc_arch}" \
       /usr/local/bin/firecracker
    chmod +x /usr/local/bin/firecracker
    rm -rf "/tmp/${tgz}" "/tmp/release-${fc_version}-${fc_arch}"
    ok "Installed: $(firecracker --version 2>&1 | head -1)"
  else
    ok "Firecracker binary already present: $(firecracker --version 2>&1 | head -1)"
  fi

  # Guest kernel
  if [[ ! -f /opt/firecracker/kernel/vmlinux ]]; then
    info "Downloading guest kernel for ${fc_arch}…"
    curl -fsSL \
      "https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/${fc_arch}/kernels/vmlinux.bin" \
      -o /opt/firecracker/kernel/vmlinux
    chmod +x /opt/firecracker/kernel/vmlinux
    ok "Guest kernel ready ($(ls -lh /opt/firecracker/kernel/vmlinux | awk '{print $5}'))"
  else
    ok "Guest kernel already present"
  fi

  # Guest rootfs (Alpine-based ext4 image)
  if [[ ! -f /opt/firecracker/rootfs/rootfs.ext4 ]]; then
    info "Building guest rootfs (Alpine 3.20 / ${fc_arch})…"
    apt-get install -y -qq e2fsprogs >/dev/null
    local alpine_url="https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/${fc_arch}/alpine-minirootfs-3.20.0-${fc_arch}.tar.gz"
    curl -fsSL "$alpine_url" -o /tmp/alpine-minirootfs.tar.gz
    dd if=/dev/zero of=/opt/firecracker/rootfs/rootfs.ext4 bs=1M count=128 status=none
    mkfs.ext4 -q /opt/firecracker/rootfs/rootfs.ext4
    local mnt; mnt=$(mktemp -d)
    mount -o loop /opt/firecracker/rootfs/rootfs.ext4 "$mnt"
    tar -xzf /tmp/alpine-minirootfs.tar.gz -C "$mnt"
    printf '#!/bin/sh\nmount -t proc proc /proc\nmount -t sysfs sysfs /sys\nmt -t devtmpfs devtmpfs /dev 2>/dev/null||true\nexec /bin/sh\n' \
      > "$mnt/sbin/init"
    chmod +x "$mnt/sbin/init"
    umount "$mnt"; rmdir "$mnt"
    rm -f /tmp/alpine-minirootfs.tar.gz
    ok "Guest rootfs ready ($(ls -lh /opt/firecracker/rootfs/rootfs.ext4 | awk '{print $5}'))"
  else
    ok "Guest rootfs already present"
  fi

  [[ -e /dev/kvm ]] \
    && ok "/dev/kvm available — hardware-accelerated microVMs enabled" \
    || warn "/dev/kvm not available — Firecracker will use ptrace platform (slower cold-start)"
}

_setup_firecracker_network() {
  set -e
  local bridge="br0"
  local bridge_cidr="172.16.0.1/24"
  local host_iface
  host_iface=$(ip route show default 2>/dev/null | awk '/default/{print $5; exit}')

  for tool in ip iptables sysctl; do
    command -v "$tool" &>/dev/null || { warn "$tool not found; skipping Firecracker network setup"; return 1; }
  done

  if ! ip link show "$bridge" &>/dev/null; then
    ip link add name "$bridge" type bridge
    ip addr add "$bridge_cidr" dev "$bridge"
    ip link set "$bridge" up
  fi

  if [[ -n "$host_iface" ]]; then
    iptables -t nat -C POSTROUTING -o "$host_iface" -j MASQUERADE 2>/dev/null \
      || iptables -t nat -A POSTROUTING -o "$host_iface" -j MASQUERADE
    iptables -C FORWARD -i "$bridge" -o "$host_iface" -j ACCEPT 2>/dev/null \
      || iptables -A FORWARD -i "$bridge" -o "$host_iface" -j ACCEPT
    iptables -C FORWARD -i "$host_iface" -o "$bridge" \
        -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null \
      || iptables -A FORWARD -i "$host_iface" -o "$bridge" \
           -m state --state RELATED,ESTABLISHED -j ACCEPT
  fi

  [[ "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)" == "1" ]] \
    || sysctl -qw net.ipv4.ip_forward=1
}

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
      curl -fsSL \
        "https://github.com/questdb/questdb/releases/download/$QDB_VER/questdb-$QDB_VER-no-jre-bin.tar.gz" \
        -o /tmp/questdb.tar.gz
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

# ─── gVisor (runsc) — default ON ──────────────────────────────────────────────
if ! $SETUP_GVISOR; then
  info "Skipping gVisor setup (--no-gvisor)"
elif ! command -v docker &>/dev/null; then
  warn "Docker not installed — skipping gVisor setup"
elif command -v runsc &>/dev/null && docker info 2>/dev/null | grep -q runsc; then
  ok "gVisor (runsc) already installed and registered with Docker"
else
  _as_root _setup_gvisor
  docker info 2>/dev/null | grep -q runsc \
    && ok "gVisor registered with Docker" \
    || warn "gVisor setup completed but runsc not visible in docker info"
fi

# ─── Firecracker — default ON ─────────────────────────────────────────────────
if ! $SETUP_FIRECRACKER; then
  info "Skipping Firecracker setup (--no-firecracker)"
else
  if command -v firecracker &>/dev/null && [[ -f /opt/firecracker/kernel/vmlinux ]]; then
    ok "Firecracker already installed: $(firecracker --version 2>&1 | head -1)"
  else
    _as_root _install_firecracker
  fi

  info "Configuring Firecracker host network (bridge + NAT)…"
  _as_root _setup_firecracker_network \
    && ok "Firecracker network ready (br0 172.16.0.1/24)" \
    || warn "Firecracker network setup failed — microVM networking may not work"
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
  local pids
  pids=$(ps -eo pid,cmd 2>/dev/null | awk '/caspar-node/ && !/awk/ && !/grep/ && !/run-nodes/ {print $1}')
  if [[ -n "$pids" ]]; then
    info "Stopping existing caspar-node processes: $pids"
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    sleep 2
    for p in $pids; do kill -9 "$p" 2>/dev/null || true; done
  fi

  if command -v docker &>/dev/null; then
    for n in 1 2 3; do
      if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -q "^caspar-node${n}$"; then
        info "Removing existing container: caspar-node${n}"
        docker rm -f "caspar-node${n}" >/dev/null 2>&1 || true
      fi
    done
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

# ─── Build / fetch the artifact we need ──────────────────────────────────────
if $USE_DOCKER; then
  if $REBUILD_IMAGE || ! docker image inspect "$DOCKER_IMAGE" >/dev/null 2>&1; then
    $REBUILD_IMAGE \
      && info "--rebuild: force-rebuilding $DOCKER_IMAGE (build-dist.sh + docker build)…" \
      || info "Docker image $DOCKER_IMAGE not present — building (runs build-dist.sh + docker build)…"
    bash "$REPO_DIR/build-dist.sh"
    docker build -f "$REPO_DIR/node/Dockerfile" -t "$DOCKER_IMAGE" "$REPO_DIR"
  fi
  ok "Docker image ready: $DOCKER_IMAGE"
else
  build_needed=false
  if $REBUILD_IMAGE; then
    build_needed=true
    info "--rebuild: forcing cargo build --release…"
  elif [[ ! -f "$BINARY" ]]; then
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

# ─── Per-node config ──────────────────────────────────────────────────────────
# NODE LAYOUT:
#  node1: TCP=8074  WS=8076  FED=8077  CHAIN=8078  ENTITY=8079  VM=8080  TEL=9099
#  node2: TCP=8174  WS=8176  FED=8177  CHAIN=8178  ENTITY=8179  VM=8180  TEL=9199
#  node3: TCP=8274  WS=8276  FED=8277  CHAIN=8278  ENTITY=8279  VM=8280  TEL=9299

ensure_node_config() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  mkdir -p "$node_dir"/{storage,db,applet,search,store_logs,telemetry,babble}
  [[ ! -f "$env_file" ]] && warn "No $env_file found — node$n may not have keys configured"
}

# ─── Local-mode launch ───────────────────────────────────────────────────────
local_start_node() {
  local n="$1"
  local node_dir="$DATA_ROOT/node${n}"
  local env_file="$node_dir/.env"
  local log_file="$node_dir/node.log"

  [[ -f "$env_file" ]] && { set -a; source "$env_file"; set +a; }

  info "Starting node$n locally (TCP=${NODE_TCP[$n]})…"
  BABBLE_DIR="$node_dir/babble" "$BINARY" >> "$log_file" 2>&1 &
  echo $! > "$node_dir/caspar.pid"
  echo $!
}

# ─── Docker-mode launch ──────────────────────────────────────────────────────
# Path translation: .env host paths (/tmp/caspar/nodeN/) → container (/app/data/)
# --network host: ports match local mode exactly; no NAT or port-mapping needed.
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

  sed "s|${DATA_ROOT}/node${n}|/app/data|g" "$env_file" > "$docker_env"

  info "Starting node$n in docker (container=$container, TCP=${NODE_TCP[$n]})…"

  local docker_args=(
    --name "$container"
    --network host
    --restart unless-stopped
    -v "$docker_env":/app/.env:ro
    -v "$node_dir":/app/data
  )
  [[ -S /var/run/docker.sock ]] \
    && docker_args+=( -v /var/run/docker.sock:/var/run/docker.sock )

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
    $USE_DOCKER \
      && warn "node$n (port $port): timeout — check: docker logs caspar-node${n}" \
      || warn "node$n (port $port): timeout — check: tail $DATA_ROOT/node${n}/node.log"
    all_up=false
  fi
done

# ─── Summary ─────────────────────────────────────────────────────────────────
echo ""
$all_up && ok "All ${#NODES[@]} node(s) running." || warn "Some node(s) may not have started correctly."

echo ""
echo "  Mode:    $MODE (${#NODES[@]} node(s)) — $($USE_DOCKER && echo 'docker' || echo 'local')"
$USE_DOCKER  && echo "  Image:   $DOCKER_IMAGE"
$USE_DOCKER  || echo "  Binary:  $BINARY"
echo "  Data:    $DATA_ROOT"
$START_QUESTDB && echo "  QuestDB: localhost:$QUESTDB_PORT (http: localhost:9000)"
echo ""
if $USE_DOCKER; then
  echo "  Containers:"
  for c in "${STARTED_CONTAINERS[@]}"; do echo "    $c → docker logs -f $c"; done
  echo ""
  echo "  Stop all:  $REPO_DIR/stop-nodes.sh"
else
  echo "  Logs:"
  for n in "${NODES[@]}"; do echo "    node$n → $DATA_ROOT/node${n}/node.log"; done
  echo ""
  echo "  Stop all:  $REPO_DIR/stop-nodes.sh   (or Ctrl-C in this terminal)"
fi
echo ""

# ─── Foreground vs detached ──────────────────────────────────────────────────
if $USE_DOCKER && ! $FOREGROUND; then
  info "Containers running detached. Run with --foreground to tail logs."
  exit 0
fi

cleanup() {
  echo ""
  info "Shutting down…"
  for p in "${STARTED_PIDS[@]:-}";      do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done
  for c in "${STARTED_CONTAINERS[@]:-}"; do [[ -n "$c" ]] && docker stop --time 10 "$c" >/dev/null 2>&1 || true; done
  [[ -n "$QUESTDB_PID" ]] && kill "$QUESTDB_PID" 2>/dev/null || true
  exit 0
}
trap cleanup INT TERM

info "Press Ctrl-C to stop everything."
if $USE_DOCKER; then
  docker logs -f --tail=20 "${STARTED_CONTAINERS[0]}" &
  wait $!
else
  wait
fi
