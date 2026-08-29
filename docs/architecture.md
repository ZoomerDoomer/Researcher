# Architecture

## Goal

Build a deterministic Bitcoin UTXO event pipeline that can later support behavioral research without confusing protocol-observable facts with economic interpretation.

## Stage 1: correctness core

```text
validated Bitcoin blocks
        |
        v
UTXO state machine
   |            |
   |            +--> reversible block undo
   |
   +--> spend events
   +--> replacement events (BIP30 only)
```

The first milestone intentionally uses an in-memory `HashMap`. This is not the production storage choice. It makes state transitions, atomicity, rollback, and tests easy to validate before introducing persistence.

## Stage 2: Bitcoin Core adapter and chain coordination

```text
Bitcoin Core JSON-RPC
        |
        v
BlockSource
        |
        v
ChainIndexer
  |   continuity: height/hash/prev_hash
  |   exact chain policy
  |   bounded undo history
  v
UTXO state machine
```

Bitcoin Core remains the consensus validator. This project indexes blocks Bitcoin Core already validated; it does not reimplement proof-of-work, scripts, coinbase maturity, or consensus validation.

The adapter uses Core's JSON-RPC API with cookie or user/password authentication. It deliberately starts with RPC rather than direct `blk*.dat` parsing. Binary/direct-file optimization is deferred until profiling proves RPC is a bottleneck.

### Chain-specific exceptions

- Height 0: the genesis coinbase output is not inserted into the spendable UTXO state.
- Mainnet 91842 and 91880: duplicate coinbase txids may overwrite still-unspent outpoints, but only when both height and canonical block hash match.
- The overwritten earlier outputs from heights 91722 and 91812 remain part of the historical state until replacement. Their lifecycle ends via a `ReplacementEvent`, never a `SpendEvent`.

This distinction matters for research: a BIP30 replacement is not holder behavior.

## Stage 3: durable scalable storage

The in-memory state is not suitable for a full mainnet scan. Before that scan:

- replace the in-memory UTXO map with a compact embedded key-value store;
- make state changes, event batches, and checkpoint advancement crash-consistent;
- write immutable spend/replacement-event batches to Parquet;
- retain enough undo data for ordinary reorgs and fall back to a durable checkpoint for deeper recovery;
- use DuckDB/Polars/Python for research;
- keep PostgreSQL, if needed, for small aggregates/metadata rather than raw chain history.

A checkpoint that stores only a block height/hash is **not** sufficient: the corresponding UTXO state and emitted-event position must be durably consistent with it.

## Research boundary

A spend event means only: an existing spendable UTXO was consumed by a transaction.

It does **not** prove:

- a sale;
- a change of beneficial owner;
- realized profit;
- an exchange deposit;
- accumulation or distribution.

Those are later heuristic layers and must never be mixed into the raw event layer.

## Timestamp boundary

Raw Bitcoin block timestamps are miner supplied and not a monotonic global clock. `age_blocks` is the canonical ordering measure. Raw timestamp deltas are retained as data, not treated as exact elapsed time.
