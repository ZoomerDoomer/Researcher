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
- pruning may be configured for the smoke test as long as actual historical pruning has not started yet.

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
crates/researcher-cli/     bounded sync/status executable
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

## Next milestone

Run `doctor` and then the bounded CLI against the local Bitcoin Core node while Genesis-era blocks are still retained. Only after that smoke test should profiling decide whether block-source optimization or Parquet export work is justified.
