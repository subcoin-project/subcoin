use bitcoin::consensus::Decodable;
use bitcoin::{Block, BlockHash};
use reqwest::Client;
use thiserror::Error;

const BLOCKSTREAM_API_URL: &str = "https://blockstream.info/api";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Invalid block hash")]
    InvalidBlockHash,
    #[error("Invalid block number")]
    InvalidBlockNumber,
    #[error("HTTP request failed: {0}")]
    HttpRequestError(#[from] reqwest::Error),
    #[error(transparent)]
    BitcoinIO(#[from] bitcoin::io::Error),
    #[error(transparent)]
    BitcoinEncode(#[from] bitcoin::consensus::encode::Error),
}

/// Client for interacting with the Blockstream API.
pub struct BlockstreamClient {
    client: Client,
}

impl BlockstreamClient {
    /// Create a new instance of [`BlockstreamClient`].
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Fetch the height of the latest block.
    pub async fn get_tip_height(&self) -> Result<u32, ApiError> {
        let url = format!("{BLOCKSTREAM_API_URL}/blocks/tip/height");
        let response = self.client.get(&url).send().await?;
        let height = response.text().await?;
        height.parse().map_err(|_| ApiError::InvalidBlockNumber)
    }

    /// Fetch the hash of the latest block (tip hash).
    #[allow(dead_code)]
    pub async fn get_tip_hash(&self) -> Result<BlockHash, ApiError> {
        let url = format!("{BLOCKSTREAM_API_URL}/blocks/tip/hash");
        let response = self.client.get(&url).send().await?;
        let hash = response.text().await?;
        hash.parse().map_err(|_| ApiError::InvalidBlockHash)
    }

    /// Fetch hash of the block specified by the height.
    pub async fn get_block_hash(&self, height: u32) -> Result<BlockHash, ApiError> {
        let url = format!("{BLOCKSTREAM_API_URL}/block-height/{height}");
        let hash = self.client.get(&url).send().await?.text().await?;
        hash.parse().map_err(|_| ApiError::InvalidBlockHash)
    }

    /// Fetch block by its height.
    pub async fn get_block_by_height(&self, height: u32) -> Result<Block, ApiError> {
        let hash = self.get_block_hash(height).await?;
        self.get_block(hash).await
    }

    /// Fetch block by its hash.
    pub async fn get_block(&self, hash: BlockHash) -> Result<Block, ApiError> {
        let url = format!("{BLOCKSTREAM_API_URL}/block/{hash}/raw");
        let raw_block = self.client.get(&url).send().await?.bytes().await?;
        let block = bitcoin::Block::consensus_decode(&mut raw_block.as_ref())?;
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_blockstream_client() {
        let client = BlockstreamClient::new();
        assert_eq!(
            client.get_block_hash(100).await.unwrap(),
            "000000007bc154e0fa7ea32218a72fe2c1bb9f86cf8c9ebf9a715ed27fdb229a"
                .parse()
                .unwrap()
        );
    }
}
