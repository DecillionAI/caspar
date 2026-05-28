#!/bin/bash
# shardchain.sh — Bootstrap a new babble shard directory.
#
# Usage: shardchain.sh <storageRoot> <workchainId> <shardchainId> <peersMode>
#   peersMode 1 = new shard: copy local genesis only
#   peersMode 2 = IS_HEAD node: copy genesis as both genesis + current peers
#   peersMode 3 = follower node: fetch peers from ROOT_NODE_ENTITY or use local
#
# Environment variables:
#   BABBLE_DATA_DIR  — directory holding this node's priv_key, key.pub,
#                      and peers.genesis.json (default: /root/.babble)
#   ROOT_NODE_ENTITY — base URL for fetching peers in mode 3
#                      (default: http://localhost:8079 for local clusters)

storageRoot=${1:-"/home/keyhan/data"}
workchainId=${2:-"main"}
shardchainId=${3:-"shard-main"}
peersMode=${4:-"3"}

babble_src="${BABBLE_DATA_DIR:-/root/.babble}"
dest="${storageRoot}/chains/${workchainId}/${shardchainId}"

mkdir -p "$dest"

# Copy this node's crypto keys into the shard directory.
cp "$babble_src/key.pub"   "$dest/key.pub"
cp "$babble_src/priv_key"  "$dest/priv_key"

if [ "$peersMode" = "1" ]; then
    cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json"
elif [ "$peersMode" = "2" ]; then
    cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json"
    cp "$babble_src/peers.genesis.json" "$dest/peers.json"
else
    # Mode 3 — follower node. Prefer a locally pre-generated peers.genesis.json
    # (written by run-nodes.sh for local triple-node clusters) so the cluster
    # can bootstrap without a live remote API.
    if [ -f "$babble_src/peers.genesis.json" ]; then
        echo "Using local peers.genesis.json from $babble_src"
        cp "$babble_src/peers.genesis.json" "$dest/peers.genesis.json"
        cp "$babble_src/peers.genesis.json" "$dest/peers.json"
    else
        # Fall back to fetching from the head node's entity API.
        root_entity="${ROOT_NODE_ENTITY:-http://localhost:8079}"
        echo "Fetching peers.genesis.json from $root_entity"
        curl -fsSL \
            -H "Work-Chain-Id: ${workchainId}" \
            -H "Shard-Chain-Id: ${shardchainId}" \
            "$root_entity/genesispeers" > "$dest/peers.genesis.json"
        echo "Fetching peers.json from $root_entity"
        curl -fsSL \
            -H "Work-Chain-Id: ${workchainId}" \
            -H "Shard-Chain-Id: ${shardchainId}" \
            "$root_entity/peers" > "$dest/peers.json"
    fi
fi
