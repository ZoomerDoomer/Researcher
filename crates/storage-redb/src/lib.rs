use bitcoin::hashes::Hash;
use bitcoin::{Block, BlockHash, Network, OutPoint, Txid};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use researcher_bitcoin_source::{BlockSource, SourceError};
use researcher_chain_indexer::connect_policy;
use researcher_indexer_core::{
    is_utxo_candidate, ReplacementEvent, SpendEvent, UtxoEntry, UtxoState,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const UTXOS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("utxos");
const BLOCK_EVENTS: TableDefinition<u32, &[u8]> = TableDefinition::new("block_events");
const META_SCHEMA: &str = "schema_version";
const META_NETWORK: &str = "network_genesis";
const META_TIP: &str = "tip";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableTip {
    pub height: u32,
    pub hash: BlockHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEventBundle {
    pub schema_version: u32,
    pub height: u32,
    pub hash: BlockHash,
    pub prev_hash: BlockHash,
    pub created_outpoints: Vec<OutPoint>,
    pub spend_events: Vec<SpendEvent>,
    pub replacement_events: Vec<ReplacementEvent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredTip {
    height: u32,
    hash: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredOutPoint {
    txid: [u8; 32],
    vout: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredUtxoEntry {
    value_sat: u64,
    created_height: u32,
    created_timestamp: u32,
    is_coinbase: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredSpendEvent {
    outpoint: StoredOutPoint,
    spending_txid: [u8; 32],
    value_sat: u64,
    created_height: u32,
    spent_height: u32,
    created_timestamp: u32,
    spent_timestamp: u32,
    is_coinbase: bool,
    age_blocks: u32,
    timestamp_delta_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredReplacementEvent {
    outpoint: StoredOutPoint,
    replaced: StoredUtxoEntry,
    replacement: StoredUtxoEntry,
    replacement_height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredBlockEventBundle {
    schema_version: u32,
    height: u32,
    hash: [u8; 32],
    prev_hash: [u8; 32],
    created_outpoints: Vec<StoredOutPoint>,
    spend_events: Vec<StoredSpendEvent>,
    replacement_events: Vec<StoredReplacementEvent>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DurableSyncStats {
    pub connected: u32,
    pub disconnected: u32,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("codec error: {0}")]
    Codec(String),

    #[error(transparent)]
    Source(#[from] SourceError),

    #[error(transparent)]
    Apply(#[from] researcher_indexer_core::ApplyError),

    #[error("database schema {actual} is incompatible with expected schema {expected}")]
    SchemaMismatch { expected: u32, actual: u32 },

    #[error("database belongs to a different Bitcoin network")]
    NetworkMismatch,

    #[error("an empty database can only start at genesis height 0, got {0}")]
    MustStartAtGenesis(u32),

    #[error("unexpected genesis block {actual}; expected {expected}")]
    UnexpectedGenesis {
        expected: BlockHash,
        actual: BlockHash,
    },

    #[error("expected next height {expected}, got {actual}")]
    HeightDiscontinuity { expected: u32, actual: u32 },

    #[error("block {hash} points to {actual_prev}, expected {expected_prev}")]
    PreviousHashMismatch {
        hash: BlockHash,
        expected_prev: BlockHash,
        actual_prev: BlockHash,
    },

    #[error("block-event row already exists at height {0}")]
    EventAlreadyExists(u32),

    #[error("missing persisted block-event row at height {0}")]
    MissingBlockEvents(u32),

    #[error("source returned block {actual} for requested hash {expected}")]
    SourceBlockHashMismatch {
        expected: BlockHash,
        actual: BlockHash,
    },

    #[error("configured source does not share the configured network genesis")]
    SourceNetworkMismatch,

    #[error("requested target height {requested} exceeds source tip {source_tip}")]
    TargetAboveSourceTip { requested: u32, source_tip: u32 },
}

pub struct DurableStore {
    db: Database,
    network: Network,
}

impl DurableStore {
    pub fn open(path: impl AsRef<Path>, network: Network) -> Result<Self, StoreError> {
        let db = Database::create(path).map_err(storage_error)?;
        let store = Self { db, network };
        store.initialize_metadata()?;
        Ok(store)
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn tip(&self) -> Result<Option<DurableTip>, StoreError> {
        let read = self.db.begin_read().map_err(storage_error)?;
        let table = read.open_table(META).map_err(storage_error)?;
        let Some(value) = table.get(META_TIP).map_err(storage_error)? else {
            return Ok(None);
        };
        decode_tip(value.value()).map(Some)
    }

    pub fn utxo(&self, outpoint: &OutPoint) -> Result<Option<UtxoEntry>, StoreError> {
        let key = outpoint_key(outpoint);
        let read = self.db.begin_read().map_err(storage_error)?;
        let table = read.open_table(UTXOS).map_err(storage_error)?;
        let Some(value) = table.get(key.as_slice()).map_err(storage_error)? else {
            return Ok(None);
        };
        decode_utxo(value.value()).map(Some)
    }

    pub fn block_events(&self, height: u32) -> Result<Option<BlockEventBundle>, StoreError> {
        let read = self.db.begin_read().map_err(storage_error)?;
        let table = read.open_table(BLOCK_EVENTS).map_err(storage_error)?;
        let Some(value) = table.get(&height).map_err(storage_error)? else {
            return Ok(None);
        };
        decode_bundle(value.value()).map(Some)
    }

    pub fn connect_block(
        &self,
        height: u32,
        block: &Block,
    ) -> Result<BlockEventBundle, StoreError> {
        let hash = block.block_hash();
        let current_tip = self.tip()?;
        self.validate_connect(height, hash, block.header.prev_blockhash, current_tip)?;

        let policy = connect_policy(self.network, height, hash);
        let created_outpoints = created_outpoints(block, policy.skip_output_creation);
        let preload = preload_outpoints(block, &created_outpoints);

        let mut write = self.db.begin_write().map_err(storage_error)?;
        write.set_quick_repair(true);

        let mut state = UtxoState::default();
        {
            let table = write.open_table(UTXOS).map_err(storage_error)?;
            for outpoint in preload {
                let key = outpoint_key(&outpoint);
                if let Some(value) = table.get(key.as_slice()).map_err(storage_error)? {
                    state.seed_entry(outpoint, decode_utxo(value.value())?)?;
                }
            }
        }

        let connected =
            state.connect_block_with_policy(height, block.header.time, &block.txdata, policy)?;

        let bundle = BlockEventBundle {
            schema_version: SCHEMA_VERSION,
            height,
            hash,
            prev_hash: block.header.prev_blockhash,
            created_outpoints,
            spend_events: connected.spend_events,
            replacement_events: connected.replacement_events,
        };

        {
            let mut utxos = write.open_table(UTXOS).map_err(storage_error)?;

            for event in &bundle.spend_events {
                if event.created_height < height {
                    let key = outpoint_key(&event.outpoint);
                    utxos.remove(key.as_slice()).map_err(storage_error)?;
                }
            }

            for outpoint in &bundle.created_outpoints {
                let key = outpoint_key(outpoint);
                match state.get(outpoint) {
                    Some(entry) => {
                        let bytes = encode_utxo(entry)?;
                        utxos
                            .insert(key.as_slice(), bytes.as_slice())
                            .map_err(storage_error)?;
                    }
                    None => {
                        utxos.remove(key.as_slice()).map_err(storage_error)?;
                    }
                }
            }
        }

        {
            let mut events = write.open_table(BLOCK_EVENTS).map_err(storage_error)?;
            if events.get(&height).map_err(storage_error)?.is_some() {
                return Err(StoreError::EventAlreadyExists(height));
            }
            let bytes = encode_bundle(&bundle)?;
            events
                .insert(&height, bytes.as_slice())
                .map_err(storage_error)?;
        }

        {
            let mut meta = write.open_table(META).map_err(storage_error)?;
            let tip = DurableTip { height, hash };
            let bytes = encode_tip(&tip)?;
            meta.insert(META_TIP, bytes.as_slice())
                .map_err(storage_error)?;
        }

        write.commit().map_err(storage_error)?;
        Ok(bundle)
    }

    pub fn disconnect_tip(&self) -> Result<Option<DurableTip>, StoreError> {
        let Some(tip) = self.tip()? else {
            return Ok(None);
        };

        let mut write = self.db.begin_write().map_err(storage_error)?;
        write.set_quick_repair(true);

        let bundle = {
            let events = write.open_table(BLOCK_EVENTS).map_err(storage_error)?;
            let Some(value) = events.get(&tip.height).map_err(storage_error)? else {
                return Err(StoreError::MissingBlockEvents(tip.height));
            };
            let bundle = decode_bundle(value.value())?;
            if bundle.hash != tip.hash {
                return Err(StoreError::MissingBlockEvents(tip.height));
            }
            bundle
        };

        {
            let mut utxos = write.open_table(UTXOS).map_err(storage_error)?;

            for outpoint in &bundle.created_outpoints {
                let key = outpoint_key(outpoint);
                utxos.remove(key.as_slice()).map_err(storage_error)?;
            }

            for event in &bundle.spend_events {
                if event.created_height < bundle.height {
                    let entry = UtxoEntry {
                        value_sat: event.value_sat,
                        created_height: event.created_height,
                        created_timestamp: event.created_timestamp,
                        is_coinbase: event.is_coinbase,
                    };
                    let key = outpoint_key(&event.outpoint);
                    let bytes = encode_utxo(&entry)?;
                    utxos
                        .insert(key.as_slice(), bytes.as_slice())
                        .map_err(storage_error)?;
                }
            }

            for replacement in &bundle.replacement_events {
                let key = outpoint_key(&replacement.outpoint);
                let bytes = encode_utxo(&replacement.replaced)?;
                utxos
                    .insert(key.as_slice(), bytes.as_slice())
                    .map_err(storage_error)?;
            }
        }

        {
            let mut events = write.open_table(BLOCK_EVENTS).map_err(storage_error)?;
            events.remove(&tip.height).map_err(storage_error)?;
        }

        let previous_tip = if tip.height == 0 {
            None
        } else {
            Some(DurableTip {
                height: tip.height - 1,
                hash: bundle.prev_hash,
            })
        };

        {
            let mut meta = write.open_table(META).map_err(storage_error)?;
            match previous_tip {
                Some(previous) => {
                    let bytes = encode_tip(&previous)?;
                    meta.insert(META_TIP, bytes.as_slice())
                        .map_err(storage_error)?;
                }
                None => {
                    meta.remove(META_TIP).map_err(storage_error)?;
                }
            }
        }

        write.commit().map_err(storage_error)?;
        Ok(previous_tip)
    }

    pub fn sync_to_tip<S: BlockSource>(&self, source: &S) -> Result<DurableSyncStats, StoreError> {
        self.verify_source_network(source)?;
        let target_height = source.tip_height()?;
        self.sync_to_target(source, target_height)
    }

    pub fn sync_to_height<S: BlockSource>(
        &self,
        source: &S,
        target_height: u32,
    ) -> Result<DurableSyncStats, StoreError> {
        self.verify_source_network(source)?;
        let source_tip = source.tip_height()?;
        if target_height > source_tip {
            return Err(StoreError::TargetAboveSourceTip {
                requested: target_height,
                source_tip,
            });
        }
        self.sync_to_target(source, target_height)
    }

    fn sync_to_target<S: BlockSource>(
        &self,
        source: &S,
        target_height: u32,
    ) -> Result<DurableSyncStats, StoreError> {
        let disconnect_count = self.required_disconnects(source, target_height)?;

        let mut stats = DurableSyncStats::default();
        for _ in 0..disconnect_count {
            self.disconnect_tip()?;
            stats.disconnected += 1;
        }

        let next_height = self.tip()?.map_or(0, |tip| tip.height + 1);
        if next_height > target_height {
            return Ok(stats);
        }

        for height in next_height..=target_height {
            let expected_hash = source.block_hash(height)?;
            let block = source.block(&expected_hash)?;
            let actual_hash = block.block_hash();
            if actual_hash != expected_hash {
                return Err(StoreError::SourceBlockHashMismatch {
                    expected: expected_hash,
                    actual: actual_hash,
                });
            }
            self.connect_block(height, &block)?;
            stats.connected += 1;
        }

        Ok(stats)
    }

    fn initialize_metadata(&self) -> Result<(), StoreError> {
        let mut write = self.db.begin_write().map_err(storage_error)?;
        write.set_quick_repair(true);
        {
            let mut meta = write.open_table(META).map_err(storage_error)?;

            let schema_bytes = SCHEMA_VERSION.to_le_bytes();
            let existing_schema = meta
                .get(META_SCHEMA)
                .map_err(storage_error)?
                .map(|value| value.value().to_vec());
            if let Some(bytes) = existing_schema {
                if bytes.len() != 4 {
                    return Err(StoreError::Storage(
                        "invalid schema-version metadata".to_owned(),
                    ));
                }
                let actual =
                    u32::from_le_bytes(bytes.as_slice().try_into().expect("length checked"));
                if actual != SCHEMA_VERSION {
                    return Err(StoreError::SchemaMismatch {
                        expected: SCHEMA_VERSION,
                        actual,
                    });
                }
            } else {
                meta.insert(META_SCHEMA, schema_bytes.as_slice())
                    .map_err(storage_error)?;
            }

            let genesis = bitcoin::blockdata::constants::genesis_block(self.network).block_hash();
            let network_bytes = genesis.to_byte_array();
            let existing_network = meta
                .get(META_NETWORK)
                .map_err(storage_error)?
                .map(|value| value.value().to_vec());
            if let Some(bytes) = existing_network {
                if bytes.as_slice() != network_bytes.as_slice() {
                    return Err(StoreError::NetworkMismatch);
                }
            } else {
                meta.insert(META_NETWORK, network_bytes.as_slice())
                    .map_err(storage_error)?;
            }
        }
        write.commit().map_err(storage_error)?;
        Ok(())
    }

    fn validate_connect(
        &self,
        height: u32,
        hash: BlockHash,
        prev_hash: BlockHash,
        tip: Option<DurableTip>,
    ) -> Result<(), StoreError> {
        match tip {
            None => {
                if height != 0 {
                    return Err(StoreError::MustStartAtGenesis(height));
                }
                let expected =
                    bitcoin::blockdata::constants::genesis_block(self.network).block_hash();
                if hash != expected {
                    return Err(StoreError::UnexpectedGenesis {
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
                    return Err(StoreError::HeightDiscontinuity {
                        expected,
                        actual: height,
                    });
                }
                if prev_hash != tip.hash {
                    return Err(StoreError::PreviousHashMismatch {
                        hash,
                        expected_prev: tip.hash,
                        actual_prev: prev_hash,
                    });
                }
            }
        }
        Ok(())
    }

    fn verify_source_network<S: BlockSource>(&self, source: &S) -> Result<(), StoreError> {
        let expected = bitcoin::blockdata::constants::genesis_block(self.network).block_hash();
        if source.block_hash(0)? != expected {
            return Err(StoreError::SourceNetworkMismatch);
        }
        Ok(())
    }

    fn required_disconnects<S: BlockSource>(
        &self,
        source: &S,
        target_height: u32,
    ) -> Result<u32, StoreError> {
        let Some(mut candidate) = self.tip()? else {
            return Ok(0);
        };
        let mut depth = 0u32;

        loop {
            let local_tip = candidate;
            if local_tip.height <= target_height
                && source.block_hash(local_tip.height)? == local_tip.hash
            {
                return Ok(depth);
            }

            let Some(bundle) = self.block_events(local_tip.height)? else {
                return Err(StoreError::MissingBlockEvents(local_tip.height));
            };

            if local_tip.height == 0 {
                return Err(StoreError::SourceNetworkMismatch);
            }

            candidate = DurableTip {
                height: local_tip.height - 1,
                hash: bundle.prev_hash,
            };
            depth += 1;
        }
    }
}

fn preload_outpoints(block: &Block, created: &[OutPoint]) -> HashSet<OutPoint> {
    let mut outpoints: HashSet<OutPoint> = created.iter().copied().collect();
    for tx in &block.txdata {
        if tx.is_coinbase() {
            continue;
        }
        for input in &tx.input {
            outpoints.insert(input.previous_output);
        }
    }
    outpoints
}

fn created_outpoints(block: &Block, skip_output_creation: bool) -> Vec<OutPoint> {
    if skip_output_creation {
        return Vec::new();
    }

    let mut outpoints = Vec::new();
    for tx in &block.txdata {
        let txid = tx.compute_txid();
        for (vout, output) in tx.output.iter().enumerate() {
            if !is_utxo_candidate(&output.script_pubkey) {
                continue;
            }
            outpoints.push(OutPoint::new(
                txid,
                u32::try_from(vout).expect("transaction output count fits in u32"),
            ));
        }
    }
    outpoints
}

fn outpoint_key(outpoint: &OutPoint) -> [u8; 36] {
    let mut key = [0u8; 36];
    key[..32].copy_from_slice(&outpoint.txid.to_byte_array());
    key[32..].copy_from_slice(&outpoint.vout.to_le_bytes());
    key
}

fn encode_tip(value: &DurableTip) -> Result<Vec<u8>, StoreError> {
    encode_wire(&StoredTip {
        height: value.height,
        hash: value.hash.to_byte_array(),
    })
}

fn decode_tip(bytes: &[u8]) -> Result<DurableTip, StoreError> {
    let stored: StoredTip = decode_wire(bytes)?;
    Ok(DurableTip {
        height: stored.height,
        hash: BlockHash::from_byte_array(stored.hash),
    })
}

fn encode_utxo(value: &UtxoEntry) -> Result<Vec<u8>, StoreError> {
    encode_wire(&stored_utxo(value))
}

fn decode_utxo(bytes: &[u8]) -> Result<UtxoEntry, StoreError> {
    let stored: StoredUtxoEntry = decode_wire(bytes)?;
    Ok(utxo_from_stored(stored))
}

fn encode_bundle(value: &BlockEventBundle) -> Result<Vec<u8>, StoreError> {
    let stored = StoredBlockEventBundle {
        schema_version: value.schema_version,
        height: value.height,
        hash: value.hash.to_byte_array(),
        prev_hash: value.prev_hash.to_byte_array(),
        created_outpoints: value
            .created_outpoints
            .iter()
            .map(stored_outpoint)
            .collect(),
        spend_events: value.spend_events.iter().map(stored_spend).collect(),
        replacement_events: value
            .replacement_events
            .iter()
            .map(stored_replacement)
            .collect(),
    };
    encode_wire(&stored)
}

fn decode_bundle(bytes: &[u8]) -> Result<BlockEventBundle, StoreError> {
    let stored: StoredBlockEventBundle = decode_wire(bytes)?;
    if stored.schema_version != SCHEMA_VERSION {
        return Err(StoreError::SchemaMismatch {
            expected: SCHEMA_VERSION,
            actual: stored.schema_version,
        });
    }
    Ok(BlockEventBundle {
        schema_version: stored.schema_version,
        height: stored.height,
        hash: BlockHash::from_byte_array(stored.hash),
        prev_hash: BlockHash::from_byte_array(stored.prev_hash),
        created_outpoints: stored
            .created_outpoints
            .into_iter()
            .map(outpoint_from_stored)
            .collect(),
        spend_events: stored
            .spend_events
            .into_iter()
            .map(spend_from_stored)
            .collect(),
        replacement_events: stored
            .replacement_events
            .into_iter()
            .map(replacement_from_stored)
            .collect(),
    })
}

fn stored_outpoint(value: &OutPoint) -> StoredOutPoint {
    StoredOutPoint {
        txid: value.txid.to_byte_array(),
        vout: value.vout,
    }
}

fn outpoint_from_stored(value: StoredOutPoint) -> OutPoint {
    OutPoint::new(Txid::from_byte_array(value.txid), value.vout)
}

fn stored_utxo(value: &UtxoEntry) -> StoredUtxoEntry {
    StoredUtxoEntry {
        value_sat: value.value_sat,
        created_height: value.created_height,
        created_timestamp: value.created_timestamp,
        is_coinbase: value.is_coinbase,
    }
}

fn utxo_from_stored(value: StoredUtxoEntry) -> UtxoEntry {
    UtxoEntry {
        value_sat: value.value_sat,
        created_height: value.created_height,
        created_timestamp: value.created_timestamp,
        is_coinbase: value.is_coinbase,
    }
}

fn stored_spend(value: &SpendEvent) -> StoredSpendEvent {
    StoredSpendEvent {
        outpoint: stored_outpoint(&value.outpoint),
        spending_txid: value.spending_txid.to_byte_array(),
        value_sat: value.value_sat,
        created_height: value.created_height,
        spent_height: value.spent_height,
        created_timestamp: value.created_timestamp,
        spent_timestamp: value.spent_timestamp,
        is_coinbase: value.is_coinbase,
        age_blocks: value.age_blocks,
        timestamp_delta_seconds: value.timestamp_delta_seconds,
    }
}

fn spend_from_stored(value: StoredSpendEvent) -> SpendEvent {
    SpendEvent {
        outpoint: outpoint_from_stored(value.outpoint),
        spending_txid: Txid::from_byte_array(value.spending_txid),
        value_sat: value.value_sat,
        created_height: value.created_height,
        spent_height: value.spent_height,
        created_timestamp: value.created_timestamp,
        spent_timestamp: value.spent_timestamp,
        is_coinbase: value.is_coinbase,
        age_blocks: value.age_blocks,
        timestamp_delta_seconds: value.timestamp_delta_seconds,
    }
}

fn stored_replacement(value: &ReplacementEvent) -> StoredReplacementEvent {
    StoredReplacementEvent {
        outpoint: stored_outpoint(&value.outpoint),
        replaced: stored_utxo(&value.replaced),
        replacement: stored_utxo(&value.replacement),
        replacement_height: value.replacement_height,
    }
}

fn replacement_from_stored(value: StoredReplacementEvent) -> ReplacementEvent {
    ReplacementEvent {
        outpoint: outpoint_from_stored(value.outpoint),
        replaced: utxo_from_stored(value.replaced),
        replacement: utxo_from_stored(value.replacement),
        replacement_height: value.replacement_height,
    }
}

fn encode_wire<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    bincode::serde::encode_to_vec(value, bincode::config::standard())
        .map_err(|error| StoreError::Codec(error.to_string()))
}

fn decode_wire<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StoreError> {
    let (value, consumed): (T, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .map_err(|error| StoreError::Codec(error.to_string()))?;
    if consumed != bytes.len() {
        return Err(StoreError::Codec(format!(
            "decoder consumed {consumed} of {} bytes",
            bytes.len()
        )));
    }
    Ok(value)
}

fn storage_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Storage(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::absolute::LockTime;
    use bitcoin::blockdata::constants::genesis_block;
    use bitcoin::transaction::Version;
    use bitcoin::{Amount, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
    use researcher_bitcoin_source::BlockSource;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

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

    fn store(temp: &TempDir) -> DurableStore {
        DurableStore::open(temp.path().join("research.redb"), Network::Bitcoin).unwrap()
    }

    fn child_of(parent: &Block, nonce: u32, transactions: Vec<Transaction>) -> Block {
        let mut child = genesis_block(Network::Bitcoin);
        child.header.prev_blockhash = parent.block_hash();
        child.header.time = parent.header.time.saturating_add(1);
        child.header.nonce = child.header.nonce.wrapping_add(nonce);
        child.txdata = transactions;
        child
    }

    fn coinbase(tag: u8, value_sat: u64) -> Transaction {
        Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![tag]),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(value_sat),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    fn spend(prevout: OutPoint, tag: u8, value_sat: u64) -> Transaction {
        Transaction {
            version: Version::ONE,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: prevout,
                script_sig: ScriptBuf::from_bytes(vec![tag]),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(value_sat),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[test]
    fn genesis_commit_is_durable_and_has_no_utxos() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        {
            let store = store(&temp);
            store.connect_block(0, &genesis).unwrap();
            assert_eq!(store.tip().unwrap().unwrap().height, 0);
        }

        let reopened = store(&temp);
        assert_eq!(reopened.tip().unwrap().unwrap().height, 0);
        assert!(reopened.block_events(0).unwrap().is_some());
    }

    #[test]
    fn spend_state_events_and_tip_survive_reopen_and_disconnect() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        let funding = coinbase(1, 5_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        let block1 = child_of(&genesis, 1, vec![funding]);
        let payment = spend(funding_outpoint, 2, 4_000);
        let payment_outpoint = OutPoint::new(payment.compute_txid(), 0);
        let block2 = child_of(&block1, 2, vec![coinbase(3, 5_000), payment]);

        {
            let store = store(&temp);
            store.connect_block(0, &genesis).unwrap();
            store.connect_block(1, &block1).unwrap();
            store.connect_block(2, &block2).unwrap();

            assert!(store.utxo(&funding_outpoint).unwrap().is_none());
            assert!(store.utxo(&payment_outpoint).unwrap().is_some());
            assert_eq!(
                store.block_events(2).unwrap().unwrap().spend_events.len(),
                1
            );
        }

        let reopened = store(&temp);
        assert_eq!(reopened.tip().unwrap().unwrap().height, 2);
        assert!(reopened.utxo(&payment_outpoint).unwrap().is_some());

        let previous = reopened.disconnect_tip().unwrap().unwrap();
        assert_eq!(previous.height, 1);
        assert!(reopened.utxo(&payment_outpoint).unwrap().is_none());
        assert!(reopened.utxo(&funding_outpoint).unwrap().is_some());
        assert!(reopened.block_events(2).unwrap().is_none());
    }

    #[test]
    fn same_block_spend_is_not_resurrected_on_disconnect() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        let funding = coinbase(1, 5_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        let block1 = child_of(&genesis, 1, vec![funding]);

        let first = spend(funding_outpoint, 2, 4_500);
        let first_outpoint = OutPoint::new(first.compute_txid(), 0);
        let second = spend(first_outpoint, 3, 4_000);
        let block2 = child_of(&block1, 2, vec![coinbase(4, 5_000), first, second]);

        let store = store(&temp);
        store.connect_block(0, &genesis).unwrap();
        store.connect_block(1, &block1).unwrap();
        store.connect_block(2, &block2).unwrap();
        store.disconnect_tip().unwrap();

        assert!(store.utxo(&first_outpoint).unwrap().is_none());
        assert!(store.utxo(&funding_outpoint).unwrap().is_some());
    }

    #[test]
    fn failed_block_does_not_advance_tip_or_mutate_state() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        let funding = coinbase(1, 5_000);
        let funding_outpoint = OutPoint::new(funding.compute_txid(), 0);
        let block1 = child_of(&genesis, 1, vec![funding]);
        let unknown = OutPoint::new(coinbase(9, 1).compute_txid(), 0);
        let invalid = child_of(
            &block1,
            2,
            vec![coinbase(2, 5_000), spend(unknown, 3, 1_000)],
        );

        let store = store(&temp);
        store.connect_block(0, &genesis).unwrap();
        store.connect_block(1, &block1).unwrap();
        let before = store.tip().unwrap();

        assert!(store.connect_block(2, &invalid).is_err());
        assert_eq!(store.tip().unwrap(), before);
        assert!(store.utxo(&funding_outpoint).unwrap().is_some());
        assert!(store.block_events(2).unwrap().is_none());
    }

    #[test]
    fn bounded_sync_stops_at_requested_height_and_rejects_unavailable_target() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        let block1 = child_of(&genesis, 1, vec![coinbase(1, 5_000)]);
        let block2 = child_of(&block1, 2, vec![coinbase(2, 5_000)]);
        let source = MemorySource::from_blocks([
            (0, genesis),
            (1, block1),
            (2, block2),
        ]);

        let store = store(&temp);
        let stats = store.sync_to_height(&source, 1).unwrap();

        assert_eq!(stats.connected, 2);
        assert_eq!(store.tip().unwrap().unwrap().height, 1);
        assert!(store.block_events(2).unwrap().is_none());

        let before = store.tip().unwrap();
        let err = store.sync_to_height(&source, 3).unwrap_err();
        assert!(matches!(
            err,
            StoreError::TargetAboveSourceTip {
                requested: 3,
                source_tip: 2
            }
        ));
        assert_eq!(store.tip().unwrap(), before);
    }

    #[test]
    fn durable_sync_reconciles_reorg_across_reopen() {
        let temp = TempDir::new().unwrap();
        let genesis = genesis_block(Network::Bitcoin);
        let block_a = child_of(&genesis, 1, vec![coinbase(1, 5_000)]);
        let block_b = child_of(&genesis, 2, vec![coinbase(2, 6_000)]);

        let source_a = MemorySource::from_blocks([(0, genesis.clone()), (1, block_a)]);
        let source_b = MemorySource::from_blocks([(0, genesis), (1, block_b.clone())]);

        {
            let store = store(&temp);
            let stats = store.sync_to_tip(&source_a).unwrap();
            assert_eq!(stats.connected, 2);
        }

        let reopened = store(&temp);
        let stats = reopened.sync_to_tip(&source_b).unwrap();
        assert_eq!(
            stats,
            DurableSyncStats {
                connected: 1,
                disconnected: 1,
            }
        );
        assert_eq!(reopened.tip().unwrap().unwrap().hash, block_b.block_hash());
    }
}
