use bitcoin::opcodes::all::{OP_CHECKMULTISIG, OP_PUSHNUM_1, OP_PUSHNUM_16};
use bitcoin::script::Instruction;
use bitcoin::{Opcode, PublicKey, Script, WitnessVersion};

/// Transaction output types
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TxoutType {
    NonStandard,
    // anyone can spend script.
    Anchor,
    PubKey(PublicKey),
    PubKeyHash([u8; 20]),
    ScriptHash([u8; 20]),
    Multisig {
        required_sigs: u8,
        keys_count: u8,
        keys: Vec<Vec<u8>>,
    },
    // unspendable OP_RETURN script that carries data.
    NullData,
    WitnessV0ScriptHash([u8; 32]),
    WitnessV0KeyHash([u8; 20]),
    WitnessV1Taproot([u8; 32]),
    // Only for Witness versions not already defined above.
    WitnessUnknown(Vec<Vec<u8>>),
}

impl TxoutType {
    /// Returns the script type name as used in Bitcoin Core RPC responses.
    pub fn script_type(&self) -> &'static str {
        match self {
            Self::NonStandard => "nonstandard",
            Self::Anchor => "anchor",
            Self::PubKey(_) => "pubkey",
            Self::PubKeyHash(_) => "pubkeyhash",
            Self::ScriptHash(_) => "scripthash",
            Self::Multisig { .. } => "multisig",
            Self::NullData => "nulldata",
            Self::WitnessV0ScriptHash(_) => "witness_v0_scripthash",
            Self::WitnessV0KeyHash(_) => "witness_v0_keyhash",
            Self::WitnessV1Taproot(_) => "witness_v1_taproot",
            Self::WitnessUnknown(_) => "witness_unknown",
        }
    }
}

pub fn solve(script_pubkey: &Script) -> TxoutType {
    if script_pubkey.is_p2sh() {
        let hash: [u8; 20] = script_pubkey.as_bytes()[2..22].try_into().unwrap();
        return TxoutType::ScriptHash(hash);
    }

    if let Some(version) = script_pubkey.witness_version() {
        let program = &script_pubkey.as_bytes()[2..];

        match (version, program.len()) {
            (WitnessVersion::V0, 20) => {
                return TxoutType::WitnessV0KeyHash(program.try_into().unwrap());
            }
            (WitnessVersion::V0, 32) => {
                return TxoutType::WitnessV0ScriptHash(program.try_into().unwrap());
            }
            (WitnessVersion::V1, 32) => {
                return TxoutType::WitnessV1Taproot(program.try_into().unwrap());
            }
            _ => {}
        }

        if is_pay_to_anchor(script_pubkey) {
            return TxoutType::Anchor;
        }

        if version != WitnessVersion::V0 {
            return TxoutType::WitnessUnknown(vec![vec![version as u8], program.to_vec()]);
        }

        return TxoutType::NonStandard;
    }

    // TODO: check IsPushOnly(script_pubkey[0])
    if script_pubkey.is_op_return() {
        return TxoutType::NullData;
    }

    if let Some(pubkey) = script_pubkey.p2pk_public_key() {
        return TxoutType::PubKey(pubkey);
    }

    if script_pubkey.is_p2pkh() {
        return TxoutType::PubKeyHash(script_pubkey.as_bytes()[3..23].try_into().unwrap());
    }

    if let Some((required_sigs, keys_count, keys)) = match_multisig(script_pubkey) {
        return TxoutType::Multisig {
            required_sigs,
            keys_count,
            keys,
        };
    }

    TxoutType::NonStandard
}

fn is_pay_to_anchor(script: &Script) -> bool {
    let script_bytes = script.as_bytes();
    script.len() == 4
        && script_bytes[0] == OP_PUSHNUM_1.to_u8()
        && script_bytes[1] == 0x02
        && script_bytes[2] == 0x4e
        && script_bytes[3] == 0x73
}

/// Checks whether a script pubkey is a bare multisig output.
///
/// In a bare multisig pubkey script the keys are not hashed, the script
/// is of the form:
///
///    `2 <pubkey1> <pubkey2> <pubkey3> 3 OP_CHECKMULTISIG`
#[inline]
fn match_multisig(script_pubkey: &Script) -> Option<(u8, u8, Vec<Vec<u8>>)> {
    let mut instructions = script_pubkey.instructions();

    let required_sigs = if let Ok(Instruction::Op(op)) = instructions.next()? {
        decode_pushnum(op)?
    } else {
        return None;
    };

    let mut keys = Vec::new();
    while let Some(Ok(instruction)) = instructions.next() {
        match instruction {
            Instruction::PushBytes(key) => {
                keys.push(key.as_bytes().to_vec());
            }
            Instruction::Op(op) => {
                if let Some(pushnum) = decode_pushnum(op) {
                    if pushnum as usize != keys.len() {
                        return None;
                    }
                }
                break;
            }
        }
    }

    let num_pubkeys: u8 = keys
        .len()
        .try_into()
        .expect("Must fit into u8 due to the MAX_PUBKEYS_PER_MULTISIG limit; qed");

    if required_sigs > num_pubkeys {
        return None;
    }

    if let Ok(Instruction::Op(op)) = instructions.next()? {
        if op != OP_CHECKMULTISIG {
            return None;
        }
    }

    if instructions.next().is_none() {
        Some((required_sigs, num_pubkeys, keys))
    } else {
        None
    }
}

fn decode_pushnum(opcode: Opcode) -> Option<u8> {
    if (OP_PUSHNUM_1.to_u8()..=OP_PUSHNUM_16.to_u8()).contains(&opcode.to_u8()) {
        Some(opcode.to_u8() - 0x50)
    } else {
        None
    }
}
