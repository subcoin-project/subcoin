use crate::cli::subcoin_params::CommonParams;
use sc_cli::{ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sc_consensus::{
    BlockImport, BlockImportParams, ForkChoiceStrategy, ImportedState, StateAction,
};
use sp_consensus::BlockOrigin;
use sp_core::Decode;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BackendExt, CoinStorageKey};
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_service::FullClient;

/// ImportChainState.
#[derive(Debug, clap::Parser)]
pub struct ImportChainState {
    /// Number of the block state.
    #[clap(long)]
    height: u32,

    /// Local file path of the state.
    #[clap(long)]
    path: PathBuf,

    #[allow(missing_docs)]
    #[clap(flatten)]
    common_params: CommonParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    import_params: ImportParams,
}

pub struct ImportChainStateCmd {
    height: u32,
    path: PathBuf,
    shared_params: SharedParams,
    import_params: ImportParams,
}

impl ImportChainStateCmd {
    /// Constructs a new instance of [`ImportChainStateCmd`].
    pub fn new(import_chain_state: ImportChainState) -> Self {
        Self {
            height: import_chain_state.height,
            path: import_chain_state.path.clone(),
            shared_params: import_chain_state.common_params.as_shared_params(),
            import_params: import_chain_state.import_params.clone(),
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let info = client.info();
        println!("info: {info:?}");

        let now = std::time::Instant::now();
        tracing::info!(
            "Before loading state, memory usage: {:?}",
            memory_stats::memory_stats().map(|usage| usage.physical_mem)
        );
        let key_values = load_data(&self.path)?;
        tracing::info!(
            "After loading state, memory usage: {:?}",
            memory_stats::memory_stats().map(|usage| usage.physical_mem)
        );
        tracing::info!("State loaded in {}ms", now.elapsed().as_millis());
        tracing::info!("Total key values: {}", key_values.len());

        let block_hash = client.hash(self.height)?.expect("Hash not found");
        let header = client.header(block_hash)?.expect("Header not found");

        let imported_state = ImportedState::<Block> {
            block: block_hash,
            state: sp_state_machine::KeyValueStates(vec![sp_state_machine::KeyValueStorageLevel {
                state_root: Vec::new(),
                parent_storage_keys: Vec::new(),
                key_values,
            }]),
        };

        let mut import_params = BlockImportParams::new(BlockOrigin::Own, header);
        import_params.import_existing = true;
        import_params.fork_choice = Some(ForkChoiceStrategy::LongestChain);
        import_params.state_action = StateAction::ApplyChanges(
            sc_consensus::block_import::StorageChanges::Import(imported_state),
        );

        tracing::info!("Start to import block: #{},{block_hash}", self.height);
        tracing::info!(
            "Memory usage: {:?}",
            memory_stats::memory_stats().map(|usage| usage.physical_mem)
        );
        let res = client.import_block(import_params).await;

        println!("Import result: {res:?}");

        Ok(())
    }
}

fn load_data(path: &Path) -> std::io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    use byteorder::{LittleEndian, ReadBytesExt};
    use memmap2::Mmap;
    use std::fs::File;
    use std::io::{Cursor, Read};

    let file = File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };

    let mut cursor = Cursor::new(&mmap[..]);
    let mut data = Vec::new();

    let mut entries = 0;

    while cursor.position() < mmap.len() as u64 {
        if (cursor.position() as usize + 4) > mmap.len() {
            break;
        }

        let key_len = cursor.read_u32::<LittleEndian>()? as usize;

        if (cursor.position() as usize + key_len) > mmap.len() {
            break;
        }
        let mut key = vec![0; key_len];
        cursor.read_exact(&mut key)?;

        // Ensure there's enough data to read the value length
        if (cursor.position() as usize + 4) > mmap.len() {
            break;
        }
        let value_len = cursor.read_u32::<LittleEndian>()? as usize;

        // Ensure there's enough data to read the value
        if (cursor.position() as usize + value_len) > mmap.len() {
            break;
        }
        let mut value = vec![0; value_len];
        cursor.read_exact(&mut value)?;

        data.push((key, value));

        entries += 1;
    }

    let total_bytes = data.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>();

    println!("Total bytes: {total_bytes}");

    Ok(data)
}

impl sc_cli::CliConfiguration for ImportChainStateCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn import_params(&self) -> Option<&ImportParams> {
        Some(&self.import_params)
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}
