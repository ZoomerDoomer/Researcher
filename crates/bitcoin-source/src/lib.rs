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
}

impl BlockSource for BitcoinCoreRpcSource {
    fn tip_height(&self) -> Result<u32, SourceError> {
        let height = self.client.get_block_count()?;
        u32::try_from(height).map_err(|_| SourceError::HeightOverflow(height))
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        Ok(self.client.get_block_hash(u64::from(height))?)
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        Ok(self.client.get_block(hash)?)
    }
}
