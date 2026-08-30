use bitcoin::{Block, BlockHash};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("Bitcoin Core RPC error: {0}")]
    Rpc(#[from] bitcoincore_rpc::Error),

    #[error("Bitcoin Core reported height {0}, which exceeds the supported u32 range")]
    HeightOverflow(u64),

    #[error("block source backend error: {0}")]
    Backend(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeStatus {
    pub network: bitcoin::Network,
    pub blocks: u32,
    pub headers: u32,
    pub initial_block_download: bool,
    pub pruned: bool,
    pub prune_height: Option<u32>,
}

pub trait BlockSource {
    fn tip_height(&self) -> Result<u32, SourceError>;
    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError>;
    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError>;
}

#[derive(Debug)]
pub struct BitcoinCoreRpcSource {
    client: Client,
}

impl BitcoinCoreRpcSource {
    pub fn new(url: &str, auth: Auth) -> Result<Self, SourceError> {
        Ok(Self {
            client: Client::new(url, auth)?,
        })
    }

    pub fn cookie(url: &str, cookie_file: impl Into<PathBuf>) -> Result<Self, SourceError> {
        Self::new(url, Auth::CookieFile(cookie_file.into()))
    }

    pub fn user_pass(
        url: &str,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, SourceError> {
        Self::new(url, Auth::UserPass(username.into(), password.into()))
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn node_status(&self) -> Result<NodeStatus, SourceError> {
        let info = self.client.get_blockchain_info()?;
        Ok(NodeStatus {
            network: info.chain,
            blocks: height_to_u32(info.blocks)?,
            headers: height_to_u32(info.headers)?,
            initial_block_download: info.initial_block_download,
            pruned: info.pruned,
            prune_height: info.prune_height.map(height_to_u32).transpose()?,
        })
    }
}

fn height_to_u32(height: u64) -> Result<u32, SourceError> {
    u32::try_from(height).map_err(|_| SourceError::HeightOverflow(height))
}

impl BlockSource for BitcoinCoreRpcSource {
    fn tip_height(&self) -> Result<u32, SourceError> {
        height_to_u32(self.client.get_block_count()?)
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        Ok(self.client.get_block_hash(u64::from(height))?)
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        Ok(self.client.get_block(hash)?)
    }
}
