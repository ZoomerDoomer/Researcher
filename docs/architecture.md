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

The durable layer uses redb as the primary state/event store. For every connected block, one ACID transaction commits:

- live UTXO mutations;
- the canonical per-block spend/replacement event bundle;
- the canonical chain tip.

No checkpoint can therefore get ahead of either the UTXO state or its events. The full historical UTXO set is never loaded into RAM: only outpoints touched or potentially collided with by the current block are preloaded into the already-tested state machine.

Per-block event bundles also form durable undo data. A disconnect removes outputs created by the block, restores older UTXOs consumed by the block, restores BIP30-replaced entries, deletes the old canonical event bundle, and moves the tip back in the same transaction.

Parquet is deliberately downstream. It will be exported from committed block-event bundles for DuckDB/Polars/Python research rather than serving as the transactional source of truth.

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


## Stage 4: low-disk full-history acquisition

A full archival Bitcoin Core node is not required permanently. For historical acquisition, Core can run with manual pruning (`prune=1`) while Researcher owns the prune watermark.

```text
Bitcoin Core validates/downloads blocks
              |
              v
Researcher reads bounded batch
              |
              v
redb ACID commit
UTXOs + events + canonical tip
              |
              v
pruneblockchain(committed_tip - safety_lag)
              |
              v
Core may delete only older raw block files
```

The order is invariant: **commit first, prune second**.

Automatic size-based pruning is intentionally rejected for this workflow. It would let Core advance its deletion watermark independently of Researcher and could create an unrecoverable historical gap. Manual pruning instead fails on the safe side: if Researcher stops, disk usage grows but historical data is not deleted.

The default safety lag is 10,000 blocks and the default Researcher batch is 5,000 blocks. These are operational safety values rather than economic assumptions and can be changed later only with explicit CLI options.
