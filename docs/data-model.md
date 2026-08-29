# Raw data model

## UtxoEntry

Represents one currently spendable outpoint in the indexer's state.

| Field | Meaning |
| --- | --- |
| `value_sat` | Output value in satoshis |
| `created_height` | Block height that created the output |
| `created_timestamp` | Raw Bitcoin block header timestamp |
| `is_coinbase` | Whether the output originated from a coinbase transaction |

The outpoint `(txid, vout)` is the key. Outputs Bitcoin Core classifies as unspendable (`OP_RETURN`-prefixed scripts or scripts larger than 10,000 bytes) are deliberately excluded, matching chainstate semantics rather than treating them as permanently dormant UTXOs.

## SpendEvent

Produced when a later transaction consumes an existing UTXO.

| Field | Meaning |
| --- | --- |
| `outpoint` | Consumed outpoint |
| `spending_txid` | Transaction that consumed it |
| `value_sat` | Value of the consumed output |
| `created_height` | Creation height |
| `spent_height` | Spend height |
| `created_timestamp` | Raw creation-block timestamp |
| `spent_timestamp` | Raw spend-block timestamp |
| `is_coinbase` | Whether the consumed output originated from a coinbase transaction |
| `age_blocks` | Exact chain-order distance |
| `timestamp_delta_seconds` | Difference between raw block timestamps |

### Timestamp warning

Bitcoin block timestamps are miner supplied and are not a monotonic global clock. Therefore `timestamp_delta_seconds` can be negative even when `age_blocks > 0`. Research requiring elapsed wall-clock time should later evaluate Median Time Past or an external time normalization.

### No economic semantics in the raw layer

The following names are deliberately avoided:

- `cost_basis`
- `profit`
- `sale`
- `holder`
- `owner`

A future price join may derive a **last-moved return**, but even that remains a proxy rather than an investor's purchase price.

### BIP30 overwrite note

The two grandfathered historical duplicate-coinbase cases are state overwrites, not spend events. Before research datasets are finalized, those replacements must be explicitly marked or excluded so they cannot be mistaken for censored long-lived UTXOs.
