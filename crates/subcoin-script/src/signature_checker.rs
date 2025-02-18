use crate::num::ScriptNum;
use crate::{EcdsaSignature, SchnorrSignature, ScriptExecutionData, SigVersion};
use bitcoin::locktime::absolute::LockTime as AbsoluteLockTime;
use bitcoin::locktime::relative::LockTime as RelativeLockTime;
use bitcoin::secp256k1::{self, All, Message, Secp256k1};
use bitcoin::sighash::{Annex, Prevouts, SighashCache, TaprootError};
use bitcoin::transaction::Version;
use bitcoin::{Amount, EcdsaSighashType, PublicKey, Script, Transaction, TxOut, XOnlyPublicKey};
use std::sync::LazyLock;

pub(crate) static SECP: LazyLock<Secp256k1<All>> = LazyLock::new(Secp256k1::new);

/// Error types related to signature verification.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum SignatureError {
    #[error("Invalid signature version")]
    InvalidSignatureVersion,
    #[error("ecdsa error: {0:?}")]
    Ecdsa(secp256k1::Error),
    #[error("Failed to parse ECDSA signature: {0:?}")]
    ParseEcdsaSignature(bitcoin::ecdsa::Error),
    #[error("schnorr error: {0:?}")]
    Schnorr(secp256k1::Error),
    #[error("Ecdsa sighash error: {0:?}")]
    EcdsaSignatureHash(bitcoin::blockdata::transaction::InputsIndexError),
    #[error("Taproot sighash error: {0:?}")]
    TaprootSignatureHash(TaprootError),
}

/// Trait for verifying Bitcoin transaction signatures.
pub trait SignatureChecker {
    /// Verifies an ECDSA signature against a message and public key.
    ///
    /// # Arguments
    /// * `sig` - The ECDSA signature to verify.
    /// * `msg` - The message that was signed.
    /// * `pk` - The public key corresponding to the signature.
    ///
    /// # Returns
    /// - `true` if the signature is valid.
    /// - `false` if the signature is invalid or an error occurs.
    fn verify_ecdsa_signature(&self, sig: &EcdsaSignature, msg: &Message, pk: &PublicKey) -> bool {
        SECP.verify_ecdsa(msg, &sig.signature, &pk.inner)
            .inspect_err(|err| {
                tracing::trace!(
                    ?err,
                    "[verify_ecdsa_signature] Failed to verify ecdsa signature"
                );
            })
            .is_ok()
    }

    /// Checks an ECDSA signature in the context of a Bitcoin transaction.
    ///
    /// # Arguments
    /// * `sig` - The ECDSA signature to check.
    /// * `pk` - The public key corresponding to the signature.
    /// * `script_code` - The script code for the transaction input.
    /// * `sig_version` - The signature version (e.g., [`SigVersion::Base`] or [`SigVersion::WitnessV0`]).
    ///
    /// # Returns
    /// - `Ok(true)` if the signature is valid.
    /// - `Ok(false)` if the signature is invalid.
    /// - `Err(SignatureError)` if an error occurs.
    ///
    /// # Notes
    /// In the context of multisignature transactions, it is expected that not all signatures may be valid.
    /// An invalid signature may be considered legitimate as long as the multisig conditions are met.
    fn check_ecdsa_signature(
        &mut self,
        sig: &EcdsaSignature,
        pk: &PublicKey,
        script_code: &Script,
        sig_version: SigVersion,
    ) -> Result<bool, SignatureError>;

    /// Verifies a Schnorr signature against a message and public key.
    ///
    /// # Arguments
    /// * `sig` - The Schnorr signature to verify.
    /// * `msg` - The message that was signed.
    /// * `pk` - The public key corresponding to the signature.
    ///
    /// # Returns
    /// - `true` if the signature is valid.
    /// - `false` if the signature is invalid or an error occurs.
    fn verify_schnorr_signature(
        &self,
        sig: &SchnorrSignature,
        msg: &Message,
        pk: &XOnlyPublicKey,
    ) -> bool {
        SECP.verify_schnorr(&sig.signature, msg, pk)
            .inspect_err(|err| {
                tracing::trace!(
                    ?err,
                    "[verify_schnorr_signature] Failed to verify schnorr signature"
                );
            })
            .is_ok()
    }

    /// Checks a Schnorr signature in the context of a Bitcoin transaction.
    ///
    /// # Arguments
    /// * `sig` - The Schnorr signature to check.
    /// * `pk` - The public key corresponding to the signature.
    /// * `sig_version` - The signature version (e.g., `SigVersion::Taproot`).
    /// * `exec_data` - Additional execution data for Taproot scripts.
    ///
    /// # Returns
    /// - `Ok(true)` if the signature is valid.
    /// - `Ok(false)` if the signature is invalid.
    /// - `Err(SignatureError)` if an error occurs.
    fn check_schnorr_signature(
        &mut self,
        sig: &SchnorrSignature,
        pk: &XOnlyPublicKey,
        sig_version: SigVersion,
        exec_data: &ScriptExecutionData,
    ) -> Result<bool, SignatureError>;

    /// Checks whether the absolute time lock (`lock_time`) in a transaction is satisfied.
    fn check_lock_time(&self, lock_time: ScriptNum) -> bool;

    /// Checks whether the relative time lock (`sequence`) for a specific input
    /// in a transaction is satisfied.
    fn check_sequence(&self, sequence: ScriptNum) -> bool;
}

/// Verify the signature against the public key.
pub fn check_ecdsa_signature(
    sig: &[u8],
    public_key: &[u8],
    checker: &mut impl SignatureChecker,
    subscript: &Script,
    sig_version: SigVersion,
) -> bool {
    tracing::trace!(
        "[check_ecdsa_signature] sig: {}, public_key: {}, subscript: {}",
        hex::encode(sig),
        hex::encode(public_key),
        hex::encode(subscript.as_bytes())
    );

    let sig = match EcdsaSignature::parse_der_lax(sig) {
        Ok(sig) => sig,
        Err(err) => {
            tracing::trace!(
                ?err,
                "Failed to parse ecdsa signature from {}",
                hex::encode(sig)
            );
            return false;
        }
    };

    match PublicKey::from_slice(public_key) {
        Ok(key) => checker
            .check_ecdsa_signature(&sig, &key, subscript, sig_version)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// A [`SignatureChecker`] implementation that skips all signature checks.
pub struct NoSignatureCheck;

impl SignatureChecker for NoSignatureCheck {
    fn check_ecdsa_signature(
        &mut self,
        _sig: &EcdsaSignature,
        _pk: &PublicKey,
        _script_code: &Script,
        _sig_version: SigVersion,
    ) -> Result<bool, SignatureError> {
        Ok(true)
    }

    fn check_schnorr_signature(
        &mut self,
        _sig: &SchnorrSignature,
        _pk: &XOnlyPublicKey,
        _sig_version: SigVersion,
        _exec_data: &ScriptExecutionData,
    ) -> Result<bool, SignatureError> {
        Ok(true)
    }

    fn check_lock_time(&self, _lock_time: ScriptNum) -> bool {
        true
    }

    fn check_sequence(&self, _sequence: ScriptNum) -> bool {
        true
    }
}

/// A [`SignatureChecker`] implementation for transactions.
pub struct TransactionSignatureChecker<'a> {
    tx: &'a Transaction,
    input_index: usize,
    input_amount: u64,
    prev_outs: Vec<TxOut>,
    sighash_cache: SighashCache<&'a Transaction>,
}

impl<'a> TransactionSignatureChecker<'a> {
    /// Constructs a new instance of [`TransactionSignatureChecker`].
    pub fn new(tx: &'a Transaction, input_index: usize, input_amount: u64) -> Self {
        let sighash_cache = SighashCache::new(tx);
        Self {
            tx,
            input_index,
            input_amount,
            prev_outs: Vec::new(),
            sighash_cache,
        }
    }
}

impl SignatureChecker for TransactionSignatureChecker<'_> {
    fn check_ecdsa_signature(
        &mut self,
        sig: &EcdsaSignature,
        pk: &PublicKey,
        script_pubkey: &Script,
        sig_version: SigVersion,
    ) -> Result<bool, SignatureError> {
        let msg: Message = match sig_version {
            SigVersion::Base => self
                .sighash_cache
                .legacy_signature_hash(self.input_index, script_pubkey, sig.sighash_type)
                .map_err(SignatureError::EcdsaSignatureHash)?
                .into(),
            SigVersion::WitnessV0 => self
                .sighash_cache
                .p2wsh_signature_hash(
                    self.input_index,
                    script_pubkey,
                    Amount::from_sat(self.input_amount),
                    EcdsaSighashType::from_consensus(sig.sighash_type),
                )
                .map_err(SignatureError::EcdsaSignatureHash)?
                .into(),
            _ => return Err(SignatureError::InvalidSignatureVersion),
        };

        let is_valid_signature = self.verify_ecdsa_signature(sig, &msg, pk);

        if !is_valid_signature {
            tracing::debug!(
                ?sig,
                ?pk,
                ?script_pubkey,
                ?sig_version,
                ?msg,
                "[check_ecdsa_signature] Invalid ECDSA signature"
            );
        }

        Ok(is_valid_signature)
    }

    fn check_schnorr_signature(
        &mut self,
        sig: &SchnorrSignature,
        pk: &XOnlyPublicKey,
        sig_version: SigVersion,
        exec_data: &ScriptExecutionData,
    ) -> Result<bool, SignatureError> {
        if !matches!(sig_version, SigVersion::Taproot | SigVersion::Tapscript) {
            return Err(SignatureError::InvalidSignatureVersion);
        }

        let last_codeseparator_pos = if exec_data.codeseparator_pos_init {
            Some(exec_data.codeseparator_pos)
        } else {
            None
        };

        let leaf_hash = exec_data.tapleaf_hash;
        let annex = exec_data.annex.as_ref().map(|a| {
            Annex::new(a).expect("Annex must be valid as it was checked on initialization; qed")
        });

        let sighash = self
            .sighash_cache
            .taproot_signature_hash(
                self.input_index,
                &Prevouts::All(&self.prev_outs),
                annex,
                Some((leaf_hash, last_codeseparator_pos.unwrap_or(u32::MAX))),
                sig.sighash_type,
            )
            .map_err(SignatureError::TaprootSignatureHash)?;

        let msg: Message = sighash.into();

        Ok(self.verify_schnorr_signature(sig, &msg, pk))
    }

    /// This function verifies that the transaction's `nLockTime` field meets the conditions specified
    /// by the `lock_time` parameter.
    ///
    /// The `lock_time` can represent either a block height or a Unix timestamp, depending on its value:
    /// - If `lock_time < 500,000,000`, it is interpreted as a block height.
    /// - If `lock_time >= 500,000,000`, it is interpreted as a Unix timestamp.
    ///
    /// The lock is satisfied if:
    /// 1. The `lock_time` is of the same type (block height or timestamp) as the transaction's `nLockTime`.
    /// 2. The `lock_time` is less than or equal to the transaction's `nLockTime`.
    /// 3. The transaction's `nSequence` field for the input is not set to the maximum value (`0xFFFFFFFF`),
    ///    which would disable the time lock.
    ///
    /// # Arguments
    /// * `lock_time` - The absolute time lock to check, represented as a [`ScriptNum`].
    ///
    /// # Returns
    /// - `true` if the absolute time lock is satisfied.
    /// - `false` otherwise.
    fn check_lock_time(&self, lock_time: ScriptNum) -> bool {
        let Ok(lock_time) = u32::try_from(lock_time.value()).map(AbsoluteLockTime::from_consensus)
        else {
            return false;
        };

        // There are two kinds of nLockTime: lock-by-blockheight
        // and lock-by-blocktime, distinguished by whether
        // nLockTime < LOCKTIME_THRESHOLD.
        //
        // We want to compare apples to apples, so fail the script
        // unless the type of nLockTime being tested is the same as
        // the nLockTime in the transaction.
        //
        // Now that we know we're comparing apples-to-apples, the
        // comparison is a simple numeric one.
        match (lock_time, self.tx.lock_time) {
            (AbsoluteLockTime::Blocks(h1), AbsoluteLockTime::Blocks(h2)) if h1 > h2 => {
                return false;
            }
            (AbsoluteLockTime::Seconds(t1), AbsoluteLockTime::Seconds(t2)) if t1 > t2 => {
                return false;
            }
            (AbsoluteLockTime::Blocks(_), AbsoluteLockTime::Seconds(_)) => return false,
            (AbsoluteLockTime::Seconds(_), AbsoluteLockTime::Blocks(_)) => return false,
            _ => {}
        }

        // Finally the nLockTime feature can be disabled and thus
        // CHECKLOCKTIMEVERIFY bypassed if every txin has been
        // finalized by setting nSequence to maxint. The
        // transaction would be allowed into the blockchain, making
        // the opcode ineffective.
        //
        // Testing if this vin is not final is sufficient to
        // prevent this condition. Alternatively we could test all
        // inputs, but testing just this input minimizes the data
        // required to prove correct CHECKLOCKTIMEVERIFY execution.
        self.tx.input[self.input_index].sequence.is_final()
    }

    /// The lock is satisfied if:
    /// 1. The transaction's version is at least 2 (BIP 68).
    /// 2. The `sequence` is of the same type (blocks or seconds) as the input's `nSequence`.
    /// 3. The `sequence` is less than or equal to the input's `nSequence`.
    ///
    /// # Arguments
    /// * `sequence` - The relative time lock to check, represented as a [`ScriptNum`].
    ///
    /// # Returns
    /// - `true` if the relative time lock is satisfied.
    /// - `false` otherwise.
    fn check_sequence(&self, sequence: ScriptNum) -> bool {
        // Fail if the transaction's version number is not set high
        // enough to trigger BIP 68 rules.
        if self.tx.version < Version::TWO {
            return false;
        }

        // Relative lock times are supported by comparing the passed
        // in operand to the sequence number of the input.
        let Some(input_lock_time) = self.tx.input[self.input_index]
            .sequence
            .to_relative_lock_time()
        else {
            return false;
        };

        let Some(lock_time) = u32::try_from(sequence.value())
            .ok()
            .and_then(|seq| RelativeLockTime::from_consensus(seq).ok())
        else {
            return false;
        };

        match (lock_time, input_lock_time) {
            (RelativeLockTime::Blocks(h1), RelativeLockTime::Blocks(h2)) if h1 > h2 => {
                return false
            }
            (RelativeLockTime::Time(t1), RelativeLockTime::Time(t2)) if t1 > t2 => return false,
            (RelativeLockTime::Blocks(_), RelativeLockTime::Time(_)) => return false,
            (RelativeLockTime::Time(_), RelativeLockTime::Blocks(_)) => return false,
            _ => {}
        }

        true
    }
}
