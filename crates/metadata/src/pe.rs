//! PE/COFF image reader (§II.25.2): just enough to locate the CLI header and
//! map RVAs to file bytes. PE32 and PE32+ are both accepted.

use crate::bytes::ByteReader;
use crate::{Error, Result};

const CLI_DIRECTORY_INDEX: usize = 14;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_size: u32,
    pub raw_pointer: u32,
}

#[derive(Debug)]
pub struct PeImage<'a> {
    data: &'a [u8],
    pub sections: Vec<Section>,
    pub cli_header: DataDirectory,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = ByteReader::new(data);
        if r.bytes(2)? != b"MZ" {
            return Err(Error::BadImage("missing MZ signature".into()));
        }
        r.seek(0x3C);
        let pe_offset = r.u32()? as usize;
        r.seek(pe_offset);
        if r.bytes(4)? != b"PE\0\0" {
            return Err(Error::BadImage("missing PE signature".into()));
        }
        let _machine = r.u16()?;
        let section_count = r.u16()?;
        r.bytes(12)?; // timestamp, symbol table pointer, symbol count
        let optional_size = r.u16()? as usize;
        let _characteristics = r.u16()?;

        let optional_start = r.pos();
        let directories_offset = match r.u16()? {
            0x10B => 96,  // PE32
            0x20B => 112, // PE32+
            magic => {
                return Err(Error::BadImage(format!(
                    "unknown optional header magic {magic:#x}"
                )));
            }
        };
        r.seek(optional_start + directories_offset - 4);
        let directory_count = r.u32()? as usize;
        if directory_count <= CLI_DIRECTORY_INDEX {
            return Err(Error::BadImage(
                "image has no CLI header data directory".into(),
            ));
        }
        r.seek(optional_start + directories_offset + CLI_DIRECTORY_INDEX * 8);
        let cli_header = DataDirectory {
            rva: r.u32()?,
            size: r.u32()?,
        };
        if cli_header.rva == 0 {
            return Err(Error::BadImage(
                "not a CLI image: CLI header directory is empty".into(),
            ));
        }

        r.seek(optional_start + optional_size);
        let mut sections = Vec::with_capacity(section_count as usize);
        for _ in 0..section_count {
            let name: [u8; 8] = r.bytes(8)?.try_into().unwrap();
            let virtual_size = r.u32()?;
            let virtual_address = r.u32()?;
            let raw_size = r.u32()?;
            let raw_pointer = r.u32()?;
            r.bytes(16)?; // relocations, line numbers, characteristics
            sections.push(Section {
                name,
                virtual_address,
                virtual_size,
                raw_size,
                raw_pointer,
            });
        }
        Ok(PeImage {
            data,
            sections,
            cli_header,
        })
    }

    /// File offset of `rva` and the number of file bytes available from there
    /// to the end of its section.
    fn locate(&self, rva: u32) -> Result<(usize, usize)> {
        for s in &self.sections {
            let span = s.virtual_size.max(s.raw_size);
            if rva < s.virtual_address || rva - s.virtual_address >= span {
                continue;
            }
            let delta = rva - s.virtual_address;
            if delta >= s.raw_size {
                return Err(Error::BadImage(format!(
                    "RVA {rva:#x} is not backed by file data"
                )));
            }
            let offset = s.raw_pointer as usize + delta as usize;
            if offset >= self.data.len() {
                return Err(Error::BadImage(format!(
                    "RVA {rva:#x} lies beyond the end of the file"
                )));
            }
            let available = ((s.raw_size - delta) as usize).min(self.data.len() - offset);
            return Ok((offset, available));
        }
        Err(Error::BadImage(format!(
            "RVA {rva:#x} is not inside any section"
        )))
    }

    pub fn slice(&self, rva: u32, len: u32) -> Result<&'a [u8]> {
        let (offset, available) = self.locate(rva)?;
        if len as usize > available {
            return Err(Error::BadImage(format!(
                "{len} bytes at RVA {rva:#x} exceed the section's file data"
            )));
        }
        Ok(&self.data[offset..offset + len as usize])
    }

    pub fn slice_from(&self, rva: u32) -> Result<&'a [u8]> {
        let (offset, available) = self.locate(rva)?;
        Ok(&self.data[offset..offset + available])
    }
}
