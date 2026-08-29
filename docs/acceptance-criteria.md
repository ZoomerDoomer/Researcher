# Acceptance criteria

## Gate A — deterministic state machine

Must pass before Bitcoin Core integration:

- every normal input must reference a currently unspent outpoint;
- consuming an outpoint removes it exactly once;
- every output creates one outpoint;
- transaction order inside a block is respected;
- a later transaction in the same block can consume an earlier transaction's output;
- a failed block application leaves state byte-for-byte/logically equivalent to the state before the call;
- disconnecting a connected block restores the exact previous state;
- duplicate unspent outpoints fail closed unless an explicit policy allows overwrite;
- overwrite rollback restores the displaced entry;
- raw block timestamps are not assumed monotonic.

## Gate B — Bitcoin Core integration

Must pass before a long mainnet scan:

- block height/hash/previous-hash continuity is verified;
- Genesis-to-fixture scans are deterministic across repeated runs;
- checkpoints resume without duplicating spend events;
- simulated crash between batches resumes consistently;
- connected/disconnected block handling preserves state;
- exact BIP30 mainnet exceptions are tested using real block hashes;
- representative SegWit and Taproot blocks decode successfully;
- the adapter never treats this project as a consensus validator.

## Gate C — scalable storage

Must pass before full-chain production indexing:

- resident memory does not grow with total processed history;
- UTXO storage grows primarily with the live UTXO set;
- spend events are appended in bounded batches;
- restart does not require rescanning from Genesis;
- a full dataset can be queried without loading it fully into RAM.

## Research gate

No trading or behavioral claim may be promoted from raw events until it is tested out-of-sample and clearly labeled as observation, proxy, heuristic, or inference.
