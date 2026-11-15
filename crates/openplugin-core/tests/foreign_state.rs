//! The foreign-state readers on hand-written fixtures, and on hostile input:
//! truncated, deeply nested, huge declared sizes, invalid text. Each must be
//! refused with an error, without a panic and without allocating what the
//! input only claims to hold.

use openplugin_core::foreign_state::bytes::{Endian, LenWidth, Reader};
use openplugin_core::foreign_state::chunk::{self, ChunkLayout, TaggedChunks};
use openplugin_core::foreign_state::kv::{self, Layout};
use openplugin_core::foreign_state::plist::{self, Value};
use openplugin_core::foreign_state::xml::{self, Node};
use openplugin_core::foreign_state::{ErrorKind, Limits};

// ---------- XML ----------

const GOOD_XML: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<!-- a comment -->\n\
<state version=\"3\" name='A &amp; B'>\n  <param id=\"gain\" value=\"-6.5\"/>\n  <param id=\"mode\" value=\"2\"></param>\n\
  <note><![CDATA[1 < 2 && 3 > 2]]> &#x41;&#66;&lt;&gt;&quot;&apos;</note>\n  <?pi ignored?>\n</state>\n<!-- trailing -->\n";

#[test]
fn xml_reads_elements_attributes_entities_and_cdata() {
    let root = xml::parse(GOOD_XML.as_bytes()).expect("parse");
    assert_eq!(root.name, "state");
    assert_eq!(root.attr("version"), Some("3"));
    assert_eq!(root.attr("name"), Some("A & B"));
    let params: Vec<_> = root.children_named("param").collect();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].attr("id"), Some("gain"));
    assert_eq!(params[0].attr("value").and_then(|v| v.parse::<f32>().ok()), Some(-6.5));
    assert_eq!(params[1].children.len(), 0);
    assert_eq!(root.child("note").map(|n| n.text()).as_deref(), Some("1 < 2 && 3 > 2 AB<>\"'"));
    // Whitespace between elements is not kept.
    assert!(root.children.iter().all(|n| matches!(n, Node::Element(_))));
}

#[test]
fn xml_every_truncation_of_a_good_document_is_refused() {
    let doc = GOOD_XML.as_bytes();
    for n in 0..doc.len() {
        // A cut inside the BOM or at a char boundary may be invalid UTF-8 or a short document; all must be errors,
        // except a cut that leaves a complete document (only trailing misc removed).
        let r = xml::parse(&doc[..n]);
        if let Ok(root) = r {
            assert_eq!(root.name, "state", "prefix {n} parsed as something else");
            assert!(n >= GOOD_XML.find("</state>").unwrap() + "</state>".len(), "prefix {n} should not be a complete document");
        }
    }
    assert!(xml::parse(doc).is_ok());
}

#[test]
fn xml_refuses_what_it_does_not_support() {
    let cases: &[(&str, &str)] = &[
        ("doctype", "<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>"),
        ("unknown entity", "<a>&nbsp;</a>"),
        ("entity without semicolon", "<a>&amp</a>"),
        ("bad char ref", "<a>&#0;</a>"),
        ("surrogate char ref", "<a>&#xD800;</a>"),
        ("huge char ref", "<a>&#99999999999;</a>"),
        ("mismatch", "<a><b></a></b>"),
        ("unclosed", "<a><b>"),
        ("two roots", "<a/><b/>"),
        ("text outside", "hello<a/>"),
        ("duplicate attribute", "<a x=\"1\" x=\"2\"/>"),
        ("unquoted attribute", "<a x=1/>"),
        ("lt in attribute", "<a x=\"<\"/>"),
        ("no space between attributes", "<a x=\"1\"y=\"2\"/>"),
        ("control character", "<a>\u{1}</a>"),
        ("double dash in comment", "<a><!-- a -- b --></a>"),
        ("cdata end in text", "<a>]]></a>"),
        ("empty", ""),
        ("declaration other than comment", "<a><!ELEMENT a ANY></a>"),
        ("bad name", "<1a/>"),
    ];
    for (name, doc) in cases {
        let r = xml::parse(doc.as_bytes());
        assert!(r.is_err(), "{name} was accepted: {doc:?}");
    }
}

#[test]
fn xml_hostile_input() {
    // Invalid UTF-8.
    let e = xml::parse(b"<a>\xff\xfe</a>").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Utf8);
    assert_eq!(e.offset, 3);
    // Very deep nesting is refused by the depth limit, not by the stack.
    let deep = "<a>".repeat(200_000);
    assert_eq!(xml::parse(deep.as_bytes()).unwrap_err().kind, ErrorKind::TooDeep);
    let wide_limit = Limits { max_depth: 1_000_000, ..Limits::default() };
    let r = xml::parse_with(deep.as_bytes(), &wide_limit);
    assert!(r.is_err(), "unclosed deep document accepted");
    // A closed document at exactly the depth limit is fine; one level more is not.
    let ok = format!("{}{}", "<a>".repeat(64), "</a>".repeat(64));
    assert!(xml::parse(ok.as_bytes()).is_ok());
    let too = format!("{}{}", "<a>".repeat(65), "</a>".repeat(65));
    assert_eq!(xml::parse(too.as_bytes()).unwrap_err().kind, ErrorKind::TooDeep);
    // Node budget.
    let many = format!("<a>{}</a>", "<b/>".repeat(1000));
    let small = Limits { max_nodes: 100, ..Limits::default() };
    assert_eq!(xml::parse_with(many.as_bytes(), &small).unwrap_err().kind, ErrorKind::TooLarge);
    // Input size cap.
    let tiny = Limits { max_len: 10, ..Limits::default() };
    assert_eq!(xml::parse_with(b"<a>0123456789</a>", &tiny).unwrap_err().kind, ErrorKind::TooLarge);
    // A "billion laughs" shape has no way in: entities cannot be declared.
    assert!(xml::parse(b"<!DOCTYPE l [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;\">]><l>&b;</l>").is_err());
    // Lots of ampersands without a semicolon do not take quadratic time or panic.
    let amps = format!("<a>{}</a>", "&".repeat(100_000));
    assert!(xml::parse(amps.as_bytes()).is_err());
}

// ---------- property lists ----------

const XML_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>name</key><string>Init</string></dict></plist>"#;

#[test]
fn xml_plist_with_the_apple_doctype_is_refused_but_without_it_reads() {
    // Apple's files carry a DOCTYPE; the strict reader refuses DTDs, so a port strips that one line first.
    assert!(plist::parse(XML_PLIST.as_bytes()).is_err());
    let stripped: String = XML_PLIST.lines().filter(|l| !l.starts_with("<!DOCTYPE")).collect::<Vec<_>>().join("\n");
    let v = plist::parse(stripped.as_bytes()).expect("plist");
    assert_eq!(v.get("name").and_then(Value::as_str), Some("Init"));
}

#[test]
fn xml_plist_value_types() {
    let doc = r#"<plist version="1.0"><dict>
<key>i</key><integer>-42</integer><key>big</key><integer>18446744073709551615</integer><key>h</key><integer>0x10</integer>
<key>r</key><real>0.25</real><key>t</key><true/><key>f</key><false/>
<key>d</key><data>
  AAEC
  Aw==</data>
<key>date</key><date>2001-01-02T00:00:10Z</date>
<key>a</key><array><string>x</string><integer>1</integer><dict/></array>
<key>s</key><string>caf&#xE9; &amp; &lt;</string>
</dict></plist>"#;
    let v = plist::parse(doc.as_bytes()).expect("plist");
    assert_eq!(v.get("i").and_then(Value::as_integer), Some(-42));
    assert_eq!(v.get("big").and_then(Value::as_integer), Some(u64::MAX as i128));
    assert_eq!(v.get("h").and_then(Value::as_integer), Some(16));
    assert_eq!(v.get("r").and_then(Value::as_f64), Some(0.25));
    assert_eq!(v.get("t").and_then(Value::as_bool), Some(true));
    assert_eq!(v.get("f").and_then(Value::as_bool), Some(false));
    assert_eq!(v.get("d").and_then(Value::as_data), Some(&[0u8, 1, 2, 3][..]));
    assert_eq!(v.get("date"), Some(&Value::Date(86_410.0)));
    assert_eq!(v.get("a").and_then(Value::as_array).map(<[_]>::len), Some(3));
    assert_eq!(v.get("s").and_then(Value::as_str), Some("caf\u{e9} & <"));
}

#[test]
fn xml_plist_refusals() {
    for doc in [
        "<plist><dict><key>a</key></dict></plist>",
        "<plist><dict><string>a</string><string>b</string></dict></plist>",
        "<plist><integer>x</integer></plist>",
        "<plist><data>@@@@</data></plist>",
        "<plist><data>AAE</data></plist>",
        "<plist><date>yesterday</date></plist>",
        "<plist><string>a</string><string>b</string></plist>",
        "<plist/>",
        "<notplist><string>a</string></notplist>",
        "<plist><frobnicate/></plist>",
    ] {
        assert!(plist::parse(doc.as_bytes()).is_err(), "accepted {doc}");
    }
    let deep = format!("<plist>{}{}</plist>", "<array>".repeat(100), "</array>".repeat(100));
    assert_eq!(plist::parse(deep.as_bytes()).unwrap_err().kind, ErrorKind::TooDeep);
}

/// A small `bplist00` writer for fixtures: every reference is one byte, every offset two.
#[derive(Clone)]
enum Obj {
    Raw(Vec<u8>),
    Array(Vec<usize>),
    Dict(Vec<usize>, Vec<usize>),
}

fn int_obj(v: u64) -> Vec<u8> {
    match v {
        0..=0xFF => vec![0x10, v as u8],
        0x100..=0xFFFF => [&[0x11][..], &(v as u16).to_be_bytes()].concat(),
        0x1_0000..=0xFFFF_FFFF => [&[0x12][..], &(v as u32).to_be_bytes()].concat(),
        _ => [&[0x13][..], &v.to_be_bytes()].concat(),
    }
}

fn marker(hi: u8, n: usize) -> Vec<u8> {
    if n < 15 { vec![(hi << 4) | n as u8] } else { [&[(hi << 4) | 0xF][..], &int_obj(n as u64)].concat() }
}

fn s(text: &str) -> Obj {
    Obj::Raw([marker(5, text.len()), text.as_bytes().to_vec()].concat())
}
fn data(bytes: &[u8]) -> Obj {
    Obj::Raw([marker(4, bytes.len()), bytes.to_vec()].concat())
}
fn int(v: u64) -> Obj {
    Obj::Raw(int_obj(v))
}
fn real(v: f64) -> Obj {
    Obj::Raw([&[0x23][..], &v.to_be_bytes()].concat())
}

/// `objects[0]` is the root. Optionally patch the trailer or the bytes afterwards.
fn bplist(objects: &[Obj]) -> Vec<u8> {
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::new();
    for o in objects {
        offsets.push(out.len());
        match o {
            Obj::Raw(b) => out.extend_from_slice(b),
            Obj::Array(r) => {
                out.extend(marker(0xA, r.len()));
                out.extend(r.iter().map(|x| *x as u8));
            }
            Obj::Dict(k, v) => {
                out.extend(marker(0xD, k.len()));
                out.extend(k.iter().map(|x| *x as u8));
                out.extend(v.iter().map(|x| *x as u8));
            }
        }
    }
    let table = out.len();
    for o in &offsets {
        out.extend_from_slice(&(*o as u16).to_be_bytes());
    }
    out.extend_from_slice(&[0; 6]);
    out.push(2);
    out.push(1);
    out.extend_from_slice(&(objects.len() as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&(table as u64).to_be_bytes());
    out
}

fn good_bplist() -> Vec<u8> {
    bplist(&[
        Obj::Dict(vec![1, 3, 5, 7], vec![2, 4, 6, 8]),
        s("name"),
        s("Warm Pad"),
        s("level"),
        real(0.75),
        s("data"),
        data(&[9, 8, 7]),
        s("tags"),
        Obj::Array(vec![1, 3, 9]),
        int(300),
    ])
}

#[test]
fn bplist_reads_a_hand_built_document() {
    let v = plist::parse(&good_bplist()).expect("bplist");
    assert_eq!(v.get("name").and_then(Value::as_str), Some("Warm Pad"));
    assert_eq!(v.get("level").and_then(Value::as_f64), Some(0.75));
    assert_eq!(v.get("data").and_then(Value::as_data), Some(&[9u8, 8, 7][..]));
    let tags = v.get("tags").and_then(Value::as_array).unwrap();
    assert_eq!(tags, &[Value::String("name".into()), Value::String("level".into()), Value::Integer(300)]);
}

#[test]
fn bplist_reads_documented_markers() {
    // null, false, true, 1/2/4/8-byte ints, 4-byte real, date, UTF-16BE string, UID, set.
    let utf16: Vec<u8> = [&[0x62][..], &[0x00, 0x48, 0x00, 0xE9]].concat(); // "He" with e-acute: H, U+00E9
    let objs = [
        Obj::Array((1..=11).collect()),
        Obj::Raw(vec![0x00]),
        Obj::Raw(vec![0x08]),
        Obj::Raw(vec![0x09]),
        Obj::Raw(vec![0x10, 0xFF]),
        Obj::Raw(vec![0x11, 0xFF, 0xFE]),
        Obj::Raw(vec![0x12, 0, 1, 0, 0]),
        Obj::Raw([&[0x13][..], &(-5i64).to_be_bytes()].concat()),
        Obj::Raw([&[0x22][..], &1.5f32.to_be_bytes()].concat()),
        Obj::Raw([&[0x33][..], &10.0f64.to_be_bytes()].concat()),
        Obj::Raw(utf16),
        Obj::Raw(vec![0x81, 0x01, 0x02]),
    ];
    let v = plist::parse(&bplist(&objs)).expect("bplist");
    let a = v.as_array().unwrap();
    assert_eq!(a[0], Value::Null);
    assert_eq!(a[1], Value::Bool(false));
    assert_eq!(a[2], Value::Bool(true));
    assert_eq!(a[3], Value::Integer(255));
    assert_eq!(a[4], Value::Integer(0xFFFE));
    assert_eq!(a[5], Value::Integer(65536));
    assert_eq!(a[6], Value::Integer(-5));
    assert_eq!(a[7], Value::Real(1.5));
    assert_eq!(a[8], Value::Date(10.0));
    assert_eq!(a[9], Value::String("H\u{e9}".into()));
    assert_eq!(a[10], Value::Uid(0x0102));
}

#[test]
fn bplist_every_truncation_is_refused() {
    let doc = good_bplist();
    for n in 0..doc.len() {
        assert!(plist::parse(&doc[..n]).is_err(), "prefix of {n} bytes was accepted");
    }
}

#[test]
fn bplist_hostile_documents() {
    // A self-referencing array.
    let looped = bplist(&[Obj::Array(vec![0])]);
    assert_eq!(plist::parse(&looped).unwrap_err().kind, ErrorKind::TooDeep);

    // An array whose count says 2^40 entries.
    let mut huge = b"bplist00".to_vec();
    huge.extend_from_slice(&[0xAF, 0x14]);
    huge.extend_from_slice(&(1u64 << 40).to_be_bytes()[3..]);
    let table = huge.len();
    huge.extend_from_slice(&8u16.to_be_bytes());
    huge.extend_from_slice(&[0; 6]);
    huge.extend_from_slice(&[2, 1]);
    huge.extend_from_slice(&1u64.to_be_bytes());
    huge.extend_from_slice(&0u64.to_be_bytes());
    huge.extend_from_slice(&(table as u64).to_be_bytes());
    assert!(plist::parse(&huge).is_err());

    // Data and strings that claim 4 GiB.
    for marker_byte in [0x4Fu8, 0x5F, 0x6F] {
        let mut d = b"bplist00".to_vec();
        d.extend_from_slice(&[marker_byte, 0x12, 0xFF, 0xFF, 0xFF, 0xFF, 1, 2, 3]);
        let table = d.len();
        d.extend_from_slice(&8u16.to_be_bytes());
        d.extend_from_slice(&[0; 6]);
        d.extend_from_slice(&[2, 1]);
        d.extend_from_slice(&1u64.to_be_bytes());
        d.extend_from_slice(&0u64.to_be_bytes());
        d.extend_from_slice(&(table as u64).to_be_bytes());
        assert_eq!(plist::parse(&d).unwrap_err().kind, ErrorKind::BadLength, "marker {marker_byte:#x}");
    }

    // One 1 MiB blob referenced from 200 array slots, in 100 arrays of 200: well under the node limit,
    // far over the byte budget, so it is refused instead of becoming 20 GB.
    let blob = vec![0u8; 1 << 20];
    let mut objs = vec![Obj::Array((1..=100).collect())];
    for _ in 0..100 {
        objs.push(Obj::Array(vec![101; 200]));
    }
    objs.push(data(&blob));
    // Reference size 1 allows 255 objects: 102 objects fit.
    let doc = bplist(&objs);
    let limits = Limits { max_total: 8 << 20, ..Limits::default() };
    assert_eq!(plist::parse_with(&doc, &limits).unwrap_err().kind, ErrorKind::TooLarge);

    // A dictionary with a non-string key; a reference past the object count; an offset into the trailer.
    assert!(plist::parse(&bplist(&[Obj::Dict(vec![1], vec![1]), int(1)])).is_err());
    assert!(plist::parse(&bplist(&[Obj::Array(vec![7])])).is_err());
    let mut bad_offset = good_bplist();
    let n = bad_offset.len();
    let table = u64::from_be_bytes(bad_offset[n - 8..].try_into().unwrap()) as usize;
    bad_offset[table..table + 2].copy_from_slice(&0xFFFFu16.to_be_bytes());
    assert!(plist::parse(&bad_offset).is_err());

    // Lone surrogate in a UTF-16 string, invalid UTF-8 in an ASCII string.
    let e = plist::parse(&bplist(&[Obj::Array(vec![1]), Obj::Raw(vec![0x61, 0xD8, 0x00])])).unwrap_err();
    assert!(matches!(e.kind, ErrorKind::Utf8 | ErrorKind::BadLength), "{e}");
    let e = plist::parse(&bplist(&[Obj::Array(vec![1]), Obj::Raw(vec![0x62, 0xD8, 0x00, 0x00, 0x41])])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Utf8);
    let e = plist::parse(&bplist(&[Obj::Array(vec![1]), Obj::Raw(vec![0x52, 0xFF, 0xFE])])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Utf8);

    // A trailer that claims more objects than bytes, and reference/offset sizes of 0 and 9.
    let mut t = good_bplist();
    let n = t.len();
    t[n - 24..n - 16].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(plist::parse(&t).is_err());
    for (i, v) in [(n - 26, 0u8), (n - 26, 9), (n - 25, 0), (n - 25, 9)] {
        let mut t = good_bplist();
        t[i] = v;
        assert!(plist::parse(&t).is_err());
    }
}

// ---------- bytes, key/value, chunks ----------

#[test]
fn reader_refuses_instead_of_panicking() {
    let mut r = Reader::big(&[0, 0, 0, 5, b'a', b'b']);
    let e = r.prefixed_bytes(LenWidth::U32).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadLength);
    assert_eq!(r.position(), 0, "a refused field consumes nothing");
    let mut r = Reader::little(&[1, 2, 3]);
    assert_eq!(r.u32().unwrap_err().kind, ErrorKind::Truncated);
    let mut r = Reader::little(&[0xFF; 8]);
    assert_eq!(r.prefixed_bytes(LenWidth::U64).unwrap_err().kind, ErrorKind::BadLength);
    assert_eq!(Reader::little(&[0xFF, 0xFF, 0xFF]).cstr().unwrap_err().kind, ErrorKind::Truncated);
    assert_eq!(Reader::little(&[0xFF, 0]).cstr().unwrap_err().kind, ErrorKind::Utf8);
    assert!(Reader::little(&[]).check_count(u64::MAX, 8).is_err());
    assert_eq!(Reader::little(&[0, 0, 0, 0]).check_count(1, 4), Ok(1));
    let mut r = Reader::big(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(r.uint(3).unwrap(), 0x010203);
    r.set_endian(Endian::Little);
    assert_eq!(r.uint(2).unwrap(), 0x0504);
    assert_eq!(Reader::little(&[0x3C, 0, 0xD8]).utf16(1).unwrap(), "<");
    assert_eq!(Reader::little(&[0x00, 0xD8, 0x00, 0x00]).utf16(2).unwrap_err().kind, ErrorKind::Utf8);
}

fn kv_blob(entries: &[(&str, &[u8])], big: bool, count: bool) -> Vec<u8> {
    let w = |n: u32| if big { n.to_be_bytes() } else { n.to_le_bytes() };
    let mut out = Vec::new();
    if count {
        out.extend_from_slice(&w(entries.len() as u32));
    }
    for (k, v) in entries {
        out.extend_from_slice(&w(k.len() as u32));
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&w(v.len() as u32));
        out.extend_from_slice(v);
    }
    out
}

#[test]
fn key_value_records_both_byte_orders() {
    for big in [false, true] {
        let gain: [u8; 4] = if big { (-3.0f32).to_be_bytes() } else { (-3.0f32).to_le_bytes() };
        let mode: [u8; 4] = if big { 2u32.to_be_bytes() } else { 2u32.to_le_bytes() };
        let blob = kv_blob(&[("gain", &gain), ("mode", &mode), ("name", b"Init")], big, true);
        let layout = Layout { endian: if big { Endian::Big } else { Endian::Little }, key_len: LenWidth::U32, value_len: LenWidth::U32, count: Some(LenWidth::U32) };
        let p = kv::read_pairs(&blob, &layout, &Limits::default()).expect("pairs");
        assert_eq!(p.entries.len(), 3);
        assert_eq!(p.f32("gain"), Some(-3.0));
        assert_eq!(p.u32("mode"), Some(2));
        assert_eq!(p.str("name"), Some("Init"));
        assert_eq!(p.get("missing"), None);
        // Without a count, records run to the end.
        let open = kv_blob(&[("gain", &gain)], big, false);
        let layout = Layout { count: None, ..layout };
        assert_eq!(kv::read_pairs(&open, &layout, &Limits::default()).unwrap().f32("gain"), Some(-3.0));
        // Every truncation is refused.
        for n in 1..open.len() {
            assert!(kv::read_pairs(&open[..n], &layout, &Limits::default()).is_err(), "prefix {n}");
        }
    }
}

#[test]
fn key_value_hostile() {
    let layout = Layout { endian: Endian::Little, key_len: LenWidth::U32, value_len: LenWidth::U32, count: Some(LenWidth::U32) };
    // A count of 4 billion records in 4 bytes.
    assert_eq!(kv::read_pairs(&u32::MAX.to_le_bytes(), &layout, &Limits::default()).unwrap_err().kind, ErrorKind::BadLength);
    // A key of 4 GiB, a value of 4 GiB, a key that is not UTF-8, bytes after the last record.
    let mut b = 1u32.to_le_bytes().to_vec();
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    b.extend_from_slice(&[0; 8]);
    assert_eq!(kv::read_pairs(&b, &layout, &Limits::default()).unwrap_err().kind, ErrorKind::BadLength);
    let mut b = kv_blob(&[("k", b"v")], false, true);
    let key_at = 8;
    b[key_at] = 0xFF;
    assert_eq!(kv::read_pairs(&b, &layout, &Limits::default()).unwrap_err().kind, ErrorKind::Utf8);
    let mut b = kv_blob(&[("k", b"v")], false, true);
    b.push(0);
    assert_eq!(kv::read_pairs(&b, &layout, &Limits::default()).unwrap_err().kind, ErrorKind::Invalid);
    // Record budget.
    let many = kv_blob(&vec![("k", &b"v"[..]); 50], false, false);
    let open = Layout { count: None, ..layout };
    let small = Limits { max_nodes: 10, ..Limits::default() };
    assert_eq!(kv::read_pairs(&many, &open, &small).unwrap_err().kind, ErrorKind::TooLarge);
}

#[test]
fn chunk_with_magic_version_and_size() {
    let layout = ChunkLayout { magic: b"FxCk", endian: Endian::Big, version_bytes: 4, size: LenWidth::U32, size_includes_header: false };
    let mut blob = b"FxCk".to_vec();
    blob.extend_from_slice(&2u32.to_be_bytes());
    blob.extend_from_slice(&3u32.to_be_bytes());
    blob.extend_from_slice(&[10, 20, 30]);
    let c = chunk::read_exact(&blob, &layout).unwrap();
    assert_eq!(c.version, Some(2));
    assert_eq!(c.payload, &[10, 20, 30]);
    assert!(chunk::has_magic(&blob, b"FxCk") && !chunk::has_magic(&blob, b"FxCx") && !chunk::has_magic(b"Fx", b"FxCk"));
    // Trailing data is returned by read_chunk and refused by read_exact.
    let mut more = blob.clone();
    more.push(99);
    let (c, rest) = chunk::read_chunk(&more, &layout).unwrap();
    assert_eq!((c.payload.len(), rest), (3, &[99u8][..]));
    assert_eq!(chunk::read_exact(&more, &layout).unwrap_err().kind, ErrorKind::Invalid);
    // A size that counts the header.
    let inc = ChunkLayout { size_includes_header: true, version_bytes: 0, size: LenWidth::U16, endian: Endian::Little, magic: b"AB" };
    let blob = [b'A', b'B', 7, 0, 1, 2, 3];
    assert_eq!(chunk::read_exact(&blob, &inc).unwrap().payload, &[1, 2, 3]);
    assert_eq!(chunk::read_exact(&[b'A', b'B', 3, 0], &inc).unwrap_err().kind, ErrorKind::BadLength);
    // Hostile sizes.
    let mut huge = b"FxCk".to_vec();
    huge.extend_from_slice(&[0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 1, 2, 3]);
    assert_eq!(chunk::read_chunk(&huge, &layout).unwrap_err().kind, ErrorKind::BadLength);
    let wide = ChunkLayout { size: LenWidth::U64, ..layout };
    let mut huge = b"FxCk".to_vec();
    huge.extend_from_slice(&[0, 0, 0, 1]);
    huge.extend_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(chunk::read_chunk(&huge, &wide).unwrap_err().kind, ErrorKind::BadLength);
    for n in 0..blob.len() {
        assert!(chunk::read_exact(&blob[..n], &inc).is_err());
    }
    assert_eq!(chunk::read_chunk(b"nope....", &layout).unwrap_err().kind, ErrorKind::Invalid);
}

#[test]
fn tagged_chunks_walk_a_sequence_and_stop_at_a_bad_size() {
    let mut data = Vec::new();
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&3u32.to_le_bytes());
    data.extend_from_slice(&[1, 2, 3, 0]); // odd payload plus a pad byte
    data.extend_from_slice(b"data");
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&[9, 9]);
    let all: Vec<_> = TaggedChunks::new(&data, Endian::Little, true).collect::<Result<_, _>>().unwrap();
    assert_eq!(all, vec![(*b"fmt ", &[1u8, 2, 3][..]), (*b"data", &[9u8, 9][..])]);
    data.extend_from_slice(b"junk");
    data.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut it = TaggedChunks::new(&data, Endian::Little, true);
    assert!(it.next().unwrap().is_ok());
    assert!(it.next().unwrap().is_ok());
    assert_eq!(it.next().unwrap().unwrap_err().kind, ErrorKind::BadLength);
    assert!(it.next().is_none(), "iteration stops after an error");
}

// ---------- mutation: nothing panics ----------

#[test]
fn mutated_documents_never_panic() {
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let corpus: Vec<(&str, Vec<u8>)> = vec![("xml", GOOD_XML.as_bytes().to_vec()), ("bplist", good_bplist()), ("xml plist", br#"<plist><dict><key>a</key><array><integer>1</integer></array></dict></plist>"#.to_vec())];
    let limits = Limits { max_total: 16 << 20, ..Limits::default() };
    for (name, base) in &corpus {
        for _ in 0..20_000 {
            let mut d = base.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % d.len() as u64) as usize;
                match next() % 3 {
                    0 => d[i] = next() as u8,
                    1 => d[i] ^= 1 << (next() % 8),
                    _ => {
                        d.truncate(i.max(1));
                    }
                }
            }
            let _ = (name, plist::parse_with(&d, &limits), xml::parse_with(&d, &limits));
        }
    }
}
