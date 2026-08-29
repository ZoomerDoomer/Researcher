use bitcoin::{Block, BlockHash, Network};
use researcher_bitcoin_source::{BlockSource, SourceError};
use researcher_indexer_core::{BlockUndo, ConnectPolicy, ReplacementEvent, SpendEvent, UtxoState};
use std::collections::VecDeque;
use std::str::FromStr;
use thiserror::Error;

const BIP30_REPEAT_91842: &str = "00000000000a4d0a398161ffc163c503763b1f4360639393e0e4c8e300e0caec";
const BIP30_REPEAT_91880: &str = "00000000000743f190a18c5577a3c2d2a1f610ae9601ac046a38084ccb7cd721";
const BIP30_ORIGINAL_91722: &str =
    "00000000000271a2dc26e7667f8419f2e15416dc6955e5a6c6cdf3f2574dd08e";
const BIP30_ORIGINAL_91812: &str =
    "00000000000af0aed4792b1acee3d966af36cf5def14935db8de83d6f9306f2f";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainTip {
    pub height: u32,
    pub hash: BlockHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedEvents {
    pub height: u32,
    pub hash: BlockHash,
    pub spend_events: Vec<SpendEvent>,
    pub replacement_events: Vec<ReplacementEvent>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub connected: u32,
    pub disconnected: u32,
}

#[derive(Debug, Error)]
pub enum ChainError {
    #[error(transparent)]
    Source(#[from] SourceError),

    #[error(transparent)]
    Apply(#[from] researcher_indexer_core::ApplyError),

    #[error("an empty indexer can only start at genesis height 0, got {0}")]
    MustStartAtGenesis(u32),

    #[error("unexpected genesis block {actual}; expected {expected} for configured network")]
    UnexpectedGenesis {
        expected: BlockHash,
        actual: BlockHash,
    },

    #[error("expected height {expected}, got {actual}")]
    HeightDiscontinuity { expected: u32, actual: u32 },

    #[error("block {hash} points to {actual_prev}, expected previous hash {expected_prev}")]
    PreviousHashMismatch {
        hash: BlockHash,
        expected_prev: BlockHash,
        actual_prev: BlockHash,
    },

    #[error("source returned block {actual} for requested hash {expected}")]
    SourceBlockHashMismatch {
        expected: BlockHash,
        actual: BlockHash,
    },

    #[error("cannot disconnect local tip because its undo data is no longer retained")]
    ReorgBeyondUndo,
}

#[derive(Debug)]
struct AppliedBlock {
    tip: ChainTip,
    previous_tip: Option<ChainTip>,
    undo: BlockUndo,
}

#[derive(Debug)]
pub struct ChainIndexer {
    network: Network,
    state: UtxoState,
    tip: Option<ChainTip>,
    undo_history: VecDeque<AppliedBlock>,
    max_undo_blocks: usize,
}

impl ChainIndexer {
    pub fn new(network: Network, max_undo_blocks: usize) -> Self {
        Self {
            network,
            state: UtxoState::default(),
            tip: None,
            undo_history: VecDeque::new(),
            max_undo_blocks,
        }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn tip(&self) -> Option<ChainTip> {
        self.tip
    }

    pub fn utxo_state(&self) -> &UtxoState {
        &self.state
    }

    pub fn connect_block(
        &mut self,
        height: u32,
        block: &Block,
    ) -> Result<AppliedEvents, ChainError> {
        let hash = block.block_hash();

        match self.tip {
            None => {
                if height != 0 {
                    return Err(ChainError::MustStartAtGenesis(height));
                }
                let expected =
                    bitcoin::blockdata::constants::genesis_block(self.network).block_hash();
                if hash != expected {
                    return Err(ChainError::UnexpectedGenesis {
                        expected,
                        actual: hash,
                    });
                }
            }
            Some(tip) => {
                let expected = tip
                    .height
                    .checked_add(1)
                    .expect("Bitcoin height cannot overflow u32 in practice");
                if height != expected {
                    return Err(ChainError::HeightDiscontinuity {
                        expected,
                        actual: height,
                    });
                }
                if block.header.prev_blockhash != tip.hash {
                    return Err(ChainError::PreviousHashMismatch {
                        hash,
                        expected_prev: tip.hash,
                        actual_prev: block.header.prev_blockhash,
                    });
                }
            }
        }

        let policy = connect_policy(self.network, height, hash);
        let connected = self.state.connect_block_with_policy(
            height,
            block.header.time,
            &block.txdata,
            policy,
        )?;

        let previous_tip = self.tip;
        let tip = ChainTip { height, hash };
        self.tip = Some(tip);
        self.undo_history.push_back(AppliedBlock {
            tip,
            previous_tip,
            undo: connected.undo,
        });
        while self.undo_history.len() > self.max_undo_blocks {
            self.undo_history.pop_front();
        }

        Ok(AppliedEvents {
            height,
            hash,
            spend_events: connected.spend_events,
            replacement_events: connected.replacement_events,
        })
    }

    pub fn disconnect_tip(&mut self) -> Result<ChainTip, ChainError> {
        let Some(current_tip) = self.tip else {
            return Err(ChainError::ReorgBeyondUndo);
        };
        let Some(applied) = self.undo_history.pop_back() else {
            return Err(ChainError::ReorgBeyondUndo);
        };
        if applied.tip != current_tip {
            return Err(ChainError::ReorgBeyondUndo);
        }

        self.state.disconnect_block(applied.undo);
        self.tip = applied.previous_tip;
        Ok(current_tip)
    }

    pub fn sync_to_tip<S: BlockSource>(&mut self, source: &S) -> Result<SyncStats, ChainError> {
        let target_height = source.tip_height()?;
        let disconnect_count = self.required_disconnects(source, target_height)?;

        let mut stats = SyncStats::default();
        for _ in 0..disconnect_count {
            self.disconnect_tip()?;
            stats.disconnected += 1;
        }

        let next_height = self.tip.map_or(0, |tip| tip.height + 1);
        if next_height > target_height {
            return Ok(stats);
        }

        for height in next_height..=target_height {
            let expected_hash = source.block_hash(height)?;
            let block = source.block(&expected_hash)?;
            let actual_hash = block.block_hash();
            if actual_hash != expected_hash {
                return Err(ChainError::SourceBlockHashMismatch {
                    expected: expected_hash,
                    actual: actual_hash,
                });
            }
            self.connect_block(height, &block)?;
            stats.connected += 1;
        }

        Ok(stats)
    }

    fn required_disconnects<S: BlockSource>(
        &self,
        source: &S,
        target_height: u32,
    ) -> Result<usize, ChainError> {
        let mut candidate = self.tip;
        let mut depth = 0usize;

        while let Some(local_tip) = candidate {
            let matches_source = if local_tip.height <= target_height {
                source.block_hash(local_tip.height)? == local_tip.hash
            } else {
                false
            };

            if matches_source {
                return Ok(depth);
            }

            let Some(applied) = self.undo_history.get(
                self.undo_history
                    .len()
                    .checked_sub(depth + 1)
                    .ok_or(ChainError::ReorgBeyondUndo)?,
            ) else {
                return Err(ChainError::ReorgBeyondUndo);
            };

            if applied.tip != local_tip {
                return Err(ChainError::ReorgBeyondUndo);
            }

            candidate = applied.previous_tip;
            depth += 1;
        }

        Ok(depth)
    }
}

pub fn is_bip30_repeat(network: Network, height: u32, hash: BlockHash) -> bool {
    if network != Network::Bitcoin {
        return false;
    }
    match height {
        91_842 => hash == parse_hash(BIP30_REPEAT_91842),
        91_880 => hash == parse_hash(BIP30_REPEAT_91880),
        _ => false,
    }
}

pub fn is_bip30_original(network: Network, height: u32, hash: BlockHash) -> bool {
    if network != Network::Bitcoin {
        return false;
    }
    match height {
        91_722 => hash == parse_hash(BIP30_ORIGINAL_91722),
        91_812 => hash == parse_hash(BIP30_ORIGINAL_91812),
        _ => false,
    }
}

pub fn connect_policy(network: Network, height: u32, hash: BlockHash) -> ConnectPolicy {
    ConnectPolicy {
        allow_unspent_overwrite: is_bip30_repeat(network, height, hash),
        skip_output_creation: height == 0,
    }
}

fn parse_hash(value: &str) -> BlockHash {
    BlockHash::from_str(value).expect("hard-coded Bitcoin block hash must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::blockdata::constants::genesis_block;
    use bitcoin::ScriptBuf;
    use researcher_bitcoin_source::BlockSource;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemorySource {
        blocks: BTreeMap<u32, Block>,
    }

    impl MemorySource {
        fn from_blocks(blocks: impl IntoIterator<Item = (u32, Block)>) -> Self {
            Self {
                blocks: blocks.into_iter().collect(),
            }
        }
    }

    impl BlockSource for MemorySource {
        fn tip_height(&self) -> Result<u32, SourceError> {
            self.blocks
                .last_key_value()
                .map(|(height, _)| *height)
                .ok_or_else(|| SourceError::Backend("empty source".to_owned()))
        }

        fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
            self.blocks
                .get(&height)
                .map(Block::block_hash)
                .ok_or_else(|| SourceError::Backend(format!("missing height {height}")))
        }

        fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
            self.blocks
                .values()
                .find(|block| block.block_hash() == *hash)
                .cloned()
                .ok_or_else(|| SourceError::Backend(format!("missing block {hash}")))
        }
    }

    fn child_of(parent: &Block, nonce_delta: u32) -> Block {
        let mut child = genesis_block(Network::Bitcoin);
        child.header.prev_blockhash = parent.block_hash();
        child.header.time = parent.header.time.saturating_add(1);
        child.header.nonce = child.header.nonce.wrapping_add(nonce_delta);
        child.txdata[0].input[0].script_sig =
            ScriptBuf::from_bytes(nonce_delta.to_le_bytes().to_vec());
        child
    }

    #[test]
    fn genesis_outputs_are_not_added_to_state() {
        let genesis = genesis_block(Network::Bitcoin);
        let mut indexer = ChainIndexer::new(Network::Bitcoin, 10);

        indexer.connect_block(0, &genesis).unwrap();

        assert_eq!(indexer.tip().unwrap().height, 0);
        assert!(indexer.utxo_state().is_empty());
    }

    #[test]
    fn wrong_network_genesis_is_rejected_without_mutating_state() {
        let wrong_genesis = genesis_block(Network::Testnet);
        let mut indexer = ChainIndexer::new(Network::Bitcoin, 10);

        let err = indexer.connect_block(0, &wrong_genesis).unwrap_err();

        assert!(matches!(err, ChainError::UnexpectedGenesis { .. }));
        assert!(indexer.tip().is_none());
        assert!(indexer.utxo_state().is_empty());
    }

    #[test]
    fn continuity_rejects_wrong_previous_hash() {
        let genesis = genesis_block(Network::Bitcoin);
        let unrelated = genesis_block(Network::Testnet);
        let mut indexer = ChainIndexer::new(Network::Bitcoin, 10);

        indexer.connect_block(0, &genesis).unwrap();
        let err = indexer.connect_block(1, &unrelated).unwrap_err();

        assert!(matches!(err, ChainError::PreviousHashMismatch { .. }));
    }

    #[test]
    fn sync_reconciles_one_block_reorg() {
        let genesis = genesis_block(Network::Bitcoin);
        let first = child_of(&genesis, 1);
        let replacement = child_of(&genesis, 2);

        let source_a = MemorySource::from_blocks([(0, genesis.clone()), (1, first)]);
        let source_b = MemorySource::from_blocks([(0, genesis), (1, replacement.clone())]);
        let mut indexer = ChainIndexer::new(Network::Bitcoin, 10);

        assert_eq!(
            indexer.sync_to_tip(&source_a).unwrap(),
            SyncStats {
                connected: 2,
                disconnected: 0,
            }
        );
        assert_eq!(
            indexer.sync_to_tip(&source_b).unwrap(),
            SyncStats {
                connected: 1,
                disconnected: 1,
            }
        );
        assert_eq!(indexer.tip().unwrap().hash, replacement.block_hash());
    }

    #[test]
    fn deep_reorg_fails_before_mutating_local_tip() {
        let genesis = genesis_block(Network::Bitcoin);
        let block1 = child_of(&genesis, 1);
        let block2 = child_of(&block1, 2);

        let long_source =
            MemorySource::from_blocks([(0, genesis.clone()), (1, block1), (2, block2.clone())]);
        let short_source = MemorySource::from_blocks([(0, genesis)]);
        let mut indexer = ChainIndexer::new(Network::Bitcoin, 1);

        indexer.sync_to_tip(&long_source).unwrap();
        let before = indexer.tip();
        let err = indexer.sync_to_tip(&short_source).unwrap_err();

        assert!(matches!(err, ChainError::ReorgBeyondUndo));
        assert_eq!(indexer.tip(), before);
        assert_eq!(indexer.tip().unwrap().hash, block2.block_hash());
    }

    #[test]
    fn bip30_policy_is_exact_by_network_height_and_hash() {
        let repeat_91842 = parse_hash(BIP30_REPEAT_91842);
        let original_91722 = parse_hash(BIP30_ORIGINAL_91722);

        assert!(is_bip30_repeat(Network::Bitcoin, 91_842, repeat_91842));
        assert!(!is_bip30_repeat(Network::Bitcoin, 91_843, repeat_91842));
        assert!(!is_bip30_repeat(Network::Testnet, 91_842, repeat_91842));

        assert!(is_bip30_original(Network::Bitcoin, 91_722, original_91722));
        assert!(!is_bip30_original(Network::Bitcoin, 91_723, original_91722));
    }
}
