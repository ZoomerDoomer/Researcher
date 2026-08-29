# Researcher

Bitcoin on-chain research infrastructure focused on building a correct raw UTXO event dataset before adding economic interpretation.

## Current milestone

Stages 1 and the first half of Stage 2 are implemented:

- deterministic Rust UTXO state machine;
- raw spend events without labeling them as sales;
- explicit BIP30 replacement events;
- exact reversible block undo;
- Bitcoin-Core-compatible exclusion of provably unspendable scripts;
- genesis-output exclusion;
- chain-aware height/hash/previous-hash checks;
- Bitcoin Core JSON-RPC block source abstraction;
- shallow reorg reconciliation;
- exact mainnet BIP30 policy by height and block hash.

It intentionally does **not** yet:

- persist the production UTXO set;
- write Parquet;
- claim crash-safe resume;
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

## Next milestone

Add durable UTXO/event storage and an atomic checkpoint protocol. A full mainnet scan should not begin until that layer passes crash/restart tests.
