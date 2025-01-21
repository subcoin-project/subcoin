use crate::num::ScriptNum;
use crate::{EcdsaSignature, PublicKey, SchnorrSignature, ScriptExecutionData, SigVersion};
use bitcoin::secp256k1::{Error, Message, Secp256k1, VerifyOnly};
use bitcoin::{Script, XOnlyPublicKey};

/// Checks transaction signature
pub trait SignatureChecker {
    fn verify_ecdsa_signature(
        &self,
        sig: &EcdsaSignature,
        msg: &Message,
        pubkey: &PublicKey,
    ) -> Result<(), Error>;

    fn check_ecdsa_signature(
        &self,
        script_sig: &EcdsaSignature,
        pubkey: &PublicKey,
        script_code: &Script,
        sig_version: SigVersion,
    ) -> Result<bool, Error>;

    fn verify_schnorr_signature(
        &self,
        sig: &SchnorrSignature,
        msg: &Message,
        pubkey: &XOnlyPublicKey,
    ) -> Result<(), Error>;

    fn check_schnorr_signature(
        &self,
        sig: &SchnorrSignature,
        pubkey: &PublicKey,
        sig_version: SigVersion,
        exec_data: &ScriptExecutionData,
    ) -> Result<bool, Error>;

    fn check_lock_time(&self, lock_time: ScriptNum) -> bool;

    fn check_sequence(&self, sequence: ScriptNum) -> bool;
}

pub struct SkipSignatureCheck {
    secp: Secp256k1<VerifyOnly>,
}

impl SkipSignatureCheck {
    pub fn new() -> Self {
        Self {
            secp: Secp256k1::verification_only(),
        }
    }
}

impl SignatureChecker for SkipSignatureCheck {
    fn verify_ecdsa_signature(
        &self,
        sig: &EcdsaSignature,
        msg: &Message,
        public: &PublicKey,
    ) -> Result<(), Error> {
        public.verify(&self.secp, msg, sig)
    }

    fn check_ecdsa_signature(
        &self,
        _script_sig: &EcdsaSignature,
        _pubkey: &PublicKey,
        _script_code: &Script,
        _sig_version: SigVersion,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    fn verify_schnorr_signature(
        &self,
        sig: &SchnorrSignature,
        msg: &Message,
        pubkey: &XOnlyPublicKey,
    ) -> Result<(), Error> {
        self.secp.verify_schnorr(sig, msg, pubkey)
    }

    fn check_schnorr_signature(
        &self,
        _sig: &SchnorrSignature,
        _pubkey: &PublicKey,
        _sig_version: SigVersion,
        _exec_data: &ScriptExecutionData,
    ) -> Result<bool, Error> {
        Ok(true)
    }

    fn check_lock_time(&self, _lock_time: ScriptNum) -> bool {
        true
    }

    fn check_sequence(&self, _sequence: ScriptNum) -> bool {
        true
    }
}
