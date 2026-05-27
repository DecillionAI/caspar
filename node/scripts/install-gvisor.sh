#!/usr/bin/env bash
# =============================================================================
# install-gvisor.sh — Install and configure gVisor (runsc) for Docker
#
# Every docker-backed VM that caspar-node spawns runs under the runsc runtime
# instead of runc, providing kernel-level syscall isolation for untrusted
# guest code. This script:
#
#   1. Installs runsc from the official gVisor apt repository (idempotent)
#   2. Merges runsc into /etc/docker/daemon.json with --network=host so
#      sandboxed containers share the host netstack (no NAT, no port mapping
#      penalty, full host network reachability)
#   3. Restarts the Docker daemon to pick up the new runtime
#   4. Verifies runsc works by running `docker info | grep runsc`
#
# Idempotent: safe to run repeatedly. Skips already-installed steps.
#
# Usage:
#   sudo bash install-gvisor.sh           # auto-install if missing
#   sudo bash install-gvisor.sh --force   # reinstall even if present
# =============================================================================

set -euo pipefail

FORCE=false
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=true ;;
    --help|-h)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info() { echo -e "${CYAN}[gvisor]${NC} $*"; }
ok()   { echo -e "${GREEN}[gvisor]${NC} $*"; }
warn() { echo -e "${YELLOW}[gvisor]${NC} $*"; }
die()  { echo -e "${RED}[gvisor] FATAL:${NC} $*" >&2; exit 1; }

# Require root (apt + writing /etc/docker + systemctl restart)
if [[ "$EUID" -ne 0 ]]; then
  die "Must be run as root (or via sudo) — apt + /etc/docker + systemctl restart"
fi

# Docker is a hard prerequisite
command -v docker >/dev/null 2>&1 \
  || die "Docker not installed. Install Docker first: https://docs.docker.com/engine/install/"

# ─── Step 1: install runsc ────────────────────────────────────────────────────
if command -v runsc >/dev/null 2>&1 && ! $FORCE; then
  ok "runsc already installed: $(runsc --version 2>&1 | head -1)"
else
  info "Installing gVisor runtime (runsc)…"
  apt-get update -qq
  apt-get install -y -qq apt-transport-https ca-certificates curl gnupg

  KEYRING="/usr/share/keyrings/gvisor-archive-keyring.gpg"
  if [[ ! -f "$KEYRING" ]] || $FORCE; then
    curl -fsSL https://gvisor.dev/archive.key | gpg --dearmor --yes -o "$KEYRING"
  fi

  ARCH=$(dpkg --print-architecture)
  echo "deb [arch=$ARCH signed-by=$KEYRING] https://storage.googleapis.com/gvisor/releases release main" \
    > /etc/apt/sources.list.d/gvisor.list

  apt-get update -qq
  apt-get install -y -qq runsc

  ok "Installed: $(runsc --version 2>&1 | head -1)"
fi

# ─── Step 2: configure /etc/docker/daemon.json ───────────────────────────────
DAEMON_JSON="/etc/docker/daemon.json"
mkdir -p /etc/docker

# Use Python rather than sed/jq for a robust idempotent JSON merge.
# gVisor flags:
#   --network=host         : skip the gVisor netstack entirely and use the host's
#                            (unrestricted; required for caspar VMs to reach the
#                             same host networks the daemon sees)
#   --platform=ptrace      : use ptrace platform (works without /dev/kvm; KVM is
#                            available but slower-to-cold-start in many CI hosts)
info "Merging runsc runtime into $DAEMON_JSON …"
python3 - "$DAEMON_JSON" <<'PYEOF'
import json, os, sys, tempfile

path = sys.argv[1]
try:
    with open(path) as f:
        raw = f.read().strip()
    cfg = json.loads(raw) if raw else {}
except FileNotFoundError:
    cfg = {}

if not isinstance(cfg, dict):
    sys.exit(f"{path} is not a JSON object")

runtimes = cfg.setdefault("runtimes", {})
runtimes["runsc"] = {
    "path": "runsc",
    "runtimeArgs": [
        "--network=host",
        "--platform=ptrace",
    ],
}

# Pretty-write atomically
fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or "/tmp",
                            prefix=".daemon.json.")
with os.fdopen(fd, "w") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
os.replace(tmp, path)
print(f"  wrote {path}")
PYEOF

# ─── Step 3: restart Docker ─────────────────────────────────────────────────
info "Restarting Docker to load runsc runtime…"
if systemctl is-active --quiet docker 2>/dev/null; then
  systemctl restart docker
  # Wait for daemon to come back up
  for _ in $(seq 1 20); do
    docker info >/dev/null 2>&1 && break
    sleep 0.5
  done
else
  warn "Docker is not running under systemd; please restart it manually"
fi

# ─── Step 4: verify ──────────────────────────────────────────────────────────
if docker info 2>/dev/null | grep -q "runsc"; then
  ok "Docker now exposes the runsc runtime"
else
  die "Docker restarted but runsc runtime is not visible. Check 'docker info' and $DAEMON_JSON"
fi

# Optional: actually launch a container with --runtime=runsc to be sure
if $FORCE || [[ "${GVISOR_SMOKE_TEST:-0}" == "1" ]]; then
  info "Running smoke test: docker run --runtime=runsc hello-world …"
  if docker run --rm --runtime=runsc hello-world >/dev/null 2>&1; then
    ok "Smoke test passed"
  else
    warn "Smoke test failed — runsc may not be fully operational"
  fi
fi

echo ""
ok "gVisor setup complete."
echo ""
echo "  runtime: runsc"
echo "  config:  $DAEMON_JSON"
echo "  network: --network=host (unrestricted, shares host netstack)"
echo "  platform: ptrace"
echo ""
echo "  Usage from Docker:"
echo "    docker run --runtime=runsc <image>"
echo ""
echo "  caspar-node's docker_vm_controller already uses runtime=runsc by default."
echo ""
