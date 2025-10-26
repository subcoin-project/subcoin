use bitcoin::locktime::absolute;
use codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_std::vec::Vec;
pub use subcoin_runtime_primitives::{OutPoint, Txid};

/// An absolute lock time value, representing either a block height or a UNIX timestamp (seconds
/// since epoch).
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, DecodeWithMemTracking)]
pub enum LockTime {
    /// A block height lock time value.
    Height(u32),
    /// A UNIX timestamp lock time value.
    ///
    /// A UNIX timestamp, seconds since epoch, guaranteed to always contain a valid time value.
    Time(u32),
}

impl From<absolute::LockTime> for LockTime {
    fn from(lock_time: absolute::LockTime) -> Self {
        match lock_time {
            absolute::LockTime::Blocks(n) => Self::Height(n.to_consensus_u32()),
            absolute::LockTime::Seconds(n) => Self::Time(n.to_consensus_u32()),
        }
    }
}

impl From<LockTime> for absolute::LockTime {
    fn from(val: LockTime) -> Self {
        match val {
            LockTime::Height(n) => Self::Blocks(
                absolute::Height::from_consensus(n).expect("Invalid height in LockTime"),
            ),
            LockTime::Time(n) => {
                Self::Seconds(absolute::Time::from_consensus(n).expect("Invalid time in LockTime"))
            }
        }
    }
}

/// The Witness is the data used to unlock bitcoin since the [segwit upgrade].
///
/// Can be logically seen as an array of bytestrings, i.e. `Vec<Vec<u8>>`, and it is serialized on the wire
/// in that format.
///
/// [segwit upgrade]: <https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki>
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq)]
pub struct Witness {
    /// Contains the witness `Vec<Vec<u8>>` serialization without the initial varint indicating the
    /// number of elements (which is stored in `witness_elements`).
    content: Vec<u8>,
    /// The number of elements in the witness.
    ///
    /// Stored separately (instead of as a VarInt in the initial part of content) so that methods
    /// like [`Witness::push`] don't have to shift the entire array.
    // usize
    witness_elements: u64,
    /// This is the valid index pointing to the beginning of the index area. This area is 4 *
    /// stack_size bytes at the end of the content vector which stores the indices of each item.
    indices_start: u64,
}

/// Regular bitcoin transaction input.
///
/// Structurely same with [`bitcoin::TxIn`].
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, DecodeWithMemTracking)]
pub struct RegularTxIn {
    /// The reference to the previous output that is being used as an input.
    pub previous_output: OutPoint,
    /// The unlocking script (scriptSig) that pushes values onto the stack,
    /// enabling the referenced output's script to be satisfied.
    pub unlocking_script: Vec<u8>,
    /// The sequence number, which suggests to miners which of two
    /// conflicting transactions should be preferred, or 0xFFFFFFFF
    /// to ignore this feature. This is generally never used since
    /// the miner behavior cannot be enforced.
    pub sequence: u32,
    /// Witness data: an array of byte-arrays.
    /// Note that this field is *not* (de)serialized with the rest of the TxIn in
    /// Encodable/Decodable, as it is (de)serialized at the end of the full
    /// Transaction. It *is* (de)serialized with the rest of the TxIn in other
    /// (de)serialization routines.
    pub witness: Vec<Vec<u8>>,
}

/// Bitcoin transaction input.
///
/// This type is a wrapper around [`bitcoin::TxIn`], designed to provide a user-friendly representation
/// of transaction inputs (`TxIn`) in polkadot.js.org. It handles both coinbase (block reward)
/// transactions and standard transactions.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, DecodeWithMemTracking)]
pub enum TxIn {
    /// Input from a coinbase transaction, which does not reference any previous output.
    Coinbase {
        /// Arbitrary data used in the coinbase transaction, such as extra nonce or miner-specific information.
        data: Vec<u8>,
        /// Sequence.
        ///
        /// Note that the value of sequence is not always Sequence::MAX, see https://www.blockchain.com/explorer/transactions/btc/0961c660358478829505e16a1f028757e54b5bbf9758341a7546573738f31429
        sequence: u32,
    },
    /// Input from a regular transaction, which references a previous output (`OutPoint`).
    Regular(RegularTxIn),
}

impl From<bitcoin::TxIn> for TxIn {
    fn from(txin: bitcoin::TxIn) -> Self {
        if txin.previous_output.is_null() {
            Self::Coinbase {
                data: txin.script_sig.into_bytes(),
                sequence: txin.sequence.0,
            }
        } else {
            Self::Regular(RegularTxIn {
                previous_output: txin.previous_output.into(),
                unlocking_script: txin.script_sig.into_bytes(),
                sequence: txin.sequence.0,
                witness: txin.witness.to_vec(),
            })
        }
    }
}

impl From<TxIn> for bitcoin::TxIn {
    fn from(val: TxIn) -> Self {
        match val {
            TxIn::Coinbase { data, sequence } => bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::null(),
                script_sig: bitcoin::ScriptBuf::from_bytes(data),
                sequence: bitcoin::Sequence(sequence),
                witness: bitcoin::Witness::new(),
            },
            TxIn::Regular(RegularTxIn {
                previous_output,
                unlocking_script,
                sequence,
                witness,
            }) => bitcoin::TxIn {
                previous_output: previous_output.into(),
                script_sig: bitcoin::ScriptBuf::from_bytes(unlocking_script),
                sequence: bitcoin::Sequence(sequence),
                witness: witness.into(),
            },
        }
    }
}

/// Bitcoin transaction output.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, DecodeWithMemTracking)]
pub struct TxOut {
    /// The value of the output, in satoshis.
    pub value: u64,
    /// The script which must be satisfied for the output to be spent.
    pub script_pubkey: Vec<u8>,
}

/// Bitcoin transaction.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, DecodeWithMemTracking)]
pub struct Transaction {
    /// The protocol version, is currently expected to be 1 or 2 (BIP 68).
    pub version: i32,
    /// Block height or timestamp. Transaction cannot be included in a block until this height/time.
    pub lock_time: LockTime,
    /// List of transaction inputs.
    pub input: Vec<TxIn>,
    /// List of transaction outputs.
    pub output: Vec<TxOut>,
}

impl From<Transaction> for bitcoin::Transaction {
    fn from(val: Transaction) -> Self {
        let Transaction {
            version,
            lock_time,
            input,
            output,
        } = val;

        bitcoin::Transaction {
            version: bitcoin::transaction::Version(version),
            lock_time: lock_time.into(),
            input: input.into_iter().map(Into::into).collect(),
            output: output
                .into_iter()
                .map(|txout| bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(txout.value),
                    script_pubkey: bitcoin::ScriptBuf::from_bytes(txout.script_pubkey),
                })
                .collect(),
        }
    }
}

impl From<bitcoin::Transaction> for Transaction {
    fn from(btc_tx: bitcoin::Transaction) -> Self {
        Self {
            version: btc_tx.version.0,
            lock_time: btc_tx.lock_time.into(),
            input: btc_tx.input.into_iter().map(Into::into).collect(),
            output: btc_tx
                .output
                .into_iter()
                .map(|txout| TxOut {
                    value: txout.value.to_sat(),
                    script_pubkey: txout.script_pubkey.into_bytes(),
                })
                .collect(),
        }
    }
}
