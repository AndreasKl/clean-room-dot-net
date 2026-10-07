//! The four metadata heaps: #Strings, #Blob, #GUID and #US (§II.24.2.2–§II.24.2.5).

use std::collections::HashMap;

use crate::bytes::{ByteReader, ByteWriter, compressed_len};
use crate::{Error, Result};

/// #Strings: NUL-terminated UTF-8 strings addressed by byte offset (§II.24.2.3).
pub struct StringHeap<'a>(&'a [u8]);

impl<'a> StringHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, offset: u32) -> Result<&'a str> {
        let offset = offset as usize;
        if offset == 0 {
            return Ok("");
        }
        let tail = self
            .0
            .get(offset..)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::BadImage(format!("#Strings offset {offset:#x} out of range")))?;
        let len = tail.iter().position(|&b| b == 0).ok_or_else(|| {
            Error::BadImage(format!("unterminated #Strings entry at {offset:#x}"))
        })?;
        std::str::from_utf8(&tail[..len])
            .map_err(|_| Error::BadImage(format!("invalid UTF-8 in #Strings at {offset:#x}")))
    }
}

/// #Blob: length-prefixed byte strings (§II.24.2.4).
pub struct BlobHeap<'a>(&'a [u8]);

impl<'a> BlobHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, offset: u32) -> Result<&'a [u8]> {
        if offset == 0 {
            return Ok(&[]);
        }
        let mut r = ByteReader::at(self.0, offset as usize);
        let len = r.compressed_u32()? as usize;
        r.bytes(len)
    }
}

/// #GUID: 16-byte GUIDs addressed by 1-based index (§II.24.2.5).
pub struct GuidHeap<'a>(&'a [u8]);

impl<'a> GuidHeap<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub fn get(&self, index: u32) -> Result<Option<[u8; 16]>> {
        if index == 0 {
            return Ok(None);
        }
        let start = (index as usize - 1) * 16;
        let bytes = self
            .0
            .get(start..start + 16)
            .ok_or_else(|| Error::BadImage(format!("#GUID index {index} out of range")))?;
        Ok(Some(bytes.try_into().unwrap()))
    }
}

/// Size in bytes of one #US entry holding `units` UTF-16 code units.
pub fn user_string_entry_size(units: usize) -> usize {
    let len = units * 2 + 1;
    compressed_len(len as u32) + len
}

/// Trailing byte of a #US entry: 1 if any unit needs more than 8-bit
/// handling, per §II.24.2.4.
pub fn user_string_flag(units: &[u16]) -> u8 {
    units
        .iter()
        .any(|&c| c > 0xFF || matches!(c, 0x01..=0x08 | 0x0E..=0x1F | 0x27 | 0x2D | 0x7F)) as u8
}

/// Reads all #US entries in heap order. Offset 0 holds the mandatory empty
/// entry; a zero length byte after it marks the start of padding.
pub fn read_user_strings(data: &[u8]) -> Result<Vec<Vec<u16>>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data[0] != 0 {
        return Err(Error::BadImage(
            "#US heap must start with an empty entry".into(),
        ));
    }
    let mut r = ByteReader::at(data, 1);
    let mut entries = Vec::new();
    while r.remaining() > 0 {
        let start = r.pos();
        let len = r.compressed_u32()? as usize;
        if len == 0 {
            break;
        }
        if len % 2 == 0 {
            return Err(Error::BadImage(format!(
                "#US entry at {start:#x} has even length {len}"
            )));
        }
        let bytes = r.bytes(len)?;
        entries.push(
            bytes[..len - 1]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        );
    }
    Ok(entries)
}

fn padded(mut data: Vec<u8>) -> Vec<u8> {
    let len = crate::bytes::align_up(data.len(), 4);
    data.resize(len, 0);
    data
}

pub struct StringHeapBuilder {
    data: Vec<u8>,
    offsets: HashMap<String, u32>,
}

impl StringHeapBuilder {
    pub fn new() -> Self {
        Self {
            data: vec![0],
            offsets: HashMap::from([(String::new(), 0)]),
        }
    }

    pub fn add(&mut self, s: &str) -> Result<u32> {
        if let Some(&offset) = self.offsets.get(s) {
            return Ok(offset);
        }
        if s.contains('\0') {
            return Err(Error::Malformed(format!("string {s:?} contains NUL")));
        }
        let offset = self.data.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
        self.offsets.insert(s.to_owned(), offset);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for StringHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BlobHeapBuilder {
    data: Vec<u8>,
    offsets: HashMap<Vec<u8>, u32>,
}

impl BlobHeapBuilder {
    pub fn new() -> Self {
        Self {
            data: vec![0],
            offsets: HashMap::new(),
        }
    }

    pub fn add(&mut self, blob: &[u8]) -> Result<u32> {
        if blob.is_empty() {
            return Ok(0);
        }
        if let Some(&offset) = self.offsets.get(blob) {
            return Ok(offset);
        }
        let offset = self.data.len() as u32;
        let mut w = ByteWriter::new();
        w.compressed_u32(blob.len() as u32)?;
        self.data.extend_from_slice(&w.buf);
        self.data.extend_from_slice(blob);
        self.offsets.insert(blob.to_vec(), offset);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for BlobHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct GuidHeapBuilder {
    guids: Vec<[u8; 16]>,
}

impl GuidHeapBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, guid: Option<[u8; 16]>) -> u32 {
        let Some(guid) = guid else { return 0 };
        if let Some(i) = self.guids.iter().position(|g| *g == guid) {
            return i as u32 + 1;
        }
        self.guids.push(guid);
        self.guids.len() as u32
    }

    pub fn finish(self) -> Vec<u8> {
        self.guids.concat()
    }
}

/// Builds #US in insertion order. No deduplication: offsets must match the
/// order of `MetadataModel::user_strings` so `ldstr` tokens stay valid.
pub struct UserStringHeapBuilder {
    data: Vec<u8>,
}

impl UserStringHeapBuilder {
    pub fn new() -> Self {
        Self { data: vec![0] }
    }

    pub fn add(&mut self, units: &[u16]) -> Result<u32> {
        let offset = self.data.len() as u32;
        let mut w = ByteWriter::new();
        w.compressed_u32((units.len() * 2 + 1) as u32)?;
        for u in units {
            w.u16(*u);
        }
        w.u8(user_string_flag(units));
        self.data.extend_from_slice(&w.buf);
        Ok(offset)
    }

    pub fn finish(self) -> Vec<u8> {
        padded(self.data)
    }
}

impl Default for UserStringHeapBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn string_heap_reads_nul_terminated_utf8() {
        let heap = StringHeap::new(b"\0abc\0de\0");
        assert_eq!(heap.get(0).unwrap(), "");
        assert_eq!(heap.get(1).unwrap(), "abc");
        assert_eq!(heap.get(5).unwrap(), "de");
        assert_eq!(heap.get(2).unwrap(), "bc");
        assert!(heap.get(9).is_err());
        assert!(StringHeap::new(b"\0ab").get(1).is_err(), "unterminated");
        assert!(
            StringHeap::new(b"\0\xFF\0").get(1).is_err(),
            "invalid UTF-8"
        );
    }

    #[test]
    fn string_builder_dedupes_and_pads() {
        let mut b = StringHeapBuilder::new();
        assert_eq!(b.add("").unwrap(), 0);
        assert_eq!(b.add("abc").unwrap(), 1);
        assert_eq!(b.add("de").unwrap(), 5);
        assert_eq!(b.add("abc").unwrap(), 1);
        assert!(b.add("a\0b").is_err());
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        let heap = StringHeap::new(&data);
        assert_eq!(heap.get(1).unwrap(), "abc");
        assert_eq!(heap.get(5).unwrap(), "de");
    }

    #[test]
    fn blob_heap_round_trip() {
        let mut b = BlobHeapBuilder::new();
        assert_eq!(b.add(&[]).unwrap(), 0);
        let small = b.add(&[1, 2, 3]).unwrap();
        let large_bytes = vec![7u8; 200];
        let large = b.add(&large_bytes).unwrap();
        assert_eq!(b.add(&[1, 2, 3]).unwrap(), small);
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        let heap = BlobHeap::new(&data);
        assert_eq!(heap.get(0).unwrap(), &[] as &[u8]);
        assert_eq!(heap.get(small).unwrap(), &[1, 2, 3]);
        assert_eq!(heap.get(large).unwrap(), &large_bytes[..]);
        assert!(BlobHeap::new(&[0, 5, 1]).get(1).is_err(), "length past end");
    }

    #[test]
    fn guid_heap_is_one_based() {
        let g = [0xAB; 16];
        let mut b = GuidHeapBuilder::new();
        assert_eq!(b.add(None), 0);
        assert_eq!(b.add(Some(g)), 1);
        assert_eq!(b.add(Some(g)), 1);
        assert_eq!(b.add(Some([1; 16])), 2);
        let data = b.finish();
        let heap = GuidHeap::new(&data);
        assert_eq!(heap.get(0).unwrap(), None);
        assert_eq!(heap.get(1).unwrap(), Some(g));
        assert_eq!(heap.get(2).unwrap(), Some([1; 16]));
        assert!(heap.get(3).is_err());
    }

    #[test]
    fn user_string_offsets_and_flag() {
        let mut b = UserStringHeapBuilder::new();
        assert_eq!(b.add(&utf16("Hello")).unwrap(), 1);
        // "Hello": 5 units → 11 bytes (10 + flag) + 1 length byte = 12.
        assert_eq!(b.add(&utf16("x")).unwrap(), 13);
        assert_eq!(user_string_entry_size(5), 12);
        assert_eq!(user_string_flag(&utf16("abc")), 0);
        assert_eq!(user_string_flag(&utf16("it's")), 1);
        assert_eq!(user_string_flag(&utf16("a-b")), 1);
        assert_eq!(user_string_flag(&utf16("é")), 0);
        assert_eq!(user_string_flag(&utf16("€")), 1);
        let data = b.finish();
        assert_eq!(data.len() % 4, 0);
        assert_eq!(
            read_user_strings(&data).unwrap(),
            vec![utf16("Hello"), utf16("x")]
        );
    }

    #[test]
    fn user_string_edge_cases_round_trip() {
        let entries = vec![
            utf16(""),
            utf16("Grüße, 世界"),
            utf16("😀"),
            vec![0xD800], // unpaired surrogate
            utf16("tab\tquote'"),
            utf16(&"long".repeat(100)), // needs a 2-byte length prefix
        ];
        let mut b = UserStringHeapBuilder::new();
        let mut expected_offset = 1;
        for e in &entries {
            assert_eq!(b.add(e).unwrap() as usize, expected_offset);
            expected_offset += user_string_entry_size(e.len());
        }
        let data = b.finish();
        assert_eq!(read_user_strings(&data).unwrap(), entries);
    }

    #[test]
    fn malformed_user_string_heaps_are_errors() {
        assert!(read_user_strings(&[1]).is_err(), "heap must start with 0");
        assert!(
            read_user_strings(&[0, 2, 0x41, 0x00]).is_err(),
            "even length"
        );
        assert!(read_user_strings(&[0, 9, 0x41]).is_err(), "length past end");
        assert_eq!(read_user_strings(&[]).unwrap(), Vec::<Vec<u16>>::new());
    }
}
