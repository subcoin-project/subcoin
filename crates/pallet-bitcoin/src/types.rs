use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_std::vec::Vec;

/// An absolute lock time value, representing either a block height or a UNIX timestamp (seconds
/// since epoch).
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq)]
pub enum LockTime {
    /// A block height lock time value.
    Blocks(u32),
    /// A UNIX timestamp lock time value.
    Seconds(u32),
}

impl From<bitcoin::locktime::absolute::LockTime> for LockTime {
    fn from(lock_time: bitcoin::locktime::absolute::LockTime) -> Self {
        match lock_time {
            bitcoin::locktime::absolute::LockTime::Blocks(n) => Self::Blocks(n.to_consensus_u32()),
            bitcoin::locktime::absolute::LockTime::Seconds(n) => {
                Self::Seconds(n.to_consensus_u32())
            }
        }
    }
}

impl Into<bitcoin::locktime::absolute::LockTime> for LockTime {
    fn into(self) -> bitcoin::locktime::absolute::LockTime {
        match self {
            Self::Blocks(n) => bitcoin::locktime::absolute::LockTime::Blocks(
                bitcoin::locktime::absolute::Height::from_consensus(n)
                    .expect("Invalid height in LockTime"),
            ),
            Self::Seconds(n) => bitcoin::locktime::absolute::LockTime::Seconds(
                bitcoin::locktime::absolute::Time::from_consensus(n)
                    .expect("Invalid time in LockTime"),
            ),
        }
    }
}

/// Wrapper type for Bitcoin txid in runtime as `bitcoin::Txid` does not implement codec.
#[derive(Clone, TypeInfo, Encode, Decode, MaxEncodedLen, PartialEq)]
pub struct Txid(H256);

impl Txid {
    /// Converts `bitcoin::Txid` to [`Txid`].
    pub fn from_bitcoin_txid(txid: bitcoin::Txid) -> Self {
        let mut d = Vec::with_capacity(32);
        txid.consensus_encode(&mut d)
            .expect("txid must be encoded correctly; qed");

        let d: [u8; 32] = d
            .try_into()
            .expect("Bitcoin txid is sha256 hash which must fit into [u8; 32]; qed");

        Self(H256::from(d))
    }

    /// Converts the runtime [`Txid`] to a `bitcoin::Txid`.
    pub fn into_bitcoin_txid(self) -> bitcoin::Txid {
        bitcoin::consensus::Decodable::consensus_decode(&mut self.encode().as_slice())
            .expect("Decode must succeed as txid was ensured to be encoded correctly; qed")
    }
}

impl core::fmt::Debug for Txid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for byte in self.0.as_bytes().iter().rev() {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

/// A reference to a transaction output.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, MaxEncodedLen)]
pub struct OutPoint {
    pub txid: Txid,
    pub vout: u32,
}

impl From<bitcoin::OutPoint> for OutPoint {
    fn from(out_point: bitcoin::OutPoint) -> Self {
        Self {
            txid: Txid::from_bitcoin_txid(out_point.txid),
            vout: out_point.vout,
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

/// Bitcoin transaction input.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq)]
pub struct TxIn {
    /// The reference to the previous output that is being used as an input.
    pub previous_output: OutPoint,
    /// The script which pushes values on the stack which will cause
    /// the referenced output's script to be accepted.
    pub script_sig: Vec<u8>,
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

/// Bitcoin transaction output.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq)]
pub struct TxOut {
    /// The value of the output, in satoshis.
    pub value: u64,
    /// The script which must be satisfied for the output to be spent.
    pub script_pubkey: Vec<u8>,
}

/// Bitcoin transaction.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq)]
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

impl Into<bitcoin::Transaction> for Transaction {
    fn into(self) -> bitcoin::Transaction {
        let Self {
            version,
            lock_time,
            input,
            output,
        } = self;

        bitcoin::Transaction {
            version: bitcoin::transaction::Version(version),
            lock_time: lock_time.into(),
            input: input
                .into_iter()
                .map(|txin| bitcoin::TxIn {
                    previous_output: bitcoin::OutPoint {
                        txid: bitcoin::Txid::from_slice(&txin.previous_output.txid.encode())
                            .expect(
                                "Txid must be valid as Transaction is constructed internally; qed",
                            ),
                        vout: txin.previous_output.vout,
                    },
                    script_sig: bitcoin::ScriptBuf::from_bytes(txin.script_sig),
                    sequence: bitcoin::Sequence(txin.sequence),
                    witness: txin.witness.into(),
                })
                .collect(),
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
            input: btc_tx
                .input
                .into_iter()
                .map(|txin| TxIn {
                    previous_output: OutPoint {
                        txid: crate::Txid::from_bitcoin_txid(txin.previous_output.txid),
                        vout: txin.previous_output.vout,
                    },
                    script_sig: txin.script_sig.into_bytes(),
                    sequence: txin.sequence.0,
                    witness: txin.witness.to_vec(),
                })
                .collect(),
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
