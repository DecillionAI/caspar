#!/usr/bin/env bash
# =============================================================================
# install-fcvmm.sh — Install Firecracker microVM manager
#
# Installs:
#   /usr/local/bin/firecracker          — microVM hypervisor binary
#   /opt/firecracker/kernel/vmlinux     — guest kernel image
#   /opt/firecracker/rootfs/rootfs.ext4 — base guest root filesystem (Alpine)
#   /opt/firecracker/{vms,snapshots}    — runtime working directories
#
# Idempotent: skips components that are already present unless --force is given.
# Requires root.
#
# Usage:
#   sudo bash install-fcvmm.sh                    # install / skip existing
#   sudo bash install-fcvmm.sh --force            # reinstall everything
#   sudo bash install-fcvmm.sh --version v1.10.1  # pin a specific release
# =============================================================================

set -euo pipefail

FORCE=false
FC_VERSION="v1.10.1"

for arg in "$@"; do
  case "$arg" in
    --force)       FORCE=true ;;
    --version)     shift; FC_VERSION="$1" ;;
    --help|-h)
      sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info() { echo -e "${CYAN}[fcvmm]${NC} $*"; }
ok()   { echo -e "${GREEN}[fcvmm]${NC} $*"; }
warn() { echo -e "${YELLOW}[fcvmm]${NC} $*"; }
die()  { echo -e "${RED}[fcvmm] FATAL:${NC} $*" >&2; exit 1; }

[[ "$EUID" -eq 0 ]] || die "Must be run as root (or via sudo)"

ARCH=$(uname -m)
case "$ARCH" in
  x86_64)         FC_ARCH="x86_64" ;;
  aarch64|arm64)  FC_ARCH="aarch64" ;;
  *)              die "Unsupported architecture: $ARCH (Firecracker supports x86_64 and aarch64)" ;;
esac

# ─── Step 1: runtime directories ─────────────────────────────────────────────
mkdir -p /opt/firecracker/{vms,kernel,rootfs,snapshots}
ok "Runtime directories ready under /opt/firecracker/"

# ─── Step 2: Firecracker binary ──────────────────────────────────────────────
FC_BIN="/usr/local/bin/firecracker"
if command -v firecracker &>/dev/null && ! $FORCE; then
  ok "firecracker already installed: $(firecracker --version 2>&1 | head -1)"
else
  info "Downloading Firecracker $FC_VERSION ($FC_ARCH)…"
  apt-get update -qq
  apt-get install -y -qq curl make git gcc bridge-utils net-tools \
                          libelf-dev pkg-config lsof

  FC_RELEASE_URL="https://github.com/firecracker-microvm/firecracker/releases/download"
  FC_TGZ="firecracker-${FC_VERSION}-${FC_ARCH}.tgz"
  curl -fsSL "${FC_RELEASE_URL}/${FC_VERSION}/${FC_TGZ}" -o "/tmp/${FC_TGZ}"
  tar -xzf "/tmp/${FC_TGZ}" -C /tmp
  mv "/tmp/release-${FC_VERSION}-${FC_ARCH}/firecracker-${FC_VERSION}-${FC_ARCH}" "$FC_BIN"
  chmod +x "$FC_BIN"
  rm -rf "/tmp/${FC_TGZ}" "/tmp/release-${FC_VERSION}-${FC_ARCH}"
  ok "Installed: $(firecracker --version 2>&1 | head -1) → $FC_BIN"
fi

# ─── Step 3: guest kernel ─────────────────────────────────────────────────────
KERNEL="/opt/firecracker/kernel/vmlinux"
if [[ -f "$KERNEL" ]] && ! $FORCE; then
  ok "Guest kernel already present: $KERNEL ($(ls -lh "$KERNEL" | awk '{print $5}'))"
else
  info "Downloading guest kernel for $FC_ARCH…"
  KERNEL_URL="https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/${FC_ARCH}/kernels/vmlinux.bin"
  curl -fsSL "$KERNEL_URL" -o "$KERNEL"
  chmod +x "$KERNEL"
  ok "Guest kernel: $KERNEL ($(ls -lh "$KERNEL" | awk '{print $5}'))"
fi

# ─── Step 4: guest root filesystem (Alpine-based ext4 image) ─────────────────
ROOTFS="/opt/firecracker/rootfs/rootfs.ext4"
if [[ -f "$ROOTFS" ]] && ! $FORCE; then
  ok "Guest rootfs already present: $ROOTFS ($(ls -lh "$ROOTFS" | awk '{print $5}'))"
else
  info "Building guest rootfs (Alpine Linux, $FC_ARCH)…"

  # Resolve Alpine arch naming
  case "$FC_ARCH" in
    x86_64)  ALPINE_ARCH="x86_64" ;;
    aarch64) ALPINE_ARCH="aarch64" ;;
  esac

  ALPINE_VERSION="3.20"
  ALPINE_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/${ALPINE_ARCH}/alpine-minirootfs-${ALPINE_VERSION}.0-${ALPINE_ARCH}.tar.gz"
  ALPINE_TGZ="/tmp/alpine-minirootfs.tar.gz"

  apt-get install -y -qq e2fsprogs debootstrap >/dev/null
  curl -fsSL "$ALPINE_URL" -o "$ALPINE_TGZ"

  # Create a 128 MB ext4 image
  dd if=/dev/zero of="$ROOTFS" bs=1M count=128 status=none
  mkfs.ext4 -q "$ROOTFS"

  MNTDIR=$(mktemp -d)
  mount -o loop "$ROOTFS" "$MNTDIR"
  tar -xzf "$ALPINE_TGZ" -C "$MNTDIR"

  # Minimal init that drops to a shell — guests can exec their own agent
  cat > "$MNTDIR/sbin/init" <<'INIT'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
exec /bin/sh
INIT
  chmod +x "$MNTDIR/sbin/init"

  umount "$MNTDIR"
  rmdir "$MNTDIR"
  rm -f "$ALPINE_TGZ"

  ok "Guest rootfs: $ROOTFS ($(ls -lh "$ROOTFS" | awk '{print $5}'))"
fi

# ─── Step 5: KVM availability check (advisory) ───────────────────────────────
if [[ -e /dev/kvm ]]; then
  ok "/dev/kvm available — hardware-accelerated microVMs enabled"
else
  warn "/dev/kvm not available — Firecracker will use the ptrace platform (slower cold-start)"
  warn "  To enable KVM: ensure your host supports virtualisation and load the kvm module"
fi

echo ""
ok "Firecracker setup complete."
echo ""
echo "  Binary:  $FC_BIN ($(firecracker --version 2>&1 | head -1))"
echo "  Kernel:  $KERNEL"
echo "  Rootfs:  $ROOTFS"
echo "  Sockets: /opt/firecracker/vms/fc.<machineId>.<vmId>.sock"
echo ""
echo "  Run network setup (bridge + NAT) before starting nodes:"
echo "    sudo bash $(dirname "$0")/setup-fcvmm-network.sh"
echo ""
