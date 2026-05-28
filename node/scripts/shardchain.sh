#!/bin/bash
# shardchain.sh — Bootstrap a new babble shard directory.
#
# Usage: shardchain.sh <storageRoot> <workchainId> <shardchainId> <peersMode>
#   peersMode 1 = new shard: copy local genesis only
#   peersMode 2 = IS_HEAD node: copy genesis as both genesis + current peers
#   peersMode 3 = follower node: copy local genesis, OR (if missing) fetch from
#                 the root node's chain HTTP API
#
# Environment variables:
#   BABBLE_DATA_DIR  — directory holding this node's priv_key, key.pub, and
#                      peers.genesis.json (default: /root/.babble).
#   ROOT_NODE        — "<host>:<tcp_api_port>" of the cluster's head node. The
#                      babble chain HTTP API runs on tcp_api_port + 4 (matches
#                      run-nodes.sh's layout: TCP=8074 → BLOCKCHAIN=8078).
#   ROOT_NODE_ENTITY — explicit override for the chain HTTP API URL (only
#                      consulted in mode 3 and only if BABBLE_DATA_DIR has no
#                      local peers.genesis.json). If unset, derived from
#                      ROOT_NODE.
#
# Exit codes:
#   0 — bootstrap succeeded; babble can start.
#   1 — key material missing, copy failed, or follower could not obtain peer
#       set. Caller (Rust blockchain driver) must surface this; silent success
#       on a broken shard leaves babble in a permanent JOINING loop and the
#       node's API listeners stay closed, which is what produces the opaque
#       "connection refused" cascade in the benchmark.

set -u

storageRoot=${1:?missing storageRoot arg}
workchainId=${2:-"main"}
shardchainId=${3:-"shard-main"}
peersMode=${4:-"3"}

babble_src="${BABBLE_DATA_DIR:-/root/.babble}"
dest="${storageRoot}/chains/${workchainId}/${shardchainId}"

log() { echo "[shardchain] $*" >&2; }
die() { log "FATAL: $*"; exit 1; }

mkdir -p "$dest" || die "mkdir $dest"

# Validator key material — required for every mode. Without these babble
# cannot derive its peer id and the node never enters BABBLING state.
[[ -f "$babble_src/key.pub"  ]] || die "missing $babble_src/key.pub"
[[ -f "$babble_src/priv_key" ]] || die "missing $babble_src/priv_key"
cp "$babble_src/key.pub"  "$dest/key.pub"  || die "cp key.pub to $dest"
cp "$babble_src/priv_key" "$dest/priv_key" || die "cp priv_key to $dest"

case "$peersMode" in
  1)
    # New shard, single-node startup. The local genesis file is authoritative.
    [[ -f "$babble_src/peers.genesis.json" ]] \
      || die "mode 1: missing $babble_src/peers.genesis.json"
    cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json" \
      || die "cp peers.genesis.json to $dest"
    ;;

  2)
    # IS_HEAD node. The local genesis IS the current peer set — followers
    # will fetch it from us. We never need to consult any remote endpoint.
    [[ -f "$babble_src/peers.genesis.json" ]] \
      || die "mode 2 (head): missing $babble_src/peers.genesis.json"
    cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json" \
      || die "cp peers.genesis.json to $dest"
    cp "$babble_src/peers.genesis.json" "$dest/peers.json" \
      || die "cp peers.json to $dest"
    ;;

  3)
    # Follower. Prefer the locally pre-bootstrapped peer set written by
    # run-nodes.sh; only fall back to fetching when the local file is
    # genuinely absent (e.g. a node joining an already-running cluster).
    if [[ -f "$babble_src/peers.genesis.json" ]]; then
      log "using local peers.genesis.json from $babble_src"
      cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json" \
        || die "cp peers.genesis.json to $dest"
      cp "$babble_src/peers.genesis.json" "$dest/peers.json" \
        || die "cp peers.json to $dest"
    else
      # Derive the chain HTTP URL from ROOT_NODE (host:tcp_port) since the
      # chain API listens on tcp_port + 4. The previous default of
      # "localhost:8079" pointed at the entity API — the wrong port — and
      # would always return 404 for /genesispeers, leaving an empty file
      # that babble could not parse.
      if [[ -n "${ROOT_NODE_ENTITY:-}" ]]; then
        root_entity="$ROOT_NODE_ENTITY"
      elif [[ -n "${ROOT_NODE:-}" ]]; then
        root_host="${ROOT_NODE%%:*}"
        root_tcp="${ROOT_NODE##*:}"
        if [[ "$root_host" == "$root_tcp" ]]; then
          die "mode 3: ROOT_NODE='${ROOT_NODE}' must be host:port"
        fi
        root_chain=$((root_tcp + 4))
        root_entity="http://${root_host}:${root_chain}"
      else
        die "mode 3: no local peers.genesis.json AND ROOT_NODE is unset"
      fi

      log "fetching peers from ${root_entity}"
      # Retry to absorb transient races during cluster startup. The head
      # node's chain API may not be ready the instant a follower runs
      # shardchain.sh; a short bounded retry loop avoids spurious failures
      # without masking a real outage.
      fetched=false
      for attempt in 1 2 3 4 5 6 7 8 9 10; do
        if curl --fail --silent --show-error --location \
              --max-time 5 \
              -H "Work-Chain-Id: ${workchainId}" \
              -H "Shard-Chain-Id: ${shardchainId}" \
              "$root_entity/genesispeers" -o "$dest/peers.genesis.json" \
           && [[ -s "$dest/peers.genesis.json" ]]; then
          fetched=true
          break
        fi
        log "attempt ${attempt}: ${root_entity}/genesispeers not ready, retrying in 2s"
        sleep 2
      done
      $fetched || die "mode 3: could not fetch /genesispeers from ${root_entity}"

      if ! curl --fail --silent --show-error --location \
              --max-time 5 \
              -H "Work-Chain-Id: ${workchainId}" \
              -H "Shard-Chain-Id: ${shardchainId}" \
              "$root_entity/peers" -o "$dest/peers.json" \
         || [[ ! -s "$dest/peers.json" ]]; then
        # /peers is allowed to be slightly behind /genesispeers; use the
        # genesis set as the current set rather than failing outright.
        log "warning: ${root_entity}/peers not available, using genesis as current"
        cp "$dest/peers.genesis.json" "$dest/peers.json" \
          || die "cp peers.genesis.json -> peers.json"
      fi
    fi
    ;;

  *)
    die "unknown peersMode: $peersMode (expected 1, 2, or 3)"
    ;;
esac

log "ok: ${dest} (mode=${peersMode})"
exit 0
