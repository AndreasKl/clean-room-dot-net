use metadata::cli::{COMIMAGE_FLAGS_ILONLY, CliHeader};
use metadata::pe::PeImage;

const HELLO: &[u8] = include_bytes!("fixtures/hello.exe");
const LIB: &[u8] = include_bytes!("fixtures/lib.dll");

#[test]
fn parses_hello_cli_header() {
    let pe = PeImage::parse(HELLO).unwrap();
    assert!(pe.cli_header.rva != 0);
    let cli = CliHeader::parse(pe.slice(pe.cli_header.rva, 72).unwrap()).unwrap();
    assert_eq!(cli.flags & COMIMAGE_FLAGS_ILONLY, COMIMAGE_FLAGS_ILONLY);
    assert_eq!(cli.entry_point_token, 0x0600_0001);
    let metadata = pe.slice(cli.metadata.rva, cli.metadata.size).unwrap();
    assert_eq!(&metadata[..4], b"BSJB");
}

#[test]
fn library_has_no_entry_point() {
    let pe = PeImage::parse(LIB).unwrap();
    let cli = CliHeader::parse(pe.slice(pe.cli_header.rva, 72).unwrap()).unwrap();
    assert_eq!(cli.entry_point_token, 0);
}

#[test]
fn slice_from_reaches_section_end() {
    let pe = PeImage::parse(HELLO).unwrap();
    let text = pe
        .sections
        .iter()
        .find(|s| s.name.starts_with(b".text"))
        .unwrap();
    let tail = pe.slice_from(text.virtual_address).unwrap();
    assert_eq!(tail.len(), text.raw_size as usize);
}

#[test]
fn rejects_non_pe_input() {
    assert!(PeImage::parse(b"hello").is_err());
    assert!(PeImage::parse(&[]).is_err());
    let mut no_pe_sig = HELLO.to_vec();
    no_pe_sig[0x80] = b'X';
    assert!(PeImage::parse(&no_pe_sig).is_err());
}

#[test]
fn rva_outside_sections_is_an_error() {
    let pe = PeImage::parse(HELLO).unwrap();
    assert!(pe.slice(0x10, 4).is_err());
    assert!(pe.slice(0x7FFF_0000, 4).is_err());
    assert!(pe.slice(pe.cli_header.rva, 0x10_0000).is_err());
}

#[test]
fn truncated_image_is_an_error() {
    assert!(PeImage::parse(&HELLO[..0x100]).is_err());
    let pe = PeImage::parse(&HELLO[..0x200]).unwrap(); // headers intact, sections cut off
    assert!(pe.slice(pe.cli_header.rva, 72).is_err());
}

#[test]
fn cli_header_round_trip() {
    let pe = PeImage::parse(HELLO).unwrap();
    let bytes = pe.slice(pe.cli_header.rva, 72).unwrap();
    let cli = CliHeader::parse(bytes).unwrap();
    let mut w = metadata::bytes::ByteWriter::new();
    cli.write(&mut w);
    assert_eq!(w.len(), 72);
    assert_eq!(CliHeader::parse(&w.buf).unwrap(), cli);
    assert!(CliHeader::parse(&bytes[..40]).is_err());
}
