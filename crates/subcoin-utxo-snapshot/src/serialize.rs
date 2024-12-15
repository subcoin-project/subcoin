use std::io::{self, Write};

// https://github.com/bitcoin/bitcoin/blob/0903ce8dbc25d3823b03d52f6e6bff74d19e801e/src/serialize.h#L305
pub fn write_compact_size<W: Write>(writer: &mut W, size: u64) -> io::Result<()> {
    if size < 253 {
        writer.write_all(&[size as u8])?;
    } else if size <= 0xFFFF {
        writer.write_all(&[253])?;
        writer.write_all(&(size as u16).to_le_bytes())?;
    } else if size <= 0xFFFF_FFFF {
        writer.write_all(&[254])?;
        writer.write_all(&(size as u32).to_le_bytes())?;
    } else {
        writer.write_all(&[255])?;
        writer.write_all(&size.to_le_bytes())?;
    }
    Ok(())
}

pub struct VarInt(pub u64);

impl From<u64> for VarInt {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl VarInt {
    pub fn serialize<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut value = self.0;
        while value >= 0x80 {
            writer.write_all(&[((value & 0x7F) | 0x80) as u8])?;
            value >>= 7;
        }
        writer.write_all(&[value as u8])?;
        Ok(())
    }
}
