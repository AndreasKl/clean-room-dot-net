//! CIL method bodies: tiny and fat headers plus extra data sections (§II.25.4).

use crate::bytes::{ByteReader, ByteWriter, align_up};
use crate::{Error, Result};

const TINY_FORMAT: u8 = 0x2;
const FAT_FORMAT: u16 = 0x3;
const FAT_MORE_SECTS: u16 = 0x8;
const FAT_INIT_LOCALS: u16 = 0x10;
const FAT_HEADER_DWORDS: u16 = 3;
const SECT_FAT_FORMAT: u8 = 0x40;
const SECT_MORE_SECTS: u8 = 0x80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodBody {
    pub max_stack: u16,
    pub init_locals: bool,
    /// StandAloneSig token of the local variable signature, or 0.
    pub local_var_sig_token: u32,
    pub code: Vec<u8>,
    /// Raw extra data sections (exception tables), starting at the first
    /// section header. Empty when the body has none.
    pub extra_sections: Vec<u8>,
}

impl MethodBody {
    pub fn parse(data: &[u8]) -> Result<MethodBody> {
        let first = *data
            .first()
            .ok_or_else(|| Error::BadImage("empty method body".into()))?;
        match first & 0x3 {
            TINY_FORMAT => {
                let size = (first >> 2) as usize;
                let code = data
                    .get(1..1 + size)
                    .ok_or_else(|| Error::BadImage("truncated tiny method body".into()))?;
                Ok(MethodBody {
                    max_stack: 8,
                    init_locals: false,
                    local_var_sig_token: 0,
                    code: code.to_vec(),
                    extra_sections: Vec::new(),
                })
            }
            0x3 => Self::parse_fat(data),
            _ => Err(Error::BadImage(format!(
                "invalid method body header byte {first:#04x}"
            ))),
        }
    }

    fn parse_fat(data: &[u8]) -> Result<MethodBody> {
        let mut r = ByteReader::new(data);
        let flags_and_size = r.u16()?;
        let flags = flags_and_size & 0x0FFF;
        if flags_and_size >> 12 != FAT_HEADER_DWORDS {
            return Err(Error::BadImage(format!(
                "fat method header size {} is not 3 dwords",
                flags_and_size >> 12
            )));
        }
        let max_stack = r.u16()?;
        let code_size = r.u32()? as usize;
        let local_var_sig_token = r.u32()?;
        let code = r.bytes(code_size)?.to_vec();
        let mut extra_sections = Vec::new();
        if flags & FAT_MORE_SECTS != 0 {
            r.align(4);
            let start = r.pos();
            loop {
                let section_start = r.pos();
                let kind = r.u8()?;
                let size = if kind & SECT_FAT_FORMAT != 0 {
                    let b = r.bytes(3)?;
                    u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16)
                } else {
                    u32::from(r.u8()?)
                } as usize;
                if size < 4 {
                    return Err(Error::BadImage(format!(
                        "method data section of size {size} is smaller than its header"
                    )));
                }
                r.seek(section_start);
                r.bytes(size)?;
                if kind & SECT_MORE_SECTS == 0 {
                    break;
                }
                r.align(4);
            }
            extra_sections = data[start..r.pos()].to_vec();
        }
        Ok(MethodBody {
            max_stack,
            init_locals: flags & FAT_INIT_LOCALS != 0,
            local_var_sig_token,
            code,
            extra_sections,
        })
    }

    pub fn is_tiny(&self) -> bool {
        self.code.len() < 64
            && self.max_stack <= 8
            && self.local_var_sig_token == 0
            && !self.init_locals
            && self.extra_sections.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        if self.is_tiny() {
            w.u8(((self.code.len() as u8) << 2) | TINY_FORMAT);
            w.bytes(&self.code);
            return w.into_vec();
        }
        let mut flags = FAT_FORMAT | (FAT_HEADER_DWORDS << 12);
        if self.init_locals {
            flags |= FAT_INIT_LOCALS;
        }
        if !self.extra_sections.is_empty() {
            flags |= FAT_MORE_SECTS;
        }
        w.u16(flags);
        w.u16(self.max_stack);
        w.u32(self.code.len() as u32);
        w.u32(self.local_var_sig_token);
        w.bytes(&self.code);
        if !self.extra_sections.is_empty() {
            w.pad_to(align_up(w.len(), 4));
            w.bytes(&self.extra_sections);
        }
        w.into_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `ldstr 0x70000001; call 0x0A000001; ret` with a tiny header.
    const TINY: &[u8] = &[
        0x2E, 0x72, 0x01, 0x00, 0x00, 0x70, 0x28, 0x01, 0x00, 0x00, 0x0A, 0x2A,
    ];

    #[test]
    fn parses_and_reencodes_a_tiny_body() {
        let body = MethodBody::parse(TINY).unwrap();
        assert_eq!(body.max_stack, 8);
        assert!(!body.init_locals);
        assert_eq!(body.local_var_sig_token, 0);
        assert_eq!(body.code, &TINY[1..]);
        assert!(body.is_tiny());
        assert_eq!(body.encode(), TINY);
    }

    #[test]
    fn tiny_parse_ignores_trailing_bytes() {
        let mut data = TINY.to_vec();
        data.extend_from_slice(&[0xFF; 8]);
        assert_eq!(MethodBody::parse(&data).unwrap().code, &TINY[1..]);
    }

    #[test]
    fn fat_body_with_locals() {
        let body = MethodBody {
            max_stack: 2,
            init_locals: true,
            local_var_sig_token: 0x1100_0001,
            code: vec![0x00, 0x00, 0x2A],
            extra_sections: vec![],
        };
        assert!(!body.is_tiny());
        let bytes = body.encode();
        assert_eq!(&bytes[..12], &[0x13, 0x30, 2, 0, 3, 0, 0, 0, 1, 0, 0, 0x11]);
        assert_eq!(MethodBody::parse(&bytes).unwrap(), body);
    }

    #[test]
    fn fat_body_with_exception_section() {
        // Small EH section: kind=1, DataSize=16 (4-byte header + one 12-byte clause).
        let mut section = vec![0x01, 16, 0, 0];
        section.extend_from_slice(&[0; 12]);
        let body = MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: vec![0x00, 0x00, 0x00, 0x00, 0x2A], // 12 + 5 = 17 → section at 20
            extra_sections: section.clone(),
        };
        let bytes = body.encode();
        assert_eq!(bytes[0] & 0x08, 0x08, "MoreSects flag set");
        assert_eq!(&bytes[20..], &section[..]);
        let mut with_trailing = bytes.clone();
        with_trailing.extend_from_slice(&[0xEE; 4]);
        assert_eq!(MethodBody::parse(&with_trailing).unwrap(), body);
    }

    #[test]
    fn code_of_64_bytes_needs_a_fat_header() {
        let body = MethodBody {
            max_stack: 8,
            init_locals: false,
            local_var_sig_token: 0,
            code: vec![0; 64],
            extra_sections: vec![],
        };
        assert!(!body.is_tiny());
        assert_eq!(MethodBody::parse(&body.encode()).unwrap(), body);
    }

    #[test]
    fn malformed_bodies_are_errors() {
        assert!(MethodBody::parse(&[]).is_err());
        assert!(MethodBody::parse(&[0x00]).is_err(), "invalid header kind");
        assert!(MethodBody::parse(&TINY[..5]).is_err(), "truncated tiny");
        assert!(
            MethodBody::parse(&[0x03, 0x30, 8, 0, 0xFF, 0, 0, 0, 0, 0, 0, 0]).is_err(),
            "truncated fat"
        );
        assert!(
            MethodBody::parse(&[0x03, 0x20, 8, 0]).is_err(),
            "bad fat header size"
        );
    }
}
