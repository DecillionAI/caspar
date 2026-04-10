# Verifiable Chain Extension (Base Toolkit) 🧩

> Updated: **2026-04-10**

This extension provides a minimal container-VM mini-app base for zk-style verifiable execution flows built on top of Caspar appengine host functions.

## What This Mini App Does

Supports two phases:

1. **Execution phase (executor node):**
   - receives `onchainExecutionRequest`
   - validates signature approval
   - runs `runVm` (`vmType: "elpify"`)
   - publishes result + proof as `onchainExecutionProofShared`
2. **Verification phase (verifier nodes):**
   - receives `onchainExecutionProofShared`
   - calls `elpifyProof`
   - publishes vote as `onchainVerificationVote`
   - emits tally messages

Also included:

- execution result metadata returned to requester as on-chain message from executor node
- PoS validator election flow (commit/reveal + stake-weighted randomness)
- smart sharding manager that consumes machine load reports, computes shard plans, and emits shard update actions via chain-driver host calls

## Node Roles

Set role and identity via environment variables:

- `VERIFIABLE_NODE_ROLE=executor` for primary execution node
- `VERIFIABLE_NODE_ROLE=verifier` for verifier nodes
- `VERIFIABLE_NODE_ID=<unique-node-id>` for vote attribution

## On-Chain Protocol Messages

- `validatorStakeAnnouncement`
- `validatorCommit` (`sha256(period:nodeId:nonce)`)
- `validatorReveal`
- `validatorElectionTick`
- `onchainValidatorElectionResult`
- `machineLoadReport`
- `onchainShardPlan`

## Chain Driver Integration

The extension calls host op `chainDriverApi` with:

- `upsertSubChain` to create/update shard sub-chains
- `rebalanceSubChains` to trigger split/merge/rebalance actions

## Request Payload Shape

```json
{
  "type": "onchainExecutionRequest",
  "data": "{\"type\":\"onchainExecutionRequest\",\"requestId\":\"req-1\",\"machineId\":\"m1\",\"storeId\":\"p1\",\"masmPath\":\"/programs/demo.masm\",\"inputs\":[1,2],\"outputs\":[3],\"proof\":[1,2,3],\"userId\":\"u1\",\"userSignature\":\"...\",\"executionPayload\":{\"result\":3}}"
}
```

## Build

```bash
docker build -t caspar-verifiable-chain extensions/verifiable-chain
```

## Notes ⚠️

- Signature verification in this base is intentionally simple (`sha256(userId:requestId)` in base64).
- Replace it with your chain's production signing/verification standard before deployment.
- Structure is intended for reuse in broader zk-verifiable flows using Caspar host APIs.
