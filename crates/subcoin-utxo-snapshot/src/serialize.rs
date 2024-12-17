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
