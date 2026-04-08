# Verifiable Chain Extension (Base Toolkit)

This extension provides a minimal **container-vm mini app base** for zk-style verifiable execution flows built on top of Caspar appengine host functions.

## What this mini app does

- Supports **2 separate phases**:
  1. **Execution phase (executor node):** receives `onchainExecutionRequest`, checks signature approval, runs `runVm` (`vmType: "elpify"`), then publishes result+proof on-chain as `onchainExecutionProofShared`.
  2. **Verification phase (verifier nodes):** receives `onchainExecutionProofShared`, calls `elpifyProof`, publishes vote as `onchainVerificationVote`, and emits tally messages.
- Returns execution result metadata back to requester as an on-chain message from the executor node.
- Adds a **PoS validator election** flow (commit/reveal + stake-weighted randomness) published on-chain.
- Adds a **smart sharding manager** that consumes machine load reports, computes balanced shard plans, and emits shard update actions via chain-driver API host calls.

## Node roles

Set role and identity via environment variables:

- `VERIFIABLE_NODE_ROLE=executor` for the primary execution node.
- `VERIFIABLE_NODE_ROLE=verifier` for verifier nodes.
- `VERIFIABLE_NODE_ID=<unique-node-id>` for vote attribution.

## New on-chain protocol messages

- `validatorStakeAnnouncement`: node stake declarations.
- `validatorCommit`: commit hash for randomness reveal (`sha256(period:nodeId:nonce)`).
- `validatorReveal`: reveal nonce + stake for a period.
- `validatorElectionTick`: trigger period election and winner publication.
- `onchainValidatorElectionResult`: elected validators with deterministic seed.
- `machineLoadReport`: per-machine VM cost reports for shard balancing.
- `onchainShardPlan`: rebalanced shard grouping publication.

## Chain driver integration

The extension calls host op `chainDriverApi` with:
- `upsertSubChain` to create/update shard sub-chains.
- `rebalanceSubChains` to trigger split/merge/rebalance actions.

## Request payload shape

```json
{
  "type": "onchainExecutionRequest",
  "data": "{\"type\":\"onchainExecutionRequest\",\"requestId\":\"req-1\",\"machineId\":\"m1\",\"pointId\":\"p1\",\"masmPath\":\"/programs/demo.masm\",\"inputs\":[1,2],\"outputs\":[3],\"proof\":[1,2,3],\"userId\":\"u1\",\"userSignature\":\"...\",\"executionPayload\":{\"result\":3}}"
}
```

## Build container

```bash
docker build -t caspar-verifiable-chain extensions/verifiable-chain
```

## Notes

- Signature verification in this base is intentionally simple (`sha256(userId:requestId)` in base64) and should be replaced with the chain's real signing/verification standard.
- The extension is structured to be reused by any zk-verifiable chain setup using Caspar host APIs.
