# Researcher

Bitcoin on-chain research infrastructure focused on building a correct raw UTXO event dataset before adding economic interpretation.

## Current milestone

The repository currently contains **Stage 1: the deterministic UTXO state-machine core**.

It can:

- connect ordered Bitcoin transactions to an in-memory UTXO state;
- emit raw spend events;
- roll a connected block back exactly;
- apply a block atomically (failure restores the previous state);
- reject duplicate unspent outpoints by default;
- explicitly model the historical BIP30 overwrite requirement without enabling it globally;
- preserve the fact that Bitcoin block timestamps are not monotonic.

It intentionally does **not** yet:

- connect to Bitcoin Core;
- persist the UTXO set;
- write Parquet;
- track the mempool;
- infer addresses/entities/exchanges;
- label spends as sales or realized profit.

## Why this order

The expensive part is not downloading ~700+ GB of chain data. The dangerous part is silently building the wrong state model and discovering it after a full scan. We therefore prove deterministic connect/disconnect behavior first, then add Bitcoin Core, then persistent storage, then research features.

## Layout

```text
crates/indexer-core/       deterministic UTXO state machine
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

Add a Bitcoin Core block-source adapter plus chain continuity/checkpoint handling. Only after that passes the integration gates should the in-memory map be replaced by a persistent KV store and spend events be written to Parquet.
