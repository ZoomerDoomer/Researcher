# Manual-prune historical backfill

## Purpose

Build the complete Researcher UTXO/spend-event history while avoiding permanent storage of the entire raw Bitcoin blockchain.

Bitcoin Core still downloads and validates every historical block. Researcher consumes those validated blocks and persists its own compact state/events before old raw block files are released.

## Required Core mode

Use:

```text
prune=1
```

This means **manual pruning**.

Do not use an automatic target such as:

```text
prune=2000
prune=50000
```

for the historical backfill. Automatic pruning can delete data based on Core's disk target independently of Researcher's progress.

Researcher checks `getblockchaininfo.automatic_pruning` and refuses the backfill unless it is explicitly false.

## Safety invariant

For every batch:

1. read only blocks Core currently retains;
2. apply them to Researcher's deterministic UTXO state machine;
3. atomically commit UTXOs, block events and canonical tip to redb;
4. compute a prune watermark from the **committed** tip;
5. call `pruneblockchain` only for blocks older than the configured safety lag.

The prune RPC is never called from uncommitted state.

## Defaults

```text
batch blocks:       5,000
prune lag:         10,000 blocks
poll interval:          5 seconds
minimum prune lag:  1,000 blocks
```

Example:

```bash
cargo run --locked -p researcher -- backfill \
  --cookie-file /path/to/.cookie \
  --db researcher.redb
```

The existing smoke-test database may be reused if it is compatible and Core still retains the next block after its stored tip.

## Resume behavior

If Researcher exits:

- the last committed batch remains valid;
- no later prune request can be issued by the stopped process;
- manual-prune Core retains newly downloaded data;
- restarting `backfill` resumes from the durable Researcher tip.

Researcher fails closed if Core's `pruneheight` has advanced past the next block required by the database.

## Disk-risk tradeoff

Manual pruning deliberately favors data integrity. If Bitcoin Core continues Initial Block Download while Researcher is stopped for a long period, Core's raw block storage can grow substantially because nothing automatically deletes it.

Operational rule: if backfill will remain stopped, stop Bitcoin Core as well or monitor free disk space.

## Completion

Historical acquisition completes when:

- Bitcoin Core leaves Initial Block Download; and
- Researcher's durable tip equals Core's validated block height.

The resulting redb database is then the durable historical source for Researcher's raw UTXO lifecycle data. Raw Core block files older than the safety lag are no longer required for ordinary research queries.
