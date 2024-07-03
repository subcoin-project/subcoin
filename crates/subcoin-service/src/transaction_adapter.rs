use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::Transaction;
use sp_core::{Decode, Encode};
use sp_runtime::traits::{Block as BlockT, Extrinsic};
use subcoin_primitives::BitcoinTransactionAdapter;

/// Responsible for doing the conversion between Bitcoin transaction and Substrate extrinsic.
///
/// ## NOTE
///
/// To convert the bitcoin transactions to substrate extrinsics, the recommended
/// practice in Substrate is to leverage a runtime api for forkless upgrade such that
/// the client (node binary) does not have to be upgraded when the Pallet/Runtime call
/// is changed within the runtime. However, we don't need this convenience as the
/// pallet-bitcoin is designed to be super stable with one and only one call. Hence we
/// choose to pull in the pallet-bitcoin dependency directly for saving the cost of
/// calling a runtime api.
///
/// Using a trait also allows not to introduce the subcoin_runtime and pallet_bitcoin
/// deps when the adapter is needed in other crates, making the compilation faster.
pub struct TransactionAdapter;

impl<Block: BlockT> BitcoinTransactionAdapter<Block> for TransactionAdapter {
    fn extrinsic_to_bitcoin_transaction(extrinsic: &Block::Extrinsic) -> Transaction {
        let unchecked_extrinsic: subcoin_runtime::UncheckedExtrinsic =
            Decode::decode(&mut extrinsic.encode().as_slice()).unwrap();

        match unchecked_extrinsic.function {
            subcoin_runtime::RuntimeCall::Bitcoin(pallet_bitcoin::Call::<
                subcoin_runtime::Runtime,
            >::transact {
                btc_tx,
            }) => Transaction::consensus_decode(&mut btc_tx.as_slice())
                .expect("Transaction decode must succeed otherwise the chain is broken; qed"),
            _ => unreachable!("Transactions only exist in pallet-bitcoin; qed"),
        }
    }

    fn bitcoin_transaction_into_extrinsic(btc_tx: &bitcoin::Transaction) -> Block::Extrinsic {
        let mut data = Vec::new();
        btc_tx
            .consensus_encode(&mut data)
            .expect("Encoding bitcoin tx in a bitcoin block must succeed; qed");

        Decode::decode(
            &mut subcoin_runtime::UncheckedExtrinsic::new(
                pallet_bitcoin::Call::<subcoin_runtime::Runtime>::transact { btc_tx: data }.into(),
                None,
            )
            .expect("Internally construct extrinsic must not fail; qed")
            .encode()
            .as_slice(),
        )
        .expect("Internally construct extrinsic must not fail; qed")
    }
}
