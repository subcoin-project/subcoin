use std::sync::LazyLock;

use crate::num::ScriptNum;
use crate::{EcdsaSignature, SchnorrSignature, ScriptExecutionData, SigVersion};
use bitcoin::secp256k1::{All, Error, Message, Secp256k1};
use bitcoin::sighash::{Annex, Prevouts, SighashCache, TaprootError};
use bitcoin::taproot::TapLeafHash;
use bitcoin::{Amount, PublicKey, Script, Transaction, TxOut, XOnlyPublicKey};

static SECP: LazyLock<Secp256k1<All>> = LazyLock::new(|| Secp256k1::new());

/// Checks transaction signature.
pub trait SignatureChecker {
    fn verify_ecdsa_signature(
        &self,
        sig: &EcdsaSignature,
        msg: &Message,
        pk: &PublicKey,
    ) -> Result<(), Error> {
        pk.verify(&SECP, msg, sig)
    }

    fn check_ecdsa_signature(
        &mut self,
        sig: &EcdsaSignature,
        pk: &PublicKey,
        script_code: &Script,
        sig_version: SigVersion,
    ) -> Result<bool, Error>;

    fn verify_schnorr_signature(
        &self,
        sig: &SchnorrSignature,
        msg: &Message,
        pk: &XOnlyPublicKey,
    ) -> Result<(), Error> {
        pk.verify(&SECP, msg, &sig.signature)
    }

    fn check_schnorr_signature(
        &mut self,
        sig: &SchnorrSignature,
        pk: &XOnlyPublicKey,
        sig_version: SigVersion,
        exec_data: &ScriptExecutionData,
    ) -> Result<bool, TaprootError>;

    fn check_lock_time(&self, lock_time: ScriptNum) -> bool;

    fn check_sequence(&self, sequence: ScriptNum) -> bool;
}

/// A SignatureChecker implementation that skips all signature checks.
pub struct NoSignatureCheck;

impl SignatureChecker for NoSignatureCheck {
    fn check_ecdsa_signature(
        &mut self,
        _sig: &EcdsaSignature,
        _pk: &PublicKey,
        _script_code: &Script,
        _sig_version: SigVersion,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    fn check_schnorr_signature(
        &mut self,
        _sig: &SchnorrSignature,
        _pk: &XOnlyPublicKey,
        _sig_version: SigVersion,
        _exec_data: &ScriptExecutionData,
    ) -> Result<bool, TaprootError> {
        Ok(true)
    }

    fn check_lock_time(&self, _lock_time: ScriptNum) -> bool {
        true
    }

    fn check_sequence(&self, _sequence: ScriptNum) -> bool {
        true
    }
}

/// A SignatureChecker implementation that skips all signature checks.
pub struct TransactionSignatureChecker {
    input_index: usize,
    input_amount: u64,
    prevouts: Vec<TxOut>,
    sighash_cache: SighashCache<Transaction>,
    taproot_annex_scriptleaf: Option<(TapLeafHash, Option<Vec<u8>>)>,
}

impl SignatureChecker for TransactionSignatureChecker {
    fn check_ecdsa_signature(
        &mut self,
        sig: &EcdsaSignature,
        pk: &PublicKey,
        script_pubkey: &Script,
        sig_version: SigVersion,
    ) -> Result<bool, Error> {
        let msg: Message = match sig_version {
            SigVersion::Base => self
                .sighash_cache
                .legacy_signature_hash(self.input_index, script_pubkey, sig.sighash_type.to_u32())
                .unwrap()
                .into(),
            SigVersion::WitnessV0 => self
                .sighash_cache
                .p2wsh_signature_hash(
                    self.input_index,
                    script_pubkey,
                    Amount::from_sat(self.input_amount),
                    sig.sighash_type,
                )
                .unwrap()
                .into(),
            _ => unreachable!("Expect ECDSA signature only"),
        };

        Ok(self.verify_ecdsa_signature(sig, &msg, pk).is_ok())
    }

    fn check_schnorr_signature(
        &mut self,
        sig: &SchnorrSignature,
        pk: &XOnlyPublicKey,
        _sig_version: SigVersion,
        _exec_data: &ScriptExecutionData,
    ) -> Result<bool, TaprootError> {
        // TODO:
        let last_codeseparator_pos = None;
        let (leaf_hash, annex) = self.taproot_annex_scriptleaf.as_ref().unwrap();
        let sighash = self.sighash_cache.taproot_signature_hash(
            self.input_index,
            &Prevouts::All(&self.prevouts),
            annex
                .as_ref()
                .map(|a| Annex::new(a).expect("we checked annex prefix before")),
            Some((*leaf_hash, last_codeseparator_pos.unwrap_or(u32::MAX))),
            sig.sighash_type,
        )?;

        let msg: Message = sighash.into();

        Ok(self.verify_schnorr_signature(sig, &msg, pk).is_ok())
    }

    fn check_lock_time(&self, _lock_time: ScriptNum) -> bool {
        true
    }

    fn check_sequence(&self, _sequence: ScriptNum) -> bool {
        true
    }
}
