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
- minimal `researcher sync/status` CLI for the real-node smoke test.

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
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

### Inspect local research state

```bash
cargo run -p researcher -- status --db researcher.redb
```

### Bounded Bitcoin Core smoke sync

Cookie authentication:

```bash
cargo run -p researcher -- sync \
  --cookie-file /path/to/.cookie \
  --target-height 1000 \
  --db researcher.redb
```

The explicit target height is intentional: the first real-node run should validate a small deterministic range before any full-chain scan is attempted.

## Next milestone

Pin the generated `Cargo.lock`, run the bounded CLI against a real local Bitcoin Core node, and only then decide whether profiling justifies block-source optimizations or Parquet export work.
