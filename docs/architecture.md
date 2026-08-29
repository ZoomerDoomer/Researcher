# Architecture

## Goal

Build a deterministic Bitcoin UTXO event pipeline that can later support behavioral research without confusing protocol-observable facts with economic interpretation.

## Stage 1: correctness core

```text
validated Bitcoin blocks
        |
        v
transaction decoder
        |
        v
UTXO state machine
   |            |
   |            +--> reversible block undo
   |
   +--> spend events
```

The first milestone intentionally uses an in-memory `HashMap`. This is not the production storage choice. It makes state transitions, atomicity, rollback, and tests easy to validate before introducing persistence.

## Stage 2: Bitcoin Core adapter

The next layer will:

- read already-validated blocks from a local Bitcoin Core node;
- verify height/hash/previous-hash continuity;
- enable the overwrite policy only for the two grandfathered mainnet BIP30 exception blocks;
- persist checkpoints;
- reconnect from the last durable checkpoint after restart;
- handle block disconnect/connect events in reverse/forward order.

Bitcoin Core remains the consensus validator. This project indexes validated chain data; it does not reimplement Bitcoin consensus.

## Stage 3: scalable storage

Only after Stage 1 and Stage 2 pass correctness gates:

- replace the in-memory UTXO map with a compact embedded key-value store;
- write immutable spend-event batches to Parquet;
- use DuckDB/Polars/Python for research;
- keep PostgreSQL, if needed, for small aggregates/metadata rather than the raw chain.

## Research boundary

A spend event means only: an existing UTXO was consumed by a transaction.

It does **not** prove:

- a sale;
- a change of beneficial owner;
- realized profit;
- an exchange deposit;
- accumulation or distribution.

Those are later heuristic layers and must never be mixed into the raw event layer.

## Historical edge case: BIP30

Mainnet contains two grandfathered duplicate-coinbase violations at heights 91842 and 91880. A generic duplicate-unspent-outpoint overwrite is unsafe, so the state machine rejects it by default and exposes an explicit policy switch. A future chain-aware adapter must gate that switch by exact mainnet height **and block hash**, never height alone.
