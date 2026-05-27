#!/usr/bin/env bash
# =============================================================================
# setup-fcvmm-network.sh — Configure host networking for Firecracker microVMs
#
# Creates a persistent bridge (br0) and enables NAT + IP forwarding so that
# each microVM's TAP interface can reach the outside world through the host.
# Called automatically by run-nodes.sh / bench-all.sh on every node start.
#
# Idempotent: safe to run repeatedly. Skips any step already in place.
# Requires root.
#
# Network layout:
#   host bridge: br0   172.16.0.1/24
#   microVM TAPs:       172.16.0.2 … 172.16.0.254  (one per running VM)
#   NAT via host's default-route interface → full outbound internet access
#
# Usage:
#   sudo bash setup-fcvmm-network.sh
#   sudo bash setup-fcvmm-network.sh --teardown   # undo everything
# =============================================================================

set -euo pipefail

BRIDGE="br0"
BRIDGE_CIDR="172.16.0.1/24"
TEARDOWN=false

for arg in "$@"; do
  case "$arg" in
    --teardown) TEARDOWN=true ;;
    --help|-h)
      sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info() { echo -e "${CYAN}[fcvmm-net]${NC} $*"; }
ok()   { echo -e "${GREEN}[fcvmm-net]${NC} $*"; }
warn() { echo -e "${YELLOW}[fcvmm-net]${NC} $*"; }
die()  { echo -e "${RED}[fcvmm-net] FATAL:${NC} $*" >&2; exit 1; }

[[ "$EUID" -eq 0 ]] || die "Must be run as root (or via sudo)"

# Detect the host's outbound interface (the one used for default route)
HOST_IFACE=$(ip route show default 2>/dev/null | awk '/default/ {print $5; exit}')
if [[ -z "$HOST_IFACE" ]]; then
  warn "Cannot detect default network interface — NAT rule will not be configured"
  warn "  Set HOST_IFACE manually and re-run, e.g.: HOST_IFACE=eth0 sudo $0"
fi

# ─── Teardown path ────────────────────────────────────────────────────────────
if $TEARDOWN; then
  info "Tearing down Firecracker network…"
  if ip link show "$BRIDGE" &>/dev/null; then
    ip link set "$BRIDGE" down 2>/dev/null || true
    ip link delete "$BRIDGE" type bridge 2>/dev/null || true
    ok "Bridge $BRIDGE removed"
  fi
  if [[ -n "$HOST_IFACE" ]]; then
    iptables -t nat -D POSTROUTING -o "$HOST_IFACE" -j MASQUERADE 2>/dev/null || true
    iptables -D FORWARD -i "$BRIDGE" -o "$HOST_IFACE" -j ACCEPT 2>/dev/null || true
    iptables -D FORWARD -i "$HOST_IFACE" -o "$BRIDGE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || true
  fi
  sysctl -qw net.ipv4.ip_forward=0 2>/dev/null || true
  ok "Teardown complete"
  exit 0
fi

# ─── Setup path ───────────────────────────────────────────────────────────────

# Ensure needed tools
for tool in ip iptables sysctl; do
  command -v "$tool" >/dev/null 2>&1 \
    || die "$tool not found (install iproute2 / iptables)"
done

# 1. Bridge
if ip link show "$BRIDGE" &>/dev/null; then
  ok "Bridge $BRIDGE already exists"
else
  info "Creating bridge $BRIDGE ($BRIDGE_CIDR)…"
  ip link add name "$BRIDGE" type bridge
  ip addr add "$BRIDGE_CIDR" dev "$BRIDGE"
  ip link set "$BRIDGE" up
  ok "Bridge $BRIDGE up ($BRIDGE_CIDR)"
fi

# 2. NAT + forwarding
if [[ -n "$HOST_IFACE" ]]; then
  # Only add rules if not already present
  if ! iptables -t nat -C POSTROUTING -o "$HOST_IFACE" -j MASQUERADE 2>/dev/null; then
    info "Adding NAT MASQUERADE rule (out: $HOST_IFACE)…"
    iptables -t nat -A POSTROUTING -o "$HOST_IFACE" -j MASQUERADE
  fi
  if ! iptables -C FORWARD -i "$BRIDGE" -o "$HOST_IFACE" -j ACCEPT 2>/dev/null; then
    iptables -A FORWARD -i "$BRIDGE" -o "$HOST_IFACE" -j ACCEPT
  fi
  if ! iptables -C FORWARD -i "$HOST_IFACE" -o "$BRIDGE" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null; then
    iptables -A FORWARD -i "$HOST_IFACE" -o "$BRIDGE" -m state --state RELATED,ESTABLISHED -j ACCEPT
  fi
  ok "NAT rules in place (bridge → $HOST_IFACE)"
else
  warn "Skipping NAT rules — no default interface detected"
fi

# 3. IP forwarding
if [[ "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)" != "1" ]]; then
  info "Enabling IPv4 forwarding…"
  sysctl -qw net.ipv4.ip_forward=1
fi
ok "IPv4 forwarding enabled"

echo ""
ok "Firecracker network ready."
echo ""
echo "  Bridge:   $BRIDGE  ($BRIDGE_CIDR)"
[[ -n "$HOST_IFACE" ]] && echo "  NAT via:  $HOST_IFACE"
echo "  microVMs: assign TAP devices in 172.16.0.0/24, gateway 172.16.0.1"
echo ""
