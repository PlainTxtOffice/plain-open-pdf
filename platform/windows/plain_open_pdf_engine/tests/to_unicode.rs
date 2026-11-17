//! ToUnicode CMap parsing and decoding at the public library boundary.

use plain_open_pdf_engine::ToUnicodeMap;

const TWO_BYTE: &[u8] = b"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
3 beginbfchar
<0003> <0020>
<0010> <00660069>
<0011> <D83DDE00>
endbfchar
2 beginbfrange
<0024> <0026> <0041>
<0030> <0032> [<0078> <0079> <007A0021>]
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end";

#[test]
fn maps_single_codes_and_ranges() {
    let map = ToUnicodeMap::parse(TWO_BYTE).unwrap();
    assert_eq!(map.len(), 5);
    assert!(!map.is_empty());
    assert_eq!(map.lookup(&[0x00, 0x03]).as_deref(), Some(" "));
    assert_eq!(map.lookup(&[0x00, 0x10]).as_deref(), Some("fi"));
    assert_eq!(map.lookup(&[0x00, 0x11]).as_deref(), Some("\u{1f600}"));
    // A range counts up from its base value, or lists each value.
    assert_eq!(map.lookup(&[0x00, 0x24]).as_deref(), Some("A"));
    assert_eq!(map.lookup(&[0x00, 0x26]).as_deref(), Some("C"));
    assert_eq!(map.lookup(&[0x00, 0x32]).as_deref(), Some("z!"));
    assert_eq!(map.lookup(&[0x00, 0x27]), None);
    // The code length is part of the code.
    assert_eq!(map.lookup(&[0x03]), None);
    assert_eq!(map.lookup(&[]), None);
}

#[test]
fn decodes_strings_by_codespace() {
    let map = ToUnicodeMap::parse(TWO_BYTE).unwrap();
    let shown = [0x00, 0x24, 0x00, 0x03, 0x00, 0x10, 0x00, 0x99, 0x00, 0x31];
    assert_eq!(map.decode(&shown), "A fi\u{fffd}y");
    // A trailing byte outside every codespace is one unmapped code.
    assert_eq!(map.decode(&[0x00, 0x25, 0x07]), "B\u{fffd}");
    assert_eq!(map.decode(&[]), "");
}

#[test]
fn reads_one_byte_and_mixed_length_codes() {
    let map = ToUnicodeMap::parse(
        b"2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange
          1 beginbfrange <41> <5A> <0061> endbfrange
          1 beginbfchar <8140> <3000> endbfchar",
    )
    .unwrap();
    assert_eq!(map.decode(b"HI\x81\x40Z"), "hi\u{3000}z");

    // Without codespace ranges each byte is a code.
    let bare = ToUnicodeMap::parse(b"1 beginbfchar <41> <0042> endbfchar").unwrap();
    assert_eq!(bare.decode(b"AA"), "BB");
}

#[test]
fn ignores_malformed_entries() {
    let map = ToUnicodeMap::parse(
        b"3 beginbfchar <41> <0042> /bad <0043> <0102030405> <0044> endbfchar
          2 beginbfrange <10> <1F> 7 <2000> <20> <0041> endbfrange",
    )
    .unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map.decode(b"A"), "B");
    assert!(ToUnicodeMap::parse(b"").unwrap().is_empty());
    assert!(ToUnicodeMap::parse(b"1 beginbfchar <41").is_err());
}
