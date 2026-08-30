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

- the production UTXO set lives in an embedded ACID store rather than a process-wide in-memory map;
- only UTXOs touched or potentially collided with by the current block are preloaded into memory;
- UTXO mutation, canonical block events, and tip advancement commit in one database transaction;
- reopening the database restores the same tip, UTXOs, and block-event history;
- disconnecting a persisted block exactly reverses its durable UTXO effects;
- same-block temporary outputs are not resurrected during durable rollback;
- a failed block cannot advance the tip or leave committed partial UTXO/event changes;
- durable sync can reconcile a reorg after process restart;
- restart does not require rescanning from Genesis;
- Parquet remains an export layer and cannot become more authoritative than the committed event store;
- dependency/toolchain versions are pinned before the full mainnet run;
- a bounded sync stops exactly at the requested canonical height and fails if the node has not reached that height;
- the CLI cannot accidentally start a sync without explicit RPC authentication;
- sync refuses a Core node on the wrong network;
- bounded sync may run during Initial Block Download only if the node has already validated through the requested target height;
- unbounded sync refuses a node still in Initial Block Download;
- bounded sync may use a node with pruning configured only while `prune_height == 0`, meaning Genesis-era block data has not yet been discarded;
- sync refuses a node once historical pruning has advanced above Genesis, before the research database is mutated.

## Gate D — low-disk historical backfill

Must pass before unattended full-history acquisition:

- Core must report pruning enabled with `automatic_pruning=false`;
- automatic pruning is rejected because it can discard blocks before Researcher commits them;
- the next block Researcher requires must be at or above Core's first retained block;
- backfill commits a bounded batch before any prune RPC is attempted;
- prune height is derived only from the durable Researcher tip;
- a minimum 1,000-block lag is enforced, with 10,000 blocks as the default;
- a stopped/crashed Researcher process cannot cause additional block deletion;
- a resumed Researcher database may continue after older raw blocks were pruned if its next required block is still retained;
- reaching the end of IBD plus matching the current Core tip terminates the historical backfill cleanly.

## Research gate

No trading or behavioral claim may be promoted from raw events until it is tested out-of-sample and clearly labeled as observation, proxy, heuristic, or inference.
