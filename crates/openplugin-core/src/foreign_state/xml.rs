//! A small, strict XML reader for plugin state.
//!
//! It reads what a state document needs: one root element, nested elements,
//! attributes, text, comments, processing instructions (skipped, including the
//! `<?xml?>` declaration), CDATA sections, the five predefined entities and
//! numeric character references. It refuses everything else on purpose:
//!
//! * a `<!DOCTYPE>` (so there are no user-defined entities and no expansion
//!   attacks), or any other `<!` construct except comments and CDATA;
//! * input that is not UTF-8, control characters, a `<` inside an attribute
//!   value, duplicate attributes, mismatched or unclosed tags, a second root,
//!   an unknown entity, a character reference to a character XML forbids;
//! * nesting deeper than [`Limits::max_depth`], more than [`Limits::max_nodes`]
//!   nodes, or input longer than [`Limits::max_len`].
//!
//! The reader loads the whole document into an owned tree. Nothing it builds
//! is larger than the input, because the one place text grows (a character
//! reference) replaces at least three bytes with at most four. The parse is
//! iterative: depth costs a `Vec` entry, not stack.
//!
//! Namespaces are not interpreted: `ns:name` is a name. Line ends in text and
//! attribute values are normalised as XML requires (`\r\n` and `\r` become
//! `\n`; in attributes, white space characters become spaces).

use super::{ErrorKind, ForeignError, Limits, Result};

/// One child of an element.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Node {
    /// A nested element.
    Element(Element),
    /// A run of text or CDATA, entities already replaced.
    Text(String),
}

/// One element: its name, attributes and children.
///
/// ```
/// use openplugin_core::foreign_state::xml;
/// let root = xml::parse(br#"<state v="2"><param id="gain">-6</param></state>"#).unwrap();
/// assert_eq!(root.attr("v"), Some("2"));
/// let gain = root.children_named("param").find(|p| p.attr("id") == Some("gain")).unwrap();
/// assert_eq!(gain.text(), "-6");
/// assert!(xml::parse(b"<a><b></a>").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// The tag name, prefix included (`ns:name`).
    pub name: String,
    /// `(name, value)` per attribute, in file order; names are unique.
    pub attrs: Vec<(String, String)>,
    /// Child elements and text, in file order.
    pub children: Vec<Node>,
}

impl Element {
    /// The value of attribute `name`, or `None`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The child elements, in order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// The first child element called `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    /// Every child element called `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements().filter(move |e| e.name == name)
    }

    /// The element's direct text (text and CDATA runs joined).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.children {
            if let Node::Text(t) = n {
                s.push_str(t);
            }
        }
        s
    }
}

/// Parse a document with the default [`Limits`].
pub fn parse(data: &[u8]) -> Result<Element> {
    parse_with(data, &Limits::default())
}

/// Parse a document within `limits` and return its root element. A UTF-8
/// byte-order mark is skipped.
pub fn parse_with(data: &[u8], limits: &Limits) -> Result<Element> {
    if data.len() > limits.max_len {
        return Err(ForeignError::new(ErrorKind::TooLarge, 0, "input larger than the limit"));
    }
    let text = std::str::from_utf8(data).map_err(|e| ForeignError::new(ErrorKind::Utf8, e.valid_up_to(), "input is not valid UTF-8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = data.len() - text.len();
    Parser { s: text.as_bytes(), src: text, pos: 0, bom, nodes: 0, limits: *limits }.document()
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    pos: usize,
    bom: usize,
    nodes: usize,
    limits: Limits,
}

fn is_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b':' || c >= 0x80
}

fn is_name_char(c: u8) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == b'-' || c == b'.'
}

fn xml_char_ok(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

impl<'a> Parser<'a> {
    fn err(&self, kind: ErrorKind, what: &'static str) -> ForeignError {
        ForeignError::new(kind, self.pos + self.bom, what)
    }

    fn invalid(&self, what: &'static str) -> ForeignError {
        self.err(ErrorKind::Invalid, what)
    }

    fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }

    fn starts(&self, lit: &str) -> bool {
        self.s[self.pos..].starts_with(lit.as_bytes())
    }

    fn skip_ws(&mut self) {
        while self.pos < self.s.len() && matches!(self.s[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn count(&mut self) -> Result<()> {
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(self.err(ErrorKind::TooLarge, "too many nodes"));
        }
        Ok(())
    }

    /// Skip `<!-- -->` and `<? ?>` at the cursor; `true` when one was skipped.
    fn skip_misc(&mut self) -> Result<bool> {
        if self.starts("<!--") {
            let body = self.pos + 4;
            let end = find(self.s, body, b"-->").ok_or_else(|| self.err(ErrorKind::Truncated, "unterminated comment"))?;
            if find(&self.s[..end], body, b"--").is_some() {
                return Err(self.invalid("`--` inside a comment"));
            }
            self.pos = end + 3;
            Ok(true)
        } else if self.starts("<?") {
            let end = find(self.s, self.pos + 2, b"?>").ok_or_else(|| self.err(ErrorKind::Truncated, "unterminated processing instruction"))?;
            self.pos = end + 2;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn name(&mut self) -> Result<String> {
        let start = self.pos;
        if self.eof() || !is_name_start(self.s[self.pos]) {
            return Err(self.invalid("expected a name"));
        }
        while self.pos < self.s.len() && is_name_char(self.s[self.pos]) {
            self.pos += 1;
        }
        // A name is delimited by ASCII bytes, so it ends on a character boundary.
        Ok(self.src[start..self.pos].to_string())
    }

    fn document(mut self) -> Result<Element> {
        loop {
            self.skip_ws();
            if self.eof() {
                return Err(self.err(ErrorKind::Truncated, "no root element"));
            }
            if self.starts("<!DOCTYPE") || self.starts("<!doctype") {
                return Err(self.invalid("DTDs are not allowed"));
            }
            if !self.skip_misc()? {
                break;
            }
        }
        if self.s[self.pos] != b'<' {
            return Err(self.invalid("text outside the root element"));
        }
        let root = self.element_tree()?;
        loop {
            self.skip_ws();
            if self.eof() {
                return Ok(root);
            }
            if !self.skip_misc()? {
                return Err(self.invalid("content after the root element"));
            }
        }
    }

    /// Parse the element at the cursor and everything inside it.
    fn element_tree(&mut self) -> Result<Element> {
        let max_depth = self.limits.depth();
        let mut stack: Vec<Element> = Vec::new();
        loop {
            // At `<` of a start tag.
            let (el, empty) = self.start_tag()?;
            if empty {
                self.count()?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => return Ok(el),
                }
            } else {
                if stack.len() >= max_depth {
                    return Err(self.err(ErrorKind::TooDeep, "elements nested too deeply"));
                }
                self.count()?;
                stack.push(el);
            }
            // Content until the next start tag, closing as needed.
            loop {
                let Some(top) = stack.last_mut() else { return Err(self.invalid("internal: empty stack")) };
                if self.eof() {
                    return Err(self.err(ErrorKind::Truncated, "element not closed"));
                }
                if self.s[self.pos] != b'<' {
                    let t = self.text()?;
                    self.count()?;
                    // Re-borrow: text() needed &mut self.
                    if let Some(top) = stack.last_mut() {
                        push_text(top, t);
                    }
                    continue;
                }
                if self.starts("</") {
                    self.pos += 2;
                    let name = self.name()?;
                    self.skip_ws();
                    if self.eof() || self.s[self.pos] != b'>' {
                        return Err(self.invalid("malformed end tag"));
                    }
                    self.pos += 1;
                    if stack.last().map(|e| e.name.as_str()) != Some(name.as_str()) {
                        return Err(self.invalid("end tag does not match the start tag"));
                    }
                    let mut done = stack.pop().unwrap_or_default();
                    if done.children.iter().any(|n| matches!(n, Node::Element(_))) {
                        done.children.retain(|n| !matches!(n, Node::Text(t) if t.chars().all(char::is_whitespace)));
                    }
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Node::Element(done)),
                        None => return Ok(done),
                    }
                } else if self.starts("<![CDATA[") {
                    let body = self.pos + 9;
                    let end = find(self.s, body, b"]]>").ok_or_else(|| self.err(ErrorKind::Truncated, "unterminated CDATA"))?;
                    let raw = &self.src[body..end];
                    if let Some(c) = raw.chars().find(|c| !xml_char_ok(*c)) {
                        let _ = c;
                        return Err(self.invalid("character not allowed in XML"));
                    }
                    let t = normalize_newlines(raw);
                    self.pos = end + 3;
                    self.count()?;
                    push_text(top, t);
                } else if self.starts("<!--") || self.starts("<?") {
                    self.skip_misc()?;
                } else if self.starts("<!") {
                    return Err(self.invalid("DTDs and declarations are not allowed"));
                } else {
                    break;
                }
            }
        }
    }

    /// `<name attr="v" ...>` or `<name .../>`; the cursor is on `<`.
    fn start_tag(&mut self) -> Result<(Element, bool)> {
        self.pos += 1;
        let name = self.name()?;
        let mut attrs: Vec<(String, String)> = Vec::new();
        loop {
            let before = self.pos;
            self.skip_ws();
            if self.eof() {
                return Err(self.err(ErrorKind::Truncated, "tag not closed"));
            }
            match self.s[self.pos] {
                b'>' => {
                    self.pos += 1;
                    return Ok((Element { name, attrs, children: Vec::new() }, false));
                }
                b'/' => {
                    if self.s.get(self.pos + 1) != Some(&b'>') {
                        return Err(self.invalid("malformed empty-element tag"));
                    }
                    self.pos += 2;
                    return Ok((Element { name, attrs, children: Vec::new() }, true));
                }
                _ => {
                    if self.pos == before {
                        return Err(self.invalid("attributes need white space before them"));
                    }
                    let key = self.name()?;
                    self.skip_ws();
                    if self.eof() || self.s[self.pos] != b'=' {
                        return Err(self.invalid("attribute without a value"));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let value = self.attr_value()?;
                    if attrs.iter().any(|(k, _)| *k == key) {
                        return Err(self.invalid("duplicate attribute"));
                    }
                    self.count()?;
                    attrs.push((key, value));
                }
            }
        }
    }

    fn attr_value(&mut self) -> Result<String> {
        let quote = match self.s.get(self.pos) {
            Some(q @ (b'"' | b'\'')) => *q,
            _ => return Err(self.invalid("attribute value must be quoted")),
        };
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.s.len() && self.s[self.pos] != quote {
            if self.s[self.pos] == b'<' {
                return Err(self.invalid("`<` in an attribute value"));
            }
            self.pos += 1;
        }
        if self.eof() {
            return Err(self.err(ErrorKind::Truncated, "attribute value not closed"));
        }
        let raw = &self.src[start..self.pos];
        self.pos += 1;
        let v = self.decode(raw, start, true)?;
        Ok(v)
    }

    /// Text up to the next `<`.
    fn text(&mut self) -> Result<String> {
        let start = self.pos;
        while self.pos < self.s.len() && self.s[self.pos] != b'<' {
            self.pos += 1;
        }
        let raw = &self.src[start..self.pos];
        if raw.contains("]]>") {
            return Err(self.invalid("`]]>` in text"));
        }
        self.decode(raw, start, false)
    }

    /// Resolve entities and normalise white space in `raw`, which starts at `at`.
    fn decode(&self, raw: &str, at: usize, attribute: bool) -> Result<String> {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let here = |what: &'static str| ForeignError::new(ErrorKind::Invalid, at + i + self.bom, what);
            match c {
                '&' => {
                    let rest = &raw[i + 1..];
                    let semi = rest.find(';').ok_or_else(|| here("entity without `;`"))?;
                    let body = &rest[..semi];
                    let ch = match body {
                        "lt" => '<',
                        "gt" => '>',
                        "amp" => '&',
                        "quot" => '"',
                        "apos" => '\'',
                        _ => {
                            let digits = body.strip_prefix('#').ok_or_else(|| here("unknown entity"))?;
                            let n = match digits.strip_prefix('x') {
                                Some(h) if !h.is_empty() && h.len() <= 8 && h.bytes().all(|b| b.is_ascii_hexdigit()) => u32::from_str_radix(h, 16).ok(),
                                None if !digits.is_empty() && digits.len() <= 8 && digits.bytes().all(|b| b.is_ascii_digit()) => digits.parse().ok(),
                                _ => None,
                            };
                            n.and_then(char::from_u32).filter(|c| xml_char_ok(*c)).ok_or_else(|| here("invalid character reference"))?
                        }
                    };
                    out.push(ch);
                    for _ in 0..body.chars().count() + 1 {
                        chars.next();
                    }
                }
                '\r' => {
                    if matches!(chars.peek(), Some((_, '\n'))) {
                        chars.next();
                    }
                    out.push(if attribute { ' ' } else { '\n' });
                }
                '\n' | '\t' if attribute => out.push(' '),
                c if !xml_char_ok(c) => return Err(here("character not allowed in XML")),
                c => out.push(c),
            }
        }
        Ok(out)
    }
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn push_text(el: &mut Element, t: String) {
    if t.is_empty() {
        return;
    }
    if let Some(Node::Text(last)) = el.children.last_mut() {
        last.push_str(&t);
    } else {
        el.children.push(Node::Text(t));
    }
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}
