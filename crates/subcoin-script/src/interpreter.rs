use keys::{Message, Signature, Public};
use crypto::{sha1, sha256, dhash160, dhash256, ripemd160};
use sign::{SignatureVersion, Sighash};
use ScriptWitness, Num, SignatureChecker;

use super::bytes::Bytes;
use bitcoin::blockdata::script::{Builder, Script};
use super::stack::Stack;
use super::flags::VerificationFlags;
use super::{MAX_SCRIPT_ELEMENT_SIZE, Error, SEQUENCE_LOCKTIME_DISABLE_FLAG, MAX_SCRIPT_ELEMENT_SIZE, MAX_SCRIPT_SIZE, MAX_OPS_PER_SCRIPT, MAX_PUBKEYS_PER_MULTISIG };
use bitcoin::Opcode;

/// Helper function.
fn check_signature(
	checker: &dyn SignatureChecker,
	script_sig: &Vec<u8>,
	public: &Vec<u8>,
	script_code: &Script,
	version: SignatureVersion
) -> bool {
	let public = match Public::from_slice(&public) {
		Ok(public) => public,
		_ => return false,
	};

	if let Some((hash_type, sig)) = script_sig.split_last() {
		checker.check_signature(&sig.into(), &public, script_code, *hash_type as u32, version)
	} else {
		return false
	}
}

/// Helper function.
fn verify_signature(
	checker: &dyn SignatureChecker,
	signature: Vec<u8>,
	public: Vec<u8>,
	message: Message,
) -> bool {
	let public = match Public::from_slice(&public) {
		Ok(public) => public,
		_ => return false,
	};

	if signature.is_empty() {
		return false;
	}

	checker.verify_signature(&signature.into(), &public, &message.into())
}

fn is_public_key(v: &[u8]) -> bool {
	match v.len() {
		33 if v[0] == 2 || v[0] == 3 => true,
		65 if v[0] == 4 => true,
		_ => false,
	}
}

/// A canonical signature exists of: <30> <total len> <02> <len R> <R> <02> <len S> <S> <hashtype>
/// Where R and S are not negative (their first byte has its highest bit not set), and not
/// excessively padded (do not start with a 0 byte, unless an otherwise negative number follows,
/// in which case a single 0 byte is necessary and even required).
///
/// See https://bitcointalk.org/index.php?topic=8392.msg127623#msg127623
///
/// This function is consensus-critical since BIP66.
fn is_valid_signature_encoding(sig: &[u8]) -> bool {
	// Format: 0x30 [total-length] 0x02 [R-length] [R] 0x02 [S-length] [S] [sighash]
	// * total-length: 1-byte length descriptor of everything that follows,
	//   excluding the sighash byte.
	// * R-length: 1-byte length descriptor of the R value that follows.
	// * R: arbitrary-length big-endian encoded R value. It must use the shortest
	//   possible encoding for a positive integers (which means no null bytes at
	//   the start, except a single one when the next byte has its highest bit set).
	// * S-length: 1-byte length descriptor of the S value that follows.
	// * S: arbitrary-length big-endian encoded S value. The same rules apply.
	// * sighash: 1-byte value indicating what data is hashed (not part of the DER
	//   signature)

	// Minimum and maximum size constraints
	if sig.len() < 9 || sig.len() > 73 {
		return false;
	}

	// A signature is of type 0x30 (compound)
	if sig[0] != 0x30 {
		return false;
	}

	// Make sure the length covers the entire signature.
	if sig[1] as usize != sig.len() - 3 {
		return false;
	}

	// Extract the length of the R element.
	let len_r = sig[3] as usize;

	// Make sure the length of the S element is still inside the signature.
	if len_r + 5 >= sig.len() {
		return false;
	}

	// Extract the length of the S element.
	let len_s = sig[len_r + 5] as usize;

	// Verify that the length of the signature matches the sum of the length
	if len_r + len_s + 7 != sig.len() {
		return false;
	}

	// Check whether the R element is an integer.
	if sig[2] != 2 {
		return false;
	}

	// Zero-length integers are not allowed for R.
	if len_r == 0 {
		return false;
	}

	// Negative numbers are not allowed for R.
	if (sig[4] & 0x80) != 0 {
		return false;
	}

	// Null bytes at the start of R are not allowed, unless R would
	// otherwise be interpreted as a negative number.
	if len_r > 1 && sig[4] == 0 && (sig[5] & 0x80) == 0 {
		return false;
	}

	// Check whether the S element is an integer.
	if sig[len_r + 4] != 2 {
		return false;
	}

	// Zero-length integers are not allowed for S.
	if len_s == 0 {
		return false;
	}

	// Negative numbers are not allowed for S.
	if (sig[len_r + 6] & 0x80) != 0 {
		return false;
	}

	// Null bytes at the start of S are not allowed, unless S would otherwise be
	// interpreted as a negative number.
	if len_s > 1 && (sig[len_r + 6] == 0) && (sig[len_r + 7] & 0x80) == 0 {
		return false;
	}

	true
}

fn is_low_der_signature(sig: &[u8]) -> Result<(), Error> {
	if !is_valid_signature_encoding(sig) {
		return Err(Error::SignatureDer);
	}

	let signature: Signature = sig.into();
	if !signature.check_low_s() {
		return Err(Error::SignatureHighS);
	}

	Ok(())
}

fn is_defined_hashtype_signature(version: SignatureVersion, sig: &[u8]) -> bool {
	if sig.is_empty() {
		return false;
	}

	Sighash::is_defined(version, sig[sig.len() - 1] as u32)
}

fn parse_hash_type(version: SignatureVersion, sig: &[u8]) -> Sighash {
	Sighash::from_u32(version, if sig.is_empty() { 0 } else { sig[sig.len() - 1] as u32 })
}

fn check_signature_encoding(sig: &[u8], flags: &VerificationFlags, version: SignatureVersion) -> Result<(), Error> {
	// Empty signature. Not strictly DER encoded, but allowed to provide a
	// compact way to provide an invalid signature for use with CHECK(MULTI)SIG

	if sig.is_empty() {
		return Ok(());
	}

	if (flags.verify_dersig || flags.verify_low_s || flags.verify_strictenc) && !is_valid_signature_encoding(sig) {
		return Err(Error::SignatureDer);
	}

	if flags.verify_low_s {
		is_low_der_signature(sig)?;
	}

	if flags.verify_strictenc && !is_defined_hashtype_signature(version, sig) {
		return Err(Error::SignatureHashtype)
	}

	// verify_strictenc is currently enabled for BitcoinCash only
	if flags.verify_strictenc {
		let uses_fork_id = parse_hash_type(version, sig).fork_id;
		let enabled_fork_id = version == SignatureVersion::ForkId;
		if uses_fork_id && !enabled_fork_id {
			return Err(Error::SignatureIllegalForkId)
		} else if !uses_fork_id && enabled_fork_id {
			return Err(Error::SignatureMustUseForkId);
		}
	}

	Ok(())
}

fn check_pubkey_encoding(v: &[u8], flags: &VerificationFlags) -> Result<(), Error> {
	if flags.verify_strictenc && !is_public_key(v) {
		return Err(Error::PubkeyType);
	}

	Ok(())
}

fn check_minimal_push(data: &[u8], opcode: Opcode) -> bool {
	if data.is_empty() {
		// Could have used OP_0.
		opcode == Opcode::OP_0
	} else if data.len() == 1 && data[0] >= 1 && data[0] <= 16 {
		// Could have used OP_1 .. OP_16.
		opcode as u8 == Opcode::OP_1 as u8 + (data[0] - 1)
	} else if data.len() == 1 && data[0] == 0x81 {
		// Could have used OP_1NEGATE
		opcode == Opcode::OP_1NEGATE
	} else if data.len() <= 75 {
		// Could have used a direct push (opcode indicating number of bytes pushed + those bytes).
		opcode as usize == data.len()
	} else if data.len() <= 255 {
		// Could have used OP_PUSHDATA.
		opcode == Opcode::OP_PUSHDATA1
	} else if data.len() <= 65535 {
		// Could have used OP_PUSHDATA2.
		opcode == Opcode::OP_PUSHDATA2
	} else {
		true
	}
}

fn cast_to_bool(data: &[u8]) -> bool {
	if data.is_empty() {
		return false;
	}

	if data[..data.len() - 1].iter().any(|x| x != &0) {
		return true;
	}

	let last = data[data.len() - 1];
	!(last == 0 || last == 0x80)
}

/// Verifies script signature and pubkey
pub fn verify_script(
	script_sig: &Script,
	script_pubkey: &Script,
	witness: &ScriptWitness,
	flags: &VerificationFlags,
	checker: &dyn SignatureChecker,
	version: SignatureVersion,
) -> Result<(), Error> {
	if flags.verify_sigpushonly && !script_sig.is_push_only() {
		return Err(Error::SignaturePushOnly);
	}

	let mut stack = Stack::new();
	let mut stack_copy = Stack::new();
	let mut had_witness = false;

	eval_script(&mut stack, script_sig, flags, checker, version)?;

	if flags.verify_p2sh {
		stack_copy = stack.clone();
	}

	let res = eval_script(&mut stack, script_pubkey, flags, checker, version)?;
	if !res {
		return Err(Error::EvalFalse);
	}

	// Verify witness program
	let mut verify_cleanstack = flags.verify_cleanstack;
	if flags.verify_witness {
		if let Some((witness_version, witness_program)) = script_pubkey.parse_witness_program() {
			if !script_sig.is_empty() {
				return Err(Error::WitnessMalleated);
			}

			had_witness = true;
			verify_cleanstack = false;
			if !verify_witness_program(witness, witness_version, witness_program, flags, checker)? {
				return Err(Error::EvalFalse);
			}
		}
	}

	// Additional validation for spend-to-script-hash transactions:
	if flags.verify_p2sh && script_pubkey.is_pay_to_script_hash() {
		if !script_sig.is_push_only() {
			return Err(Error::SignaturePushOnly);
		}

		std::mem::swap(&mut stack, &mut stack_copy);

		// stack cannot be empty here, because if it was the
		// P2SH  HASH <> EQUAL  scriptPubKey would be evaluated with
		// an empty stack and the EvalScript above would return false.
		assert!(!stack.is_empty());

		let pubkey2: Script = stack.pop()?.into();

		let res = eval_script(&mut stack, &pubkey2, flags, checker, version)?;
		if !res {
			return Err(Error::EvalFalse);
		}

		if flags.verify_witness {
			if let Some((witness_version, witness_program)) = pubkey2.parse_witness_program() {
				if script_sig != &Builder::default().push_slice(&pubkey2).into_script() {
					return Err(Error::WitnessMalleatedP2SH);
				}

				had_witness = true;
				verify_cleanstack = false;
				if !verify_witness_program(witness, witness_version, witness_program, flags, checker)? {
					return Err(Error::EvalFalse);
				}
			}
		}
	}

    // The CLEANSTACK check is only performed after potential P2SH evaluation,
    // as the non-P2SH evaluation of a P2SH script will obviously not result in
    // a clean stack (the P2SH inputs remain). The same holds for witness evaluation.
	if verify_cleanstack {
        // Disallow CLEANSTACK without P2SH, as otherwise a switch CLEANSTACK->P2SH+CLEANSTACK
        // would be possible, which is not a softfork (and P2SH should be one).
		assert!(flags.verify_p2sh);
		if stack.len() != 1 {
			return Err(Error::Cleanstack);
		}
	}

	if flags.verify_witness {
		// We can't check for correct unexpected witness data if P2SH was off, so require
		// that WITNESS implies P2SH. Otherwise, going from WITNESS->P2SH+WITNESS would be
		// possible, which is not a softfork.
		assert!(flags.verify_p2sh);
		if !had_witness && !witness.is_empty() {
			return Err(Error::WitnessUnexpected);
		}
	}

	Ok(())
}

fn verify_witness_program(
	witness: &ScriptWitness,
	witness_version: u8,
	witness_program: &[u8],
	flags: &VerificationFlags,
	checker: &dyn SignatureChecker,
) -> Result<bool, Error> {
	if witness_version != 0 {
		if flags.verify_discourage_upgradable_witness_program {
			return Err(Error::DiscourageUpgradableWitnessProgram);
		}

		return Ok(true);
	}

	let witness_stack = witness;
	let witness_stack_len = witness_stack.len();
	let (mut stack, script_pubkey): (Stack<_>, Script) = match witness_program.len() {
		32 => {
			if witness_stack_len == 0 {
				return Err(Error::WitnessProgramWitnessEmpty);
			}

			let script_pubkey = &witness_stack[witness_stack_len - 1];
			let stack = &witness_stack[0..witness_stack_len - 1];
			let script_pubkey_hash = sha256(script_pubkey);

			if script_pubkey_hash != witness_program[0..32].into() {
				return Err(Error::WitnessProgramMismatch);
			}

			(stack.iter().cloned().collect::<Vec<_>>().into(), Script::new(script_pubkey.clone()))
		},
		20 => {
			if witness_stack_len != 2 {
				return Err(Error::WitnessProgramMismatch);
			}

			let script_pubkey = Builder::default()
				.push_opcode(Opcode::OP_DUP)
				.push_opcode(Opcode::OP_HASH160)
				.push_slice(witness_program)
				.push_opcode(Opcode::OP_EQUALVERIFY)
				.push_opcode(Opcode::OP_CHECKSIG)
				.into_script();

			(witness_stack.clone().into(), script_pubkey)
		},
		_ => return Err(Error::WitnessProgramWrongLength),
	};

	if stack.iter().any(|s| s.len() > MAX_SCRIPT_ELEMENT_SIZE) {
		return Err(Error::PushSize);
	}

	if !eval_script(&mut stack, &script_pubkey, flags, checker, SignatureVersion::WitnessV0)? {
		return Ok(false);
	}

	if stack.len() != 1 {
		return Err(Error::EvalFalse);
	}

	let success = cast_to_bool(stack.last().expect("stack.len() == 1; last() only returns errors when stack is empty; qed"));
	Ok(success)
}

/// Evaluautes the script
#[cfg_attr(feature="cargo-clippy", allow(match_same_arms))]
pub fn eval_script(
	stack: &mut Stack<Bytes>,
	script: &Script,
	flags: &VerificationFlags,
	checker: &dyn SignatureChecker,
	version: SignatureVersion
) -> Result<bool, Error> {
	if script.len() > MAX_SCRIPT_SIZE {
		return Err(Error::ScriptSize);
	}

	let mut pc = 0;
	let mut op_count = 0;
	let mut begincode = 0;
	let mut exec_stack = Vec::<bool>::new();
	let mut altstack = Stack::<Bytes>::new();

	while pc < script.len() {
		let executing = exec_stack.iter().all(|x| *x);
		let instruction = match script.get_instruction(pc) {
			Ok(i) => i,
			Err(Error::BadOpcode) if !executing => {
				pc += 1;
				continue;
			},
			Err(err) => return Err(err),
		};
		let opcode = instruction.opcode;

		if let Some(data) = instruction.data {
			if data.len() > MAX_SCRIPT_ELEMENT_SIZE {
				return Err(Error::PushSize);
			}

			if executing && flags.verify_minimaldata && !check_minimal_push(data, opcode) {
				return Err(Error::Minimaldata);
			}
		}

		if opcode.is_countable() {
			op_count += 1;
			if op_count > MAX_OPS_PER_SCRIPT {
				return Err(Error::OpCount);
			}
		}

		if opcode.is_disabled(flags) {
			return Err(Error::DisabledOpcode(opcode));
		}

		pc += instruction.step;
		if !(executing || (Opcode::OP_IF <= opcode && opcode <= Opcode::OP_ENDIF)) {
			continue;
		}

		match opcode {
			Opcode::OP_PUSHDATA1 |
			Opcode::OP_PUSHDATA2 |
			Opcode::OP_PUSHDATA4 |
			Opcode::OP_0 |
			Opcode::OP_PUSHBYTES_1 |
			Opcode::OP_PUSHBYTES_2 |
			Opcode::OP_PUSHBYTES_3 |
			Opcode::OP_PUSHBYTES_4 |
			Opcode::OP_PUSHBYTES_5 |
			Opcode::OP_PUSHBYTES_6 |
			Opcode::OP_PUSHBYTES_7 |
			Opcode::OP_PUSHBYTES_8 |
			Opcode::OP_PUSHBYTES_9 |
			Opcode::OP_PUSHBYTES_10 |
			Opcode::OP_PUSHBYTES_11 |
			Opcode::OP_PUSHBYTES_12 |
			Opcode::OP_PUSHBYTES_13 |
			Opcode::OP_PUSHBYTES_14 |
			Opcode::OP_PUSHBYTES_15 |
			Opcode::OP_PUSHBYTES_16 |
			Opcode::OP_PUSHBYTES_17 |
			Opcode::OP_PUSHBYTES_18 |
			Opcode::OP_PUSHBYTES_19 |
			Opcode::OP_PUSHBYTES_20 |
			Opcode::OP_PUSHBYTES_21 |
			Opcode::OP_PUSHBYTES_22 |
			Opcode::OP_PUSHBYTES_23 |
			Opcode::OP_PUSHBYTES_24 |
			Opcode::OP_PUSHBYTES_25 |
			Opcode::OP_PUSHBYTES_26 |
			Opcode::OP_PUSHBYTES_27 |
			Opcode::OP_PUSHBYTES_28 |
			Opcode::OP_PUSHBYTES_29 |
			Opcode::OP_PUSHBYTES_30 |
			Opcode::OP_PUSHBYTES_31 |
			Opcode::OP_PUSHBYTES_32 |
			Opcode::OP_PUSHBYTES_33 |
			Opcode::OP_PUSHBYTES_34 |
			Opcode::OP_PUSHBYTES_35 |
			Opcode::OP_PUSHBYTES_36 |
			Opcode::OP_PUSHBYTES_37 |
			Opcode::OP_PUSHBYTES_38 |
			Opcode::OP_PUSHBYTES_39 |
			Opcode::OP_PUSHBYTES_40 |
			Opcode::OP_PUSHBYTES_41 |
			Opcode::OP_PUSHBYTES_42 |
			Opcode::OP_PUSHBYTES_43 |
			Opcode::OP_PUSHBYTES_44 |
			Opcode::OP_PUSHBYTES_45 |
			Opcode::OP_PUSHBYTES_46 |
			Opcode::OP_PUSHBYTES_47 |
			Opcode::OP_PUSHBYTES_48 |
			Opcode::OP_PUSHBYTES_49 |
			Opcode::OP_PUSHBYTES_50 |
			Opcode::OP_PUSHBYTES_51 |
			Opcode::OP_PUSHBYTES_52 |
			Opcode::OP_PUSHBYTES_53 |
			Opcode::OP_PUSHBYTES_54 |
			Opcode::OP_PUSHBYTES_55 |
			Opcode::OP_PUSHBYTES_56 |
			Opcode::OP_PUSHBYTES_57 |
			Opcode::OP_PUSHBYTES_58 |
			Opcode::OP_PUSHBYTES_59 |
			Opcode::OP_PUSHBYTES_60 |
			Opcode::OP_PUSHBYTES_61 |
			Opcode::OP_PUSHBYTES_62 |
			Opcode::OP_PUSHBYTES_63 |
			Opcode::OP_PUSHBYTES_64 |
			Opcode::OP_PUSHBYTES_65 |
			Opcode::OP_PUSHBYTES_66 |
			Opcode::OP_PUSHBYTES_67 |
			Opcode::OP_PUSHBYTES_68 |
			Opcode::OP_PUSHBYTES_69 |
			Opcode::OP_PUSHBYTES_70 |
			Opcode::OP_PUSHBYTES_71 |
			Opcode::OP_PUSHBYTES_72 |
			Opcode::OP_PUSHBYTES_73 |
			Opcode::OP_PUSHBYTES_74 |
			Opcode::OP_PUSHBYTES_75 => {
				if let Some(data) = instruction.data {
					stack.push(data.to_vec().into());
				}
			},
			Opcode::OP_1NEGATE |
			Opcode::OP_1 |
			Opcode::OP_2 |
			Opcode::OP_3 |
			Opcode::OP_4 |
			Opcode::OP_5 |
			Opcode::OP_6 |
			Opcode::OP_7 |
			Opcode::OP_8 |
			Opcode::OP_9 |
			Opcode::OP_10 |
			Opcode::OP_11 |
			Opcode::OP_12 |
			Opcode::OP_13 |
			Opcode::OP_14 |
			Opcode::OP_15 |
			Opcode::OP_16 => {
				let value = (opcode as i32).wrapping_sub(Opcode::OP_1 as i32 - 1);
				stack.push(Num::from(value).to_bytes());
			},
			Opcode::OP_CAT if flags.verify_concat => {
				let mut value_to_append = stack.pop()?;
				let value_to_update = stack.last_mut()?;
				if value_to_update.len() + value_to_append.len() > MAX_SCRIPT_ELEMENT_SIZE {
					return Err(Error::PushSize);
				}
				value_to_update.append(&mut value_to_append);
			},
			// OP_SPLIT replaces OP_SUBSTR
			Opcode::OP_SUBSTR if flags.verify_split => {
				let n = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if n.is_negative() {
					return Err(Error::InvalidStackOperation);
				}
				let n: usize = n.into();
				let splitted_value = {
					let value_to_split = stack.last_mut()?;
					if n > value_to_split.len() {
						return Err(Error::InvalidSplitRange);
					}
					value_to_split.split_off(n)
				};
				stack.push(splitted_value);
			},
			Opcode::OP_AND if flags.verify_and => {
				let mask = stack.pop()?;
				let mask_len = mask.len();
				let value_to_update = stack.last_mut()?;
				if mask_len != value_to_update.len() {
					return Err(Error::InvalidOperandSize);
				}
				for (byte_to_update, byte_mask) in (*value_to_update).iter_mut().zip(mask.iter()) {
					*byte_to_update = *byte_to_update & byte_mask;
				}
			},
			Opcode::OP_OR if flags.verify_or => {
				let mask = stack.pop()?;
				let mask_len = mask.len();
				let value_to_update = stack.last_mut()?;
				if mask_len != value_to_update.len() {
					return Err(Error::InvalidOperandSize);
				}
				for (byte_to_update, byte_mask) in (*value_to_update).iter_mut().zip(mask.iter()) {
					*byte_to_update = *byte_to_update | byte_mask;
				}
			},
			Opcode::OP_XOR if flags.verify_xor => {
				let mask = stack.pop()?;
				let mask_len = mask.len();
				let value_to_update = stack.last_mut()?;
				if mask_len != value_to_update.len() {
					return Err(Error::InvalidOperandSize);
				}
				for (byte_to_update, byte_mask) in (*value_to_update).iter_mut().zip(mask.iter()) {
					*byte_to_update = *byte_to_update ^ byte_mask;
				}
			},
			Opcode::OP_DIV if flags.verify_div => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if v2.is_zero() {
					return Err(Error::DivisionByZero);
				}
				stack.push((v1 / v2).to_bytes());
			},
			Opcode::OP_MOD if flags.verify_mod => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if v2.is_zero() {
					return Err(Error::DivisionByZero);
				}
				stack.push((v1 % v2).to_bytes());
			},
			// OP_BIN2NUM replaces OP_RIGHT
			Opcode::OP_RIGHT if flags.verify_bin2num => {
				let bin = stack.pop()?;
				let n = Num::minimally_encode(&bin, 4)?;
				stack.push(n.to_bytes());
			},
			// OP_NUM2BIN replaces OP_LEFT
			Opcode::OP_LEFT if flags.verify_num2bin => {
				let bin_size = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if bin_size.is_negative() || bin_size > MAX_SCRIPT_ELEMENT_SIZE.into() {
					return Err(Error::PushSize);
				}

				let bin_size: usize = bin_size.into();
				let num = Num::minimally_encode(&stack.pop()?, 4)?;
				let mut num = num.to_bytes();

				// check if we can fit number into array of bin_size length
				if num.len() > bin_size {
					return Err(Error::ImpossibleEncoding);
				}

				// check if we need to extend binary repr with zero-bytes
				if num.len() < bin_size {
					let sign_byte = num.last_mut().map(|last_byte| {
						let sign_byte = *last_byte & 0x80;
						*last_byte = *last_byte & 0x7f;
						sign_byte
					}).unwrap_or(0x00);

					num.resize(bin_size - 1, 0x00);
					num.push(sign_byte);
				}

				stack.push(num);
			},
			Opcode::OP_CAT | Opcode::OP_SUBSTR | Opcode::OP_LEFT | Opcode::OP_RIGHT |
			Opcode::OP_INVERT | Opcode::OP_AND | Opcode::OP_OR | Opcode::OP_XOR |
			Opcode::OP_2MUL | Opcode::OP_2DIV | Opcode::OP_MUL | Opcode::OP_DIV |
			Opcode::OP_MOD | Opcode::OP_LSHIFT | Opcode::OP_RSHIFT => {
				return Err(Error::DisabledOpcode(opcode));
			},
			Opcode::OP_NOP => (),
			Opcode::OP_CHECKLOCKTIMEVERIFY => {
				if flags.verify_locktime {
					// Note that elsewhere numeric opcodes are limited to
					// operands in the range -2**31+1 to 2**31-1, however it is
					// legal for opcodes to produce results exceeding that
					// range. This limitation is implemented by CScriptNum's
					// default 4-byte limit.
					//
					// If we kept to that limit we'd have a year 2038 problem,
					// even though the nLockTime field in transactions
					// themselves is uint32 which only becomes meaningless
					// after the year 2106.
					//
					// Thus as a special case we tell CScriptNum to accept up
					// to 5-byte bignums, which are good until 2**39-1, well
					// beyond the 2**32-1 limit of the nLockTime field itself.
					let lock_time = Num::from_slice(stack.last()?, flags.verify_minimaldata, 5)?;

					// In the rare event that the argument may be < 0 due to
					// some arithmetic being done first, you can always use
					// 0 MAX CHECKLOCKTIMEVERIFY.
					if lock_time.is_negative() {
						return Err(Error::NegativeLocktime);
					}

					if !checker.check_lock_time(lock_time) {
						return Err(Error::UnsatisfiedLocktime);
					}
				} else if flags.verify_discourage_upgradable_nops {
					return Err(Error::DiscourageUpgradableNops);
				}
			},
			Opcode::OP_CHECKSEQUENCEVERIFY => {
				if flags.verify_checksequence {
					let sequence = Num::from_slice(stack.last()?, flags.verify_minimaldata, 5)?;

					if sequence.is_negative() {
						return Err(Error::NegativeLocktime);
					}

					if (sequence & (SEQUENCE_LOCKTIME_DISABLE_FLAG as i64).into()).is_zero() && !checker.check_sequence(sequence) {
						return Err(Error::UnsatisfiedLocktime);
					}

				} else if flags.verify_discourage_upgradable_nops {
					return Err(Error::DiscourageUpgradableNops);
				}
			},
			Opcode::OP_NOP1 |
			Opcode::OP_NOP4 |
			Opcode::OP_NOP5 |
			Opcode::OP_NOP6 |
			Opcode::OP_NOP7 |
			Opcode::OP_NOP8 |
			Opcode::OP_NOP9 |
			Opcode::OP_NOP10 => {
				if flags.verify_discourage_upgradable_nops {
					return Err(Error::DiscourageUpgradableNops);
				}
			},
			Opcode::OP_IF | Opcode::OP_NOTIF => {
				let mut exec_value = false;
				if executing {
					exec_value = cast_to_bool(&stack.pop().map_err(|_| Error::UnbalancedConditional)?);
					if opcode == Opcode::OP_NOTIF {
						exec_value = !exec_value;
					}
				}
				exec_stack.push(exec_value);
			},
			Opcode::OP_ELSE => {
				if exec_stack.is_empty() {
					return Err(Error::UnbalancedConditional);
				}
				let last_index = exec_stack.len() - 1;
				let last = exec_stack[last_index];
				exec_stack[last_index] = !last;
			},
			Opcode::OP_ENDIF => {
				if exec_stack.is_empty() {
					return Err(Error::UnbalancedConditional);
				}
				exec_stack.pop();
			},
			Opcode::OP_VERIFY => {
				let exec_value = cast_to_bool(&stack.pop()?);
				if !exec_value {
					return Err(Error::Verify);
				}
			},
			Opcode::OP_RETURN => {
				return Err(Error::ReturnOpcode);
			},
			Opcode::OP_TOALTSTACK => {
				altstack.push(stack.pop()?);
			},
			Opcode::OP_FROMALTSTACK => {
				stack.push(altstack.pop().map_err(|_| Error::InvalidAltstackOperation)?);
			},
			Opcode::OP_2DROP => {
				stack.drop(2)?;
			},
			Opcode::OP_2DUP => {
				stack.dup(2)?;
			},
			Opcode::OP_3DUP => {
				stack.dup(3)?;
			},
			Opcode::OP_2OVER => {
				stack.over(2)?;
			},
			Opcode::OP_2ROT => {
				stack.rot(2)?;
			},
			Opcode::OP_2SWAP => {
				stack.swap(2)?;
			},
			Opcode::OP_IFDUP => {
				if cast_to_bool(stack.last()?) {
					stack.dup(1)?;
				}
			},
			Opcode::OP_DEPTH => {
				let depth = Num::from(stack.len());
				stack.push(depth.to_bytes());
			},
			Opcode::OP_DROP => {
				stack.pop()?;
			},
			Opcode::OP_DUP => {
				stack.dup(1)?;
			},
			Opcode::OP_NIP => {
				stack.nip()?;
			},
			Opcode::OP_OVER => {
				stack.over(1)?;
			},
			Opcode::OP_PICK | Opcode::OP_ROLL => {
				let n: i64 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?.into();
				if n < 0 || n >= stack.len() as i64 {
					return Err(Error::InvalidStackOperation);
				}

				let v = match opcode {
					Opcode::OP_PICK => stack.top(n as usize)?.clone(),
					_ => stack.remove(n as usize)?,
				};

				stack.push(v);
			},
			Opcode::OP_ROT => {
				stack.rot(1)?;
			},
			Opcode::OP_SWAP => {
				stack.swap(1)?;
			},
			Opcode::OP_TUCK => {
				stack.tuck()?;
			},
			Opcode::OP_SIZE => {
				let n = Num::from(stack.last()?.len());
				stack.push(n.to_bytes());
			},
			Opcode::OP_EQUAL => {
				let v1 = stack.pop()?;
				let v2 = stack.pop()?;
				if v1 == v2 {
					stack.push(vec![1].into());
				} else {
					stack.push(Bytes::new());
				}
			},
			Opcode::OP_EQUALVERIFY => {
				let equal = stack.pop()? == stack.pop()?;
				if !equal {
					return Err(Error::EqualVerify);
				}
			},
			Opcode::OP_1ADD => {
				let n = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)? + 1.into();
				stack.push(n.to_bytes());
			},
			Opcode::OP_1SUB => {
				let n = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)? - 1.into();
				stack.push(n.to_bytes());
			},
			Opcode::OP_NEGATE => {
				let n = -Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				stack.push(n.to_bytes());
			},
			Opcode::OP_ABS => {
				let n = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?.abs();
				stack.push(n.to_bytes());
			},
			Opcode::OP_NOT => {
				let n = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?.is_zero();
				let n = Num::from(n);
				stack.push(n.to_bytes());
			},
			Opcode::OP_0NOTEQUAL => {
				let n = !Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?.is_zero();
				let n = Num::from(n);
				stack.push(n.to_bytes());
			},
			Opcode::OP_ADD => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				stack.push((v1 + v2).to_bytes());
			},
			Opcode::OP_SUB => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				stack.push((v2 - v1).to_bytes());
			},
			Opcode::OP_BOOLAND => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(!v1.is_zero() && !v2.is_zero());
				stack.push(v.to_bytes());
			},
			Opcode::OP_BOOLOR => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(!v1.is_zero() || !v2.is_zero());
				stack.push(v.to_bytes());
			},
			Opcode::OP_NUMEQUAL => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 == v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_NUMEQUALVERIFY => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if v1 != v2 {
					return Err(Error::NumEqualVerify);
				}
			},
			Opcode::OP_NUMNOTEQUAL => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 != v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_LESSTHAN => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 > v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_GREATERTHAN => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 < v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_LESSTHANOREQUAL => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 >= v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_GREATERTHANOREQUAL => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v = Num::from(v1 <= v2);
				stack.push(v.to_bytes());
			},
			Opcode::OP_MIN => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				stack.push(v1.min(v2).to_bytes());
			},
			Opcode::OP_MAX => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				stack.push(v1.max(v2).to_bytes());
			},
			Opcode::OP_WITHIN => {
				let v1 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v2 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				let v3 = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if v2 <= v3 && v3 < v1 {
					stack.push(vec![1].into());
				} else {
					stack.push(Bytes::new());
				}
			},
			Opcode::OP_RIPEMD160 => {
				let v = ripemd160(&stack.pop()?);
				stack.push(v.to_vec().into());
			},
			Opcode::OP_SHA1 => {
				let v = sha1(&stack.pop()?);
				stack.push(v.to_vec().into());
			},
			Opcode::OP_SHA256 => {
				let v = sha256(&stack.pop()?);
				stack.push(v.to_vec().into());
			},
			Opcode::OP_HASH160 => {
				let v = dhash160(&stack.pop()?);
				stack.push(v.to_vec().into());
			},
			Opcode::OP_HASH256 => {
				let v = dhash256(&stack.pop()?);
				stack.push(v.to_vec().into());
			},
			Opcode::OP_CODESEPARATOR => {
				begincode = pc;
			},
			Opcode::OP_CHECKSIG | Opcode::OP_CHECKSIGVERIFY => {
				let pubkey = stack.pop()?;
				let signature = stack.pop()?;
				let sighash = parse_hash_type(version, &signature);
				let mut subscript = script.subscript(begincode);
				match version {
					SignatureVersion::ForkId if sighash.fork_id => (),
					SignatureVersion::WitnessV0 => (),
					SignatureVersion::Base | SignatureVersion::ForkId => {
						let signature_script = Builder::default().push_slice(&*signature).into_script();
						subscript = subscript.find_and_delete(&*signature_script);
					},
				}

				check_signature_encoding(&signature, flags, version)?;
				check_pubkey_encoding(&pubkey, flags)?;

				let success = check_signature(checker, &signature, &pubkey, &subscript, version);
				match opcode {
					Opcode::OP_CHECKSIG => {
						if success {
							stack.push(vec![1].into());
						} else {
							stack.push(Bytes::new());
						}
					},
					Opcode::OP_CHECKSIGVERIFY if !success => {
						return Err(Error::CheckSigVerify);
					},
					_ => {},
				}
			},
			Opcode::OP_CHECKMULTISIG | Opcode::OP_CHECKMULTISIGVERIFY => {
				let keys_count = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if keys_count < 0.into() || keys_count > MAX_PUBKEYS_PER_MULTISIG.into() {
					return Err(Error::PubkeyCount);
				}

				let keys_count: usize = keys_count.into();
				let keys = (0..keys_count).into_iter().map(|_| stack.pop()).collect::<Result<Vec<_>, _>>()?;

				let sigs_count = Num::from_slice(&stack.pop()?, flags.verify_minimaldata, 4)?;
				if sigs_count < 0.into() || sigs_count > keys_count.into() {
					return Err(Error::SigCount);
				}

				let sigs_count: usize = sigs_count.into();
				let sigs = (0..sigs_count).into_iter().map(|_| stack.pop()).collect::<Result<Vec<_>, _>>()?;

				let mut subscript = script.subscript(begincode);

				for signature in &sigs {
					let sighash = parse_hash_type(version, &signature);
					match version {
						SignatureVersion::ForkId if sighash.fork_id => (),
						SignatureVersion::WitnessV0 => (),
						SignatureVersion::Base | SignatureVersion::ForkId => {
							let signature_script = Builder::default().push_slice(&*signature).into_script();
							subscript = subscript.find_and_delete(&*signature_script);
						},
					}
				}

				let mut success = true;
				let mut k = 0;
				let mut s = 0;
				while s < sigs.len() && success {
					let key = &keys[k];
					let sig = &sigs[s];

					check_signature_encoding(sig, flags, version)?;
					check_pubkey_encoding(key, flags)?;

					let ok = check_signature(checker, sig, key, &subscript, version);
					if ok {
						s += 1;
					}
					k += 1;

					success = sigs.len() - s <= keys.len() - k;
				}

				if !stack.pop()?.is_empty() && flags.verify_nulldummy {
					return Err(Error::SignatureNullDummy);
				}

				match opcode {
					Opcode::OP_CHECKMULTISIG => {
						if success {
							stack.push(vec![1].into());
						} else {
							stack.push(Bytes::new());
						}
					},
					Opcode::OP_CHECKMULTISIGVERIFY if !success => {
						return Err(Error::CheckSigVerify);
					},
					_ => {},
				}
			},
			Opcode::OP_RESERVED |
			Opcode::OP_VER |
			Opcode::OP_RESERVED1 |
			Opcode::OP_RESERVED2 => {
				if executing {
					return Err(Error::DisabledOpcode(opcode));
				}
			},
			Opcode::OP_VERIF |
			Opcode::OP_VERNOTIF => {
				return Err(Error::DisabledOpcode(opcode));
			},
			Opcode::OP_CHECKDATASIG | Opcode::OP_CHECKDATASIGVERIFY if flags.verify_checkdatasig => {
				let pubkey = stack.pop()?;
				let message = stack.pop()?;
				let signature = stack.pop()?;

				check_signature_encoding(&signature, flags, version)?;
				check_pubkey_encoding(&pubkey, flags)?;

				let signature: Vec<u8> = signature.into();
				let message_hash = sha256(&message);
				let success = verify_signature(checker, signature.into(), pubkey.into(), message_hash);
				match opcode {
					Opcode::OP_CHECKDATASIG => {
						if success {
							stack.push(vec![1].into());
						} else {
							stack.push(Bytes::new());
						}
					},
					Opcode::OP_CHECKDATASIGVERIFY if !success => {
						return Err(Error::CheckDataSigVerify);
					},
					_ => {},
				}
			},
			Opcode::OP_CHECKDATASIG | Opcode::OP_CHECKDATASIGVERIFY => {
				return Err(Error::DisabledOpcode(opcode));
			},
		}

		if stack.len() + altstack.len() > 1000 {
			return Err(Error::StackSize);
		}
	}

	if !exec_stack.is_empty() {
		return Err(Error::UnbalancedConditional);
	}

	let success = !stack.is_empty() && {
		let last = stack.last()?;
		cast_to_bool(last)
	};

	Ok(success)
}
