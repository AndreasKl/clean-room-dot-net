//! Little-endian byte cursors and ECMA-335 compressed integers (§II.23.2).

use crate::{Error, Result};

pub fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

/// Encoded size in bytes of an unsigned compressed integer (§II.23.2).
pub fn compressed_len(value: u32) -> usize {
    if value < 0x80 {
        1
    } else if value < 0x4000 {
        2
    } else {
        4
    }
}

/// Bounds-checked little-endian reader. Every read past the end is an
/// `Error::BadImage`, never a panic.
pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn align(&mut self, alignment: usize) {
        self.pos = align_up(self.pos, alignment);
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| {
                Error::BadImage(format!(
                    "unexpected end of data: wanted {n} bytes at offset {:#x}",
                    self.pos
                ))
            })?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn peek_u8(&self) -> Result<u8> {
        self.data.get(self.pos).copied().ok_or_else(|| {
            Error::BadImage(format!("unexpected end of data at offset {:#x}", self.pos))
        })
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    /// Reads a 2- or 4-byte unsigned value, as used by table columns.
    pub fn uint(&mut self, size: usize) -> Result<u32> {
        match size {
            2 => Ok(u32::from(self.u16()?)),
            4 => self.u32(),
            _ => unreachable!("table columns are 2 or 4 bytes wide"),
        }
    }

    /// §II.23.2 unsigned compressed integer.
    pub fn compressed_u32(&mut self) -> Result<u32> {
        let b0 = u32::from(self.u8()?);
        if b0 & 0x80 == 0 {
            Ok(b0)
        } else if b0 & 0xC0 == 0x80 {
            let b1 = u32::from(self.u8()?);
            Ok(((b0 & 0x3F) << 8) | b1)
        } else if b0 & 0xE0 == 0xC0 {
            let rest = self.bytes(3)?;
            Ok(((b0 & 0x1F) << 24)
                | (u32::from(rest[0]) << 16)
                | (u32::from(rest[1]) << 8)
                | u32::from(rest[2]))
        } else {
            Err(Error::BadImage(format!(
                "invalid compressed integer lead byte {b0:#04x}"
            )))
        }
    }

    /// §II.23.2 signed compressed integer: the sign bit is rotated into
    /// bit 0 of a 7-, 14- or 29-bit field.
    pub fn compressed_i32(&mut self) -> Result<i32> {
        let start = self.pos;
        let raw = self.compressed_u32()?;
        let bias: i32 = match self.pos - start {
            1 => 0x40,
            2 => 0x2000,
            _ => 0x1000_0000,
        };
        let magnitude = (raw >> 1) as i32;
        Ok(if raw & 1 == 1 {
            magnitude - bias
        } else {
            magnitude
        })
    }
}

#[derive(Debug, Default)]
pub struct ByteWriter {
    pub buf: Vec<u8>,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn zeros(&mut self, n: usize) {
        self.buf.resize(self.buf.len() + n, 0);
    }

    pub fn align(&mut self, alignment: usize) {
        let len = align_up(self.buf.len(), alignment);
        self.buf.resize(len, 0);
    }

    pub fn pad_to(&mut self, len: usize) {
        if self.buf.len() < len {
            self.buf.resize(len, 0);
        }
    }

    /// Writes a 2- or 4-byte unsigned value, as used by table columns.
    pub fn uint(&mut self, v: u32, size: usize) {
        match size {
            2 => self.u16(v as u16),
            4 => self.u32(v),
            _ => unreachable!("table columns are 2 or 4 bytes wide"),
        }
    }

    pub fn patch_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// §II.23.2 unsigned compressed integer.
    pub fn compressed_u32(&mut self, v: u32) -> Result<()> {
        if v < 0x80 {
            self.u8(v as u8);
        } else if v < 0x4000 {
            self.u8(0x80 | (v >> 8) as u8);
            self.u8(v as u8);
        } else if v < 0x2000_0000 {
            self.u8(0xC0 | (v >> 24) as u8);
            self.u8((v >> 16) as u8);
            self.u8((v >> 8) as u8);
            self.u8(v as u8);
        } else {
            return Err(Error::Malformed(format!(
                "{v:#x} is too large for a compressed integer"
            )));
        }
        Ok(())
    }

    /// §II.23.2 signed compressed integer.
    pub fn compressed_i32(&mut self, v: i32) -> Result<()> {
        let sign = u32::from(v < 0);
        let bits = v as u32;
        if (-0x40..0x40).contains(&v) {
            self.u8((((bits & 0x3F) << 1) | sign) as u8);
        } else if (-0x2000..0x2000).contains(&v) {
            let raw = ((bits & 0x1FFF) << 1) | sign;
            self.u8(0x80 | (raw >> 8) as u8);
            self.u8(raw as u8);
        } else if (-0x1000_0000..0x1000_0000).contains(&v) {
            let raw = ((bits & 0x0FFF_FFFF) << 1) | sign;
            self.u8(0xC0 | (raw >> 24) as u8);
            self.u8((raw >> 16) as u8);
            self.u8((raw >> 8) as u8);
            self.u8(raw as u8);
        } else {
            return Err(Error::Malformed(format!(
                "{v} is out of range for a compressed signed integer"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteReader, ByteWriter, align_up, compressed_len};

    // Examples from ECMA-335 §II.23.2.
    const UNSIGNED: &[(u32, &[u8])] = &[
        (0x03, &[0x03]),
        (0x7F, &[0x7F]),
        (0x80, &[0x80, 0x80]),
        (0x2E57, &[0xAE, 0x57]),
        (0x3FFF, &[0xBF, 0xFF]),
        (0x4000, &[0xC0, 0x00, 0x40, 0x00]),
        (0x1FFF_FFFF, &[0xDF, 0xFF, 0xFF, 0xFF]),
    ];

    const SIGNED: &[(i32, &[u8])] = &[
        (3, &[0x06]),
        (-3, &[0x7B]),
        (64, &[0x80, 0x80]),
        (-64, &[0x01]),
        (8192, &[0xC0, 0x00, 0x40, 0x00]),
        (-8192, &[0x80, 0x01]),
        (268_435_455, &[0xDF, 0xFF, 0xFF, 0xFE]),
        (-268_435_456, &[0xC0, 0x00, 0x00, 0x01]),
    ];

    #[test]
    fn compressed_unsigned_matches_ecma_examples() {
        for &(value, encoded) in UNSIGNED {
            let mut w = ByteWriter::new();
            w.compressed_u32(value).unwrap();
            assert_eq!(w.buf, encoded, "encoding {value:#x}");
            assert_eq!(ByteReader::new(encoded).compressed_u32().unwrap(), value);
            assert_eq!(compressed_len(value), encoded.len());
        }
    }

    #[test]
    fn compressed_signed_matches_ecma_examples() {
        for &(value, encoded) in SIGNED {
            let mut w = ByteWriter::new();
            w.compressed_i32(value).unwrap();
            assert_eq!(w.buf, encoded, "encoding {value}");
            assert_eq!(ByteReader::new(encoded).compressed_i32().unwrap(), value);
        }
    }

    #[test]
    fn compressed_out_of_range_is_an_error() {
        assert!(ByteWriter::new().compressed_u32(0x2000_0000).is_err());
        assert!(ByteWriter::new().compressed_i32(0x1000_0000).is_err());
        assert!(ByteWriter::new().compressed_i32(-0x1000_0001).is_err());
    }

    #[test]
    fn invalid_compressed_lead_byte_is_an_error() {
        assert!(ByteReader::new(&[0xE0]).compressed_u32().is_err());
    }

    #[test]
    fn reading_past_end_is_an_error_not_a_panic() {
        let mut r = ByteReader::new(&[1, 2, 3]);
        assert!(r.u32().is_err());
        assert_eq!(r.u16().unwrap(), 0x0201);
        assert!(r.u16().is_err());
        assert!(r.peek_u8().is_ok());
        r.seek(100);
        assert!(r.u8().is_err());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn little_endian_round_trip() {
        let mut w = ByteWriter::new();
        w.u8(0x11);
        w.u16(0x2233);
        w.u32(0x4455_6677);
        w.u64(0x8899_AABB_CCDD_EEFF);
        w.uint(0xABCD, 2);
        w.uint(0x1234_5678, 4);
        let mut r = ByteReader::new(&w.buf);
        assert_eq!(r.u8().unwrap(), 0x11);
        assert_eq!(r.u16().unwrap(), 0x2233);
        assert_eq!(r.u32().unwrap(), 0x4455_6677);
        assert_eq!(r.u64().unwrap(), 0x8899_AABB_CCDD_EEFF);
        assert_eq!(r.uint(2).unwrap(), 0xABCD);
        assert_eq!(r.uint(4).unwrap(), 0x1234_5678);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn align_and_patch() {
        let mut w = ByteWriter::new();
        w.u8(1);
        w.align(4);
        assert_eq!(w.len(), 4);
        w.u32(0);
        w.patch_u32(4, 0xDEAD_BEEF);
        assert_eq!(&w.buf[4..8], &[0xEF, 0xBE, 0xAD, 0xDE]);
        w.pad_to(16);
        assert_eq!(w.len(), 16);
        assert_eq!(align_up(17, 8), 24);
        assert_eq!(align_up(16, 8), 16);
    }
}
