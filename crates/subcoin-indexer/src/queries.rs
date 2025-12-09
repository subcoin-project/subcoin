//! Query functions for the indexer.

use crate::db::{IndexerDatabase, Result};
use crate::types::{AddressBalance, AddressHistory, IndexerState, IndexerStatus, Utxo};
use bitcoin::Txid;
use subcoin_primitives::TxPosition;

/// Query interface for the indexer database.
#[derive(Clone)]
pub struct IndexerQuery {
    db: IndexerDatabase,
}

impl IndexerQuery {
    /// Create a new query interface.
    pub fn new(db: IndexerDatabase) -> Self {
        Self { db }
    }

    /// Get transaction position by txid.
    pub async fn get_tx_position(&self, txid: Txid) -> Result<Option<TxPosition>> {
        let result = self.db.get_transaction(&txid).await?;
        Ok(result.map(|(block_number, index)| TxPosition {
            block_number,
            index,
        }))
    }

    /// Get address transaction history with pagination.
    pub async fn get_address_history(
        &self,
        address: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<AddressHistory>> {
        let rows: Vec<(Vec<u8>, i64, i64)> = sqlx::query_as(
            r#"
            SELECT ah.txid, ah.block_height, ah.delta
            FROM address_history ah
            WHERE ah.address = ?
            ORDER BY ah.block_height DESC
            LIMIT ? OFFSET ?
            "#,
        )
        .bind(address)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(self.db.pool())
        .await?;

        let mut history = Vec::with_capacity(rows.len());
        for (txid_bytes, block_height, delta) in rows {
            let txid = IndexerDatabase::parse_txid(&txid_bytes)?;

            // Get timestamp from blocks table
            let timestamp: Option<(i64,)> =
                sqlx::query_as("SELECT timestamp FROM blocks WHERE height = ?")
                    .bind(block_height)
                    .fetch_optional(self.db.pool())
                    .await?;

            history.push(AddressHistory {
                txid,
                block_height: block_height as u32,
                delta,
                timestamp: timestamp.map(|(t,)| t as u32).unwrap_or(0),
            });
        }

        Ok(history)
    }

    /// Get unspent outputs for an address.
    #[allow(clippy::type_complexity)]
    pub async fn get_address_utxos(&self, address: &str) -> Result<Vec<Utxo>> {
        let rows: Vec<(Vec<u8>, i64, i64, i64, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT txid, vout, value, block_height, script_pubkey
            FROM outputs
            WHERE address = ? AND spent_txid IS NULL
            ORDER BY block_height DESC
            "#,
        )
        .bind(address)
        .fetch_all(self.db.pool())
        .await?;

        let mut utxos = Vec::with_capacity(rows.len());
        for (txid_bytes, vout, value, block_height, script_pubkey) in rows {
            let txid = IndexerDatabase::parse_txid(&txid_bytes)?;
            utxos.push(Utxo {
                txid,
                vout: vout as u32,
                value: value as u64,
                block_height: block_height as u32,
                script_pubkey,
            });
        }

        Ok(utxos)
    }

    /// Get address balance and statistics.
    pub async fn get_address_balance(&self, address: &str) -> Result<AddressBalance> {
        // Get confirmed balance (sum of unspent outputs)
        let confirmed: Option<(i64,)> = sqlx::query_as(
            "SELECT COALESCE(SUM(value), 0) FROM outputs WHERE address = ? AND spent_txid IS NULL",
        )
        .bind(address)
        .fetch_optional(self.db.pool())
        .await?;

        // Get total received
        let total_received: Option<(i64,)> =
            sqlx::query_as("SELECT COALESCE(SUM(value), 0) FROM outputs WHERE address = ?")
                .bind(address)
                .fetch_optional(self.db.pool())
                .await?;

        // Get total sent (sum of spent outputs)
        let total_sent: Option<(i64,)> = sqlx::query_as(
            "SELECT COALESCE(SUM(value), 0) FROM outputs WHERE address = ? AND spent_txid IS NOT NULL",
        )
        .bind(address)
        .fetch_optional(self.db.pool())
        .await?;

        // Get transaction count (distinct transactions)
        let tx_count: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(DISTINCT txid) FROM address_history WHERE address = ?")
                .bind(address)
                .fetch_optional(self.db.pool())
                .await?;

        // Get UTXO count
        let utxo_count: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM outputs WHERE address = ? AND spent_txid IS NULL")
                .bind(address)
                .fetch_optional(self.db.pool())
                .await?;

        Ok(AddressBalance {
            confirmed: confirmed.map(|(v,)| v as u64).unwrap_or(0),
            total_received: total_received.map(|(v,)| v as u64).unwrap_or(0),
            total_sent: total_sent.map(|(v,)| v as u64).unwrap_or(0),
            tx_count: tx_count.map(|(v,)| v as u64).unwrap_or(0),
            utxo_count: utxo_count.map(|(v,)| v as u64).unwrap_or(0),
        })
    }

    /// Get the number of transactions for an address.
    pub async fn get_address_tx_count(&self, address: &str) -> Result<u64> {
        let count: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(DISTINCT txid) FROM address_history WHERE address = ?")
                .bind(address)
                .fetch_optional(self.db.pool())
                .await?;
        Ok(count.map(|(v,)| v as u64).unwrap_or(0))
    }

    /// Get the current indexer status.
    pub async fn get_status(&self, chain_tip: u32) -> Result<IndexerStatus> {
        let state = self.db.load_state().await?;

        match state {
            Some(IndexerState::HistoricalIndexing {
                target_height,
                current_height,
            }) => {
                let progress = if target_height > 0 {
                    (current_height as f64 / target_height as f64) * 100.0
                } else {
                    0.0
                };
                Ok(IndexerStatus {
                    is_syncing: true,
                    indexed_height: current_height,
                    target_height: Some(target_height),
                    progress_percent: progress,
                })
            }
            Some(IndexerState::Active { last_indexed }) => {
                let progress = if chain_tip > 0 {
                    (last_indexed as f64 / chain_tip as f64) * 100.0
                } else {
                    100.0
                };
                Ok(IndexerStatus {
                    is_syncing: last_indexed < chain_tip,
                    indexed_height: last_indexed,
                    target_height: if last_indexed < chain_tip {
                        Some(chain_tip)
                    } else {
                        None
                    },
                    progress_percent: progress.min(100.0),
                })
            }
            None => Ok(IndexerStatus {
                is_syncing: chain_tip > 0,
                indexed_height: 0,
                target_height: if chain_tip > 0 { Some(chain_tip) } else { None },
                progress_percent: 0.0,
            }),
        }
    }
}

/// Implement the TransactionIndex trait from subcoin-primitives.
impl subcoin_primitives::TransactionIndex for IndexerQuery {
    fn tx_index(&self, txid: Txid) -> sp_blockchain::Result<Option<TxPosition>> {
        // We need to block on the async call since the trait is sync
        // This assumes we're running within a tokio context (which we always are in the node)
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| sp_blockchain::Error::Backend("No tokio runtime available".to_string()))?;

        let db = self.db.clone();
        let result = std::thread::spawn(move || {
            handle.block_on(async move { db.get_transaction(&txid).await })
        })
        .join()
        .map_err(|_| sp_blockchain::Error::Backend("Failed to query transaction".to_string()))?;

        result
            .map(|opt| {
                opt.map(|(block_number, index)| TxPosition {
                    block_number,
                    index,
                })
            })
            .map_err(|e| sp_blockchain::Error::Backend(e.to_string()))
    }
}
