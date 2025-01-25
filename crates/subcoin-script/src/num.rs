//! Script numeric

use std::ops::{Add, Neg, Sub};

/// Script number error type.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum NumError {
    #[error("Script number overflow")]
    Overflow,
    #[error("Non-minimally encoded script number")]
    NotMinimallyEncoded,
}

/// A numeric type used in Bitcoin Script operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScriptNum {
    value: i64,
}

impl<T: Into<i64>> From<T> for ScriptNum {
    fn from(value: T) -> Self {
        Self {
            value: value.into(),
        }
    }
}

impl ScriptNum {
    /// Maximum script number length in bytes.
    pub const MAX_NUM_SIZE: usize = 4;

    /// Construct a [`ScriptNum`] with size validation.
    pub fn from_bytes(
        data: &[u8],
        require_minimal: bool,
        max_size: Option<usize>,
    ) -> Result<Self, NumError> {
        let max_size = max_size.unwrap_or(Self::MAX_NUM_SIZE);

        if data.len() > max_size {
            return Err(NumError::Overflow);
        }

        if data.is_empty() {
            return Ok(Self { value: 0 });
        }

        // Check for minimal encoding if required
        if require_minimal && !Self::is_minimally_encoded(data) {
            return Err(NumError::NotMinimallyEncoded);
        }

        let mut result = 0i64;

        // Parse bytes in little-endian order.
        for (i, &byte) in data.iter().enumerate() {
            result |= i64::from(byte).wrapping_shl(8 * i as u32);
        }

        // Check sign bit and extend if necessary.
        if data
            .last()
            .expect("Data must not be empty as checked above; qed")
            & 0x80
            != 0
        {
            let value = -(result & !(0x80i64.wrapping_shl((8 * (data.len() - 1)) as u32)));
            Ok(Self { value })
        } else {
            Ok(Self { value: result })
        }
    }

    /// Convert the number to a minimally encoded byte vector.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.value == 0 {
            return vec![];
        }

        let mut result = Vec::new();

        let mut abs_value = self.value.abs();

        while abs_value != 0 {
            result.push((abs_value & 0xff) as u8);
            abs_value >>= 8;
        }

        let negative = self.value < 0;

        // Handle sign bit
        if result.last().unwrap() & 0x80 != 0 {
            result.push(if negative { 0x80 } else { 0 });
        } else if negative {
            let len = result.len();
            result[len - 1] |= 0x80;
        }

        result
    }

    /// Check if the byte array is minimally encoded.
    fn is_minimally_encoded(data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }

        // Check leading zeros.
        if data[data.len() - 1] & 0x7f == 0 {
            if data.len() <= 1 || (data[data.len() - 2] & 0x80) == 0 {
                return false;
            }
        }

        true
    }

    /// Get the underlying value.
    pub fn value(&self) -> i64 {
        self.value
    }

    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    pub fn is_negative(&self) -> bool {
        self.value.is_negative()
    }

    pub fn abs(&self) -> Self {
        self.value.abs().into()
    }
}

impl Add for ScriptNum {
    type Output = Result<Self, NumError>;

    fn add(self, other: Self) -> Result<Self, NumError> {
        self.value
            .checked_add(other.value)
            .map(|value| Self { value })
            .ok_or(NumError::Overflow)
    }
}

impl Sub for ScriptNum {
    type Output = Result<Self, NumError>;

    fn sub(self, other: Self) -> Result<Self, NumError> {
        self.value
            .checked_sub(other.value)
            .map(|value| Self { value })
            .ok_or(NumError::Overflow)
    }
}

impl Neg for ScriptNum {
    type Output = Result<Self, NumError>;

    fn neg(self) -> Result<Self, NumError> {
        self.value
            .checked_neg()
            .map(|value| Self { value })
            .ok_or(NumError::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_num_arithmetic() {
        let a = ScriptNum::from(5);
        let b = ScriptNum::from(3);

        assert_eq!((a + b).unwrap().value(), 8);
        assert_eq!((a - b).unwrap().value(), 2);
        assert_eq!((-a).unwrap().value(), -5);
    }

    // Helper function to convert hex string to bytes
    fn hex_to_bytes(s: &str) -> Vec<u8> {
        hex::decode(s).expect("Invalid hex")
    }

    // Test for converting script numbers to byte representations
    #[test]
    fn test_script_num_to_bytes() {
        let tests = vec![
            (0i64, vec![]),
            (1, hex_to_bytes("01")),
            (-1, hex_to_bytes("81")),
            (127, hex_to_bytes("7f")),
            (-127, hex_to_bytes("ff")),
            (128, hex_to_bytes("8000")),
            (-128, hex_to_bytes("8080")),
            (129, hex_to_bytes("8100")),
            (-129, hex_to_bytes("8180")),
            (256, hex_to_bytes("0001")),
            (-256, hex_to_bytes("0081")),
            (32767, hex_to_bytes("ff7f")),
            (-32767, hex_to_bytes("ffff")),
            (32768, hex_to_bytes("008000")),
            (-32768, hex_to_bytes("008080")),
            (65535, hex_to_bytes("ffff00")),
            (-65535, hex_to_bytes("ffff80")),
            (524288, hex_to_bytes("000008")),
            (-524288, hex_to_bytes("000088")),
            (7340032, hex_to_bytes("000070")),
            (-7340032, hex_to_bytes("0000f0")),
            (8388608, hex_to_bytes("00008000")),
            (-8388608, hex_to_bytes("00008080")),
            (2147483647, hex_to_bytes("ffffff7f")),
            (-2147483647, hex_to_bytes("ffffffff")),
            (2147483648, hex_to_bytes("0000008000")),
            (-2147483648, hex_to_bytes("0000008080")),
            (2415919104, hex_to_bytes("0000009000")),
            (-2415919104, hex_to_bytes("0000009080")),
            (4294967295, hex_to_bytes("ffffffff00")),
            (-4294967295, hex_to_bytes("ffffffff80")),
            (4294967296, hex_to_bytes("0000000001")),
            (-4294967296, hex_to_bytes("0000000081")),
            (281474976710655, hex_to_bytes("ffffffffffff00")),
            (-281474976710655, hex_to_bytes("ffffffffffff80")),
            (72057594037927935, hex_to_bytes("ffffffffffffff00")),
            (-72057594037927935, hex_to_bytes("ffffffffffffff80")),
            (9223372036854775807, hex_to_bytes("ffffffffffffff7f")),
            (-9223372036854775807, hex_to_bytes("ffffffffffffffff")),
        ];

        for (num, expected) in tests {
            let got_bytes = ScriptNum::from(num).to_bytes();
            assert_eq!(
                got_bytes, expected,
                "Did not get expected bytes for {num}, got {got_bytes:?}, want {expected:?}",
            );
        }
    }

    // Test for converting byte representations to script numbers
    //
    // Copied from https://github.com/btcsuite/btcd/blob/ff2e03e11233fa25c01cf4acbf76501fc008b31f/txscript/scriptnum_test.go#L27
    #[test]
    fn test_script_num_from_bytes() {
        let tests = vec![
            ("80", Err(NumError::NotMinimallyEncoded), true, None),
            // Empty bytes.
            ("", Ok(0), true, None),
            ("01", Ok(1), true, None),
            ("81", Ok(-1), true, None),
            ("7f", Ok(127), true, None),
            ("ff", Ok(-127), true, None),
            ("8000", Ok(128), true, None),
            ("8080", Ok(-128), true, None),
            ("8100", Ok(129), true, None),
            ("8180", Ok(-129), true, None),
            ("0001", Ok(256), true, None),
            ("0081", Ok(-256), true, None),
            ("ff7f", Ok(32767), true, None),
            ("ffff", Ok(-32767), true, None),
            ("008000", Ok(32768), true, None),
            ("008080", Ok(-32768), true, None),
            ("ffff00", Ok(65535), true, None),
            ("ffff80", Ok(-65535), true, None),
            ("000008", Ok(524288), true, None),
            ("000088", Ok(-524288), true, None),
            ("000070", Ok(7340032), true, None),
            ("0000f0", Ok(-7340032), true, None),
            ("00008000", Ok(8388608), true, None),
            ("00008080", Ok(-8388608), true, None),
            ("ffffff7f", Ok(2147483647), true, None),
            ("ffffffff", Ok(-2147483647), true, None),
            ("ffffffff7f", Ok(549755813887), true, Some(5)),
            ("ffffffffff", Ok(-549755813887), true, Some(5)),
            ("ffffffffffffff7f", Ok(9223372036854775807), true, Some(8)),
            ("ffffffffffffffff", Ok(-9223372036854775807), true, Some(8)),
            //
            // In Go, shifting by an amount greater than or equal to the bit width of
            // the type results in zero for unsigned integers and retains the sign bit
            // for signed integers.
            //
            //  fn go_style_shift_left(value: i64, shift: u32) -> i64 {
            //      if shift >= 64 {
            //          // Mimic Go's behavior: return 0 for unsigned, handle sign for signed
            //          return 0;
            //      }
            //      value.wrapping_shl(shift)
            //  }
            //
            // The following test cases are disabled unless the related codes are updated:
            //
            // ```
            // let value = if data.len() >= 9 {
            //      -(result & !0)
            // } else {
            //      -(result & !(0x80i64.wrapping_shl((8 * (data.len() - 1)) as u32)))
            // };
            // ```
            // ("ffffffffffffffff7f", Ok(-1), true, Some(9)),
            // ("ffffffffffffffffff", Ok(1), true, Some(9)),
            // ("ffffffffffffffffff7f", Ok(-1), true, Some(10)),
            // ("ffffffffffffffffffff", Ok(1), true, Some(10)),
            ("0000008000", Err(NumError::Overflow), true, None),
            ("0000008080", Err(NumError::Overflow), true, None),
            ("0000009000", Err(NumError::Overflow), true, None),
            ("0000009080", Err(NumError::Overflow), true, None),
            ("ffffffff00", Err(NumError::Overflow), true, None),
            ("ffffffff80", Err(NumError::Overflow), true, None),
            ("0000000001", Err(NumError::Overflow), true, None),
            ("0000000081", Err(NumError::Overflow), true, None),
            ("ffffffffffff00", Err(NumError::Overflow), true, None),
            ("ffffffffffff80", Err(NumError::Overflow), true, None),
            ("ffffffffffffff00", Err(NumError::Overflow), true, None),
            ("ffffffffffffff80", Err(NumError::Overflow), true, None),
            ("ffffffffffffff7f", Err(NumError::Overflow), true, None),
            ("ffffffffffffffff", Err(NumError::Overflow), true, None),
            ("00", Err(NumError::NotMinimallyEncoded), true, None),
            ("0100", Err(NumError::NotMinimallyEncoded), true, None),
            ("7f00", Err(NumError::NotMinimallyEncoded), true, None),
            ("800000", Err(NumError::NotMinimallyEncoded), true, None),
            ("810000", Err(NumError::NotMinimallyEncoded), true, None),
            ("000100", Err(NumError::NotMinimallyEncoded), true, None),
            ("ff7f00", Err(NumError::NotMinimallyEncoded), true, None),
            ("00800000", Err(NumError::NotMinimallyEncoded), true, None),
            ("ffff0000", Err(NumError::NotMinimallyEncoded), true, None),
            ("00000800", Err(NumError::NotMinimallyEncoded), true, None),
            ("00007000", Err(NumError::NotMinimallyEncoded), true, None),
            (
                "0009000100",
                Err(NumError::NotMinimallyEncoded),
                true,
                Some(5),
            ),
            ("00", Ok(0), false, None),
            ("0100", Ok(1), false, None),
            ("7f00", Ok(127), false, None),
            ("800000", Ok(128), false, None),
            ("810000", Ok(129), false, None),
            ("000100", Ok(256), false, None),
            ("ff7f00", Ok(32767), false, None),
            ("00800000", Ok(32768), false, None),
            ("ffff0000", Ok(65535), false, None),
            ("00000800", Ok(524288), false, None),
            ("00007000", Ok(7340032), false, None),
            ("0009000100", Ok(16779520), false, Some(5)),
        ];

        for (serialized_in_hex, expected_result, minimal_encoding, max_size) in tests {
            let serialized = hex_to_bytes(serialized_in_hex);
            let result =
                ScriptNum::from_bytes(&serialized, minimal_encoding, max_size).map(|num| num.value);
            assert_eq!(
                result, expected_result,
                "Failed to convert bytes {serialized_in_hex} to ScriptNum, \
                got: {result:?}, expected {expected_result:?}"
            );
        }
    }
}
