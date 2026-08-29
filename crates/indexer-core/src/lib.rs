use bitcoin::{OutPoint, Script, Transaction, Txid};
use std::collections::HashMap;

const MAX_SCRIPT_SIZE: usize = 10_000;

fn is_core_unspendable(script: &Script) -> bool {
    script.is_op_return() || script.len() > MAX_SCRIPT_SIZE
}
use thiserror::Error;

/// Policy switches that must only be enabled by a chain-aware caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectPolicy {
    /// Allows a newly created output to overwrite an already-unspent outpoint.
    ///
    /// This is disabled by default. A future mainnet adapter may enable it only
    /// for the two grandfathered BIP30 duplicate-coinbase blocks.
    pub allow_unspent_overwrite: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UtxoEntry {
    pub value_sat: u64,
    pub created_height: u32,
    pub created_timestamp: u32,
    pub is_coinbase: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendEvent {
    pub outpoint: OutPoint,
    pub spending_txid: Txid,
    pub value_sat: u64,
    pub created_height: u32,
    pub spent_height: u32,
    pub created_timestamp: u32,
    pub spent_timestamp: u32,
    /// Whether the consumed output originated from a coinbase transaction.
    pub is_coinbase: bool,
    /// Canonical ordering-based age. This is always non-negative for a valid chain.
    pub age_blocks: u32,
    /// Raw block-timestamp delta. Bitcoin block timestamps are not a monotonic clock,
    /// so this value may be negative and must not be treated as exact elapsed time.
    pub timestamp_delta_seconds: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApplyError {
    #[error("unknown or already-spent prevout {outpoint}")]
    UnknownPrevout { outpoint: OutPoint },

    #[error("attempted to overwrite an unspent outpoint {outpoint}")]
    DuplicateUnspentOutpoint { outpoint: OutPoint },

    #[error(
        "outpoint {outpoint} was created at height {created_height} but spent at earlier height {spent_height}"
    )]
    SpendBeforeCreate {
        outpoint: OutPoint,
        created_height: u32,
        spent_height: u32,
    },
}

#[derive(Clone, Debug)]
enum UndoOp {
    RestoreSpent {
        outpoint: OutPoint,
        entry: UtxoEntry,
    },
    RevertCreate {
        outpoint: OutPoint,
        previous: Option<UtxoEntry>,
    },
}

/// Undo data for exactly one connected block.
///
/// Callers must disconnect blocks in reverse connection order.
#[derive(Clone, Debug)]
pub struct BlockUndo {
    ops: Vec<UndoOp>,
}

#[derive(Clone, Debug)]
pub struct ConnectedBlock {
    pub spend_events: Vec<SpendEvent>,
    pub undo: BlockUndo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UtxoState {
    utxos: HashMap<OutPoint, UtxoEntry>,
}

impl UtxoState {
    pub fn len(&self) -> usize {
        self.utxos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utxos.is_empty()
    }

    pub fn get(&self, outpoint: &OutPoint) -> Option<&UtxoEntry> {
        self.utxos.get(outpoint)
    }

    pub fn connect_block(
        &mut self,
        height: u32,
        timestamp: u32,
        transactions: &[Transaction],
    ) -> Result<ConnectedBlock, ApplyError> {
        self.connect_block_with_policy(height, timestamp, transactions, ConnectPolicy::default())
    }

    pub fn connect_block_with_policy(
        &mut self,
        height: u32,
        timestamp: u32,
        transactions: &[Transaction],
        policy: ConnectPolicy,
    ) -> Result<ConnectedBlock, ApplyError> {
        let mut undo_ops = Vec::new();
        let mut spend_events = Vec::new();

        for tx in transactions {
            let spending_txid = tx.compute_txid();
            let is_coinbase = tx.is_coinbase();

            if !is_coinbase {
                for input in &tx.input {
                    let outpoint = input.previous_output;
                    let Some(entry) = self.utxos.remove(&outpoint) else {
                        self.rollback_ops(undo_ops);
                        return Err(ApplyError::UnknownPrevout { outpoint });
                    };

                    undo_ops.push(UndoOp::RestoreSpent {
                        outpoint,
                        entry: entry.clone(),
                    });

                    let Some(age_blocks) = height.checked_sub(entry.created_height) else {
                        self.rollback_ops(undo_ops);
                        return Err(ApplyError::SpendBeforeCreate {
                            outpoint,
                            created_height: entry.created_height,
                            spent_height: height,
                        });
                    };

                    spend_events.push(SpendEvent {
                        outpoint,
                        spending_txid,
                        value_sat: entry.value_sat,
                        created_height: entry.created_height,
                        spent_height: height,
                        created_timestamp: entry.created_timestamp,
                        spent_timestamp: timestamp,
                        is_coinbase: entry.is_coinbase,
                        age_blocks,
                        timestamp_delta_seconds: i64::from(timestamp)
                            - i64::from(entry.created_timestamp),
                    });
                }
            }

            for (vout, output) in tx.output.iter().enumerate() {
                // Match Bitcoin Core chainstate semantics: provably unspendable
                // outputs are never inserted into the UTXO set.
                if is_core_unspendable(&output.script_pubkey) {
                    continue;
                }

                let outpoint = OutPoint::new(
                    spending_txid,
                    u32::try_from(vout).expect("transaction output count fits in u32"),
                );
                let new_entry = UtxoEntry {
                    value_sat: output.value.to_sat(),
                    created_height: height,
                    created_timestamp: timestamp,
                    is_coinbase,
                };

                let previous = self.utxos.insert(outpoint, new_entry);

                match previous {
                    Some(previous_entry) if !policy.allow_unspent_overwrite => {
                        self.utxos.insert(outpoint, previous_entry);
                        self.rollback_ops(undo_ops);
                        return Err(ApplyError::DuplicateUnspentOutpoint { outpoint });
                    }
                    previous => {
                        undo_ops.push(UndoOp::RevertCreate { outpoint, previous });
                    }
                }
            }
        }

        Ok(ConnectedBlock {
            spend_events,
            undo: BlockUndo { ops: undo_ops },
        })
    }

    pub fn disconnect_block(&mut self, undo: BlockUndo) {
        self.rollback_ops(undo.ops);
    }

    fn rollback_ops(&mut self, ops: Vec<UndoOp>) {
        for op in ops.into_iter().rev() {
            match op {
                UndoOp::RestoreSpent { outpoint, entry } => {
                    self.utxos.insert(outpoint, entry);
                }
                UndoOp::RevertCreate { outpoint, previous } => match previous {
                    Some(entry) => {
                        self.utxos.insert(outpoint, entry);
                    }
                    None => {
                        self.utxos.remove(&outpoint);
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        absolute::LockTime, transaction::Version, Amount, ScriptBuf, Sequence, TxIn, TxOut, Witness,
    };
    use std::str::FromStr;

    fn coinbase(value_sat: u64) -> Transaction {
        coinbase_with_script(value_sat, ScriptBuf::new())
    }

    fn coinbase_with_script(value_sat: u64, script_pubkey: ScriptBuf) -> Transaction {
        Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(value_sat),
                script_pubkey,
            }],
        }
    }

    fn spend(prevout: OutPoint, values_sat: &[u64]) -> Transaction {
        Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: prevout,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: values_sat
                .iter()
                .copied()
                .map(|value_sat| TxOut {
                    value: Amount::from_sat(value_sat),
                    script_pubkey: ScriptBuf::new(),
                })
                .collect(),
        }
    }

    fn unknown_outpoint() -> OutPoint {
        let txid =
            Txid::from_str("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid txid");
        OutPoint::new(txid, 0)
    }

    #[test]
    fn spend_removes_utxo_and_emits_event() {
        let mut state = UtxoState::default();
        let funding = coinbase(5_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);

        state.connect_block(1, 1_000, &[funding]).unwrap();
        let spending = spend(funding_outpoint, &[4_900]);

        let connected = state.connect_block(11, 7_000, &[spending]).unwrap();

        assert!(state.get(&funding_outpoint).is_none());
        assert_eq!(connected.spend_events.len(), 1);
        let event = &connected.spend_events[0];
        assert_eq!(event.value_sat, 5_000);
        assert_eq!(event.age_blocks, 10);
        assert_eq!(event.timestamp_delta_seconds, 6_000);
        assert!(event.is_coinbase);
    }

    #[test]
    fn op_return_output_is_not_added_to_utxo_state() {
        let mut state = UtxoState::default();
        let tx = coinbase_with_script(5_000, ScriptBuf::from_bytes(vec![0x6a]));
        let outpoint = OutPoint::new(tx.compute_txid(), 0);

        state.connect_block(1, 1_000, &[tx]).unwrap();

        assert!(state.get(&outpoint).is_none());
        assert!(state.is_empty());
    }

    #[test]
    fn oversized_script_output_is_not_added_to_utxo_state() {
        let mut state = UtxoState::default();
        let tx = coinbase_with_script(
            5_000,
            ScriptBuf::from_bytes(vec![0x51; MAX_SCRIPT_SIZE + 1]),
        );
        let outpoint = OutPoint::new(tx.compute_txid(), 0);

        state.connect_block(1, 1_000, &[tx]).unwrap();

        assert!(state.get(&outpoint).is_none());
        assert!(state.is_empty());
    }

    #[test]
    fn failed_block_is_atomic() {
        let mut state = UtxoState::default();
        let funding = coinbase(10_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        state.connect_block(1, 1_000, &[funding]).unwrap();

        let before = state.clone();
        let valid_first = spend(funding_outpoint, &[9_000]);
        let invalid_second = spend(unknown_outpoint(), &[1_000]);

        let err = state
            .connect_block(2, 2_000, &[valid_first, invalid_second])
            .unwrap_err();

        assert!(matches!(err, ApplyError::UnknownPrevout { .. }));
        assert_eq!(state, before);
    }

    #[test]
    fn disconnect_restores_exact_previous_state() {
        let mut state = UtxoState::default();
        let funding = coinbase(10_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        state.connect_block(1, 1_000, &[funding]).unwrap();

        let before = state.clone();
        let connected = state
            .connect_block(2, 2_000, &[spend(funding_outpoint, &[6_000, 3_000])])
            .unwrap();

        assert_ne!(state, before);
        state.disconnect_block(connected.undo);
        assert_eq!(state, before);
    }

    #[test]
    fn later_transaction_can_spend_output_created_in_same_block() {
        let mut state = UtxoState::default();
        let funding = coinbase(10_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        state.connect_block(1, 1_000, &[funding]).unwrap();

        let tx1 = spend(funding_outpoint, &[9_000]);
        let tx1_outpoint = OutPoint::new(tx1.compute_txid(), 0);
        let tx2 = spend(tx1_outpoint, &[8_000]);

        let connected = state.connect_block(2, 2_000, &[tx1, tx2]).unwrap();

        assert_eq!(connected.spend_events.len(), 2);
        assert_eq!(connected.spend_events[1].age_blocks, 0);
    }

    #[test]
    fn duplicate_unspent_outpoint_is_rejected_by_default() {
        let mut state = UtxoState::default();
        let duplicate = coinbase(5_000);
        state
            .connect_block(1, 1_000, std::slice::from_ref(&duplicate))
            .unwrap();
        let before = state.clone();

        let err = state.connect_block(2, 2_000, &[duplicate]).unwrap_err();

        assert!(matches!(err, ApplyError::DuplicateUnspentOutpoint { .. }));
        assert_eq!(state, before);
    }

    #[test]
    fn explicit_overwrite_policy_is_reversible_for_bip30_adapter() {
        let mut state = UtxoState::default();
        let duplicate = coinbase(5_000);
        let outpoint = OutPoint::new(duplicate.compute_txid(), 0);

        state
            .connect_block(1, 1_000, std::slice::from_ref(&duplicate))
            .unwrap();
        let before = state.clone();

        let connected = state
            .connect_block_with_policy(
                2,
                2_000,
                &[duplicate],
                ConnectPolicy {
                    allow_unspent_overwrite: true,
                },
            )
            .unwrap();

        assert_eq!(state.get(&outpoint).unwrap().created_height, 2);
        state.disconnect_block(connected.undo);
        assert_eq!(state, before);
    }

    #[test]
    fn timestamp_delta_is_not_assumed_monotonic() {
        let mut state = UtxoState::default();
        let funding = coinbase(5_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        state.connect_block(1, 2_000, &[funding]).unwrap();

        let connected = state
            .connect_block(2, 1_900, &[spend(funding_outpoint, &[4_000])])
            .unwrap();

        assert_eq!(connected.spend_events[0].timestamp_delta_seconds, -100);
        assert_eq!(connected.spend_events[0].age_blocks, 1);
    }
}
