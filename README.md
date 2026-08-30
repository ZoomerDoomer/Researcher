# Researcher

Bitcoin on-chain research infrastructure focused on building a correct raw UTXO event dataset before adding economic interpretation.

## Current milestone

Stages 1–3 now have an implemented correctness path:

- deterministic Rust UTXO state machine;
- raw spend events without labeling them as sales;
- explicit BIP30 replacement events;
- exact reversible block undo;
- Bitcoin-Core-compatible exclusion of provably unspendable scripts;
- genesis-output exclusion;
- chain-aware height/hash/previous-hash checks;
- Bitcoin Core JSON-RPC block source abstraction;
- shallow reorg reconciliation;
- exact mainnet BIP30 policy by height and block hash;
- redb-backed durable UTXO state;
- atomic per-block event + UTXO + tip commits;
- durable block rollback and post-restart reorg reconciliation;
- bounded sync to an explicit target height;
- minimal `researcher doctor/sync/status` CLI for the real-node smoke test;
- preflight rejection of wrong-network nodes and nodes that have already discarded Genesis-era block data;
- bounded smoke sync during IBD once the requested historical height is locally validated;
- pruning may be configured for the smoke test as long as actual historical pruning has not started yet;
- manual-prune backfill mode that persists bounded batches before asking Core to delete old blocks.

It intentionally does **not** yet:

- export committed events to Parquet;
- run a full mainnet performance/soak test;
- track the mempool;
- infer addresses/entities/exchanges;
- label spends as sales or realized profit.

## Why this order

The dangerous failure mode is not that a 700+ GB scan takes time. It is silently generating a structurally wrong research dataset. The project therefore separates:

```text
Bitcoin Core (consensus validation)
        ↓
chain coordination
        ↓
raw UTXO lifecycle
        ↓
durable event storage
        ↓
heuristics / market data
        ↓
research hypotheses
```

## Layout

```text
crates/indexer-core/       deterministic UTXO state machine
crates/bitcoin-source/     Bitcoin Core JSON-RPC block source
crates/chain-indexer/      chain continuity + reorg coordination
crates/storage-redb/       durable ACID UTXO/event/tip storage
crates/researcher-cli/     doctor/sync/status/backfill executable
docs/architecture.md       staged architecture and boundaries
docs/data-model.md         raw event semantics
docs/acceptance-criteria.md correctness/scaling gates
.github/workflows/ci.yml   fmt + clippy + tests
```

## Run locally

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

### Inspect local research state

```bash
cargo run -p researcher -- status --db researcher.redb
```

### Check the Bitcoin Core node first

```bash
cargo run --locked -p researcher -- doctor \
  --cookie-file /path/to/.cookie \
  --target-height 1000
```

For the first bounded smoke test the node does **not** need to be fully synced. It only needs to have validated at least the requested target height and still retain Genesis-era block data. Pruning may be configured; it becomes a blocker only after Core has actually deleted the old blocks we need.

### Bounded Bitcoin Core smoke sync

Cookie authentication:

```bash
cargo run --locked -p researcher -- sync \
  --cookie-file /path/to/.cookie \
  --target-height 1000 \
  --db researcher.redb
```

The explicit target height is intentional: the first real-node run should validate a small deterministic range before any full-chain scan is attempted. See `docs/real-node-smoke-test.md` for the exact acceptance sequence.

### Low-disk historical backfill

For the complete historical dataset without retaining the whole raw blockchain, Bitcoin Core must use **manual pruning** (`prune=1`), not an automatic size target.

```bash
cargo run --locked -p researcher -- backfill \
  --cookie-file /path/to/.cookie \
  --db researcher.redb
```

Defaults:

- commit at most 5,000 blocks per batch;
- retain a 10,000-block safety lag before asking Core to prune;
- poll Core every 5 seconds while caught up to the current IBD tip.

Researcher refuses automatic pruning. It also refuses to continue if Core has already deleted the next historical block Researcher needs. If the backfill process is stopped, Core in manual-prune mode will stop deleting old blocks; stop Core as well if the backfill will remain offline for a long time to avoid unbounded disk growth.

See `docs/manual-prune-backfill.md`.

## Next milestone

Validate manual-prune backfill against the same local Bitcoin Core installation before allowing it to run through the full Initial Block Download.
