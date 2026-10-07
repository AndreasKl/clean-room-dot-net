//! The CLI header (§II.25.3.3).

use crate::bytes::{ByteReader, ByteWriter};
use crate::pe::DataDirectory;
use crate::{Error, Result};

pub const CLI_HEADER_SIZE: usize = 72;

pub const COMIMAGE_FLAGS_ILONLY: u32 = 0x1;
pub const COMIMAGE_FLAGS_32BITREQUIRED: u32 = 0x2;
pub const COMIMAGE_FLAGS_STRONGNAMESIGNED: u32 = 0x8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliHeader {
    pub runtime_major: u16,
    pub runtime_minor: u16,
    pub metadata: DataDirectory,
    pub flags: u32,
    pub entry_point_token: u32,
    pub resources: DataDirectory,
    pub strong_name: DataDirectory,
}

fn read_dir(r: &mut ByteReader) -> Result<DataDirectory> {
    Ok(DataDirectory {
        rva: r.u32()?,
        size: r.u32()?,
    })
}

fn write_dir(w: &mut ByteWriter, d: DataDirectory) {
    w.u32(d.rva);
    w.u32(d.size);
}

impl CliHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < CLI_HEADER_SIZE {
            return Err(Error::BadImage(format!(
                "CLI header truncated to {} bytes",
                bytes.len()
            )));
        }
        let mut r = ByteReader::new(bytes);
        let cb = r.u32()? as usize;
        if cb < CLI_HEADER_SIZE {
            return Err(Error::BadImage(format!(
                "CLI header size {cb} is smaller than 72"
            )));
        }
        Ok(CliHeader {
            runtime_major: r.u16()?,
            runtime_minor: r.u16()?,
            metadata: read_dir(&mut r)?,
            flags: r.u32()?,
            entry_point_token: r.u32()?,
            resources: read_dir(&mut r)?,
            strong_name: read_dir(&mut r)?,
        })
    }

    pub fn write(&self, w: &mut ByteWriter) {
        w.u32(CLI_HEADER_SIZE as u32);
        w.u16(self.runtime_major);
        w.u16(self.runtime_minor);
        write_dir(w, self.metadata);
        w.u32(self.flags);
        w.u32(self.entry_point_token);
        write_dir(w, self.resources);
        write_dir(w, self.strong_name);
        // Code manager table, VTable fixups, export address table jumps and
        // managed native header: always zero for IL-only images.
        w.zeros(32);
    }
}
