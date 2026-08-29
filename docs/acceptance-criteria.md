# Acceptance criteria

## Gate A — deterministic state machine

Must pass before Bitcoin Core integration:

- every normal input must reference a currently unspent outpoint;
- consuming an outpoint removes it exactly once;
- every spendable output creates one outpoint;
- `OP_RETURN`-prefixed and >10,000-byte scripts are excluded from the UTXO state, matching Bitcoin Core;
- transaction order inside a block is respected;
- a later transaction in the same block can consume an earlier transaction's output;
- a failed block application restores the exact previous state;
- disconnecting a connected block restores the exact previous state;
- duplicate unspent outpoints fail closed unless an explicit chain policy allows overwrite;
- an allowed overwrite emits a replacement event and remains reversible;
- coinbase provenance survives from UTXO creation into the spend event;
- raw block timestamps are not assumed monotonic.

## Gate B — Bitcoin Core integration and chain coordination

Must pass before persistent storage work is accepted:

- a Bitcoin Core JSON-RPC source supports cookie and user/password auth;
- source tip, block hash by height, and decoded block retrieval are represented behind a small `BlockSource` interface;
- an empty indexer can start only at height 0;
- genesis outputs are excluded from the UTXO state;
- block height/hash/previous-hash continuity is verified;
- a source returning bytes for the wrong requested hash is rejected;
- an ordinary shallow reorg disconnects to the common ancestor and reconnects the canonical branch;
- a reorg deeper than retained undo fails closed;
- BIP30 exceptions require exact mainnet height and block hash;
- BIP30 replacements are not emitted as spend events;
- the adapter never treats this project as a consensus validator.

Durable restart/checkpoint guarantees move to Gate C because a height/hash checkpoint without the matching persistent UTXO state is not a valid resume point.

## Gate C — durable scalable storage

Must pass before full-chain production indexing:

- resident memory does not grow with total processed history;
- UTXO storage grows primarily with the live UTXO set;
- spend and replacement events are appended in bounded batches;
- state mutation, event durability, and checkpoint advancement are crash-consistent;
- restart does not require rescanning from Genesis;
- simulated crash/restart cannot duplicate or lose committed events;
- a full dataset can be queried without loading it fully into RAM;
- dependency/toolchain versions are pinned for reproducible research runs.

## Research gate

No trading or behavioral claim may be promoted from raw events until it is tested out-of-sample and clearly labeled as observation, proxy, heuristic, or inference.
