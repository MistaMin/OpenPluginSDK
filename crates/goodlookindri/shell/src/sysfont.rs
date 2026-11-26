//! Fonts installed on this computer, found by name where the system keeps them. Nothing is copied
//! into the plug-in: the face is read at run time on the machine that has it, so its licence is
//! that machine's. A name that is not installed gives `None`; draw with a bundled face then.

use crate::text::register_at;
use goodlookindri::scene::FontId;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The installed face whose full name ("Helvetica Neue Bold"), PostScript name, or family name for
/// its regular style ("Helvetica Neue") is `name`, ignoring case. The first call per name scans the
/// font folders (name tables only); the answer is kept, so later calls are a map lookup. Call it
/// from the editor, not the audio thread.
pub fn system(name: &str) -> Option<FontId> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<FontId>>>> = OnceLock::new();
    let key = name.trim().to_lowercase();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *hit;
    }
    let found = find(&key).and_then(|(path, index)| register_at(fs::read(path).ok()?, index));
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, found);
    found
}

fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts", "/Network/Library/Fonts"].map(PathBuf::from));
        dirs.extend(home.map(|h| h.join("Library/Fonts")));
    } else if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windir.join("Fonts"));
        dirs.extend(std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join(r"Microsoft\Windows\Fonts")));
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        if let Some(h) = home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    dirs
}

fn find(key: &str) -> Option<(PathBuf, u32)> {
    let mut stack: Vec<(PathBuf, u8)> = font_dirs().into_iter().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 4 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc") {
                continue;
            }
            if let Some(index) = file_faces(&path).and_then(|faces| faces.iter().position(|n| n.matches(key))) {
                return Some((path, index as u32));
            }
        }
    }
    None
}

/// Names one face gives itself, lower case: the legacy family and style (name ids 1, 2), the
/// typographic ones (16, 17; a style missing there is the legacy one), and full and PostScript (4, 6).
#[derive(Default, Debug)]
pub(crate) struct Names { family: Vec<String>, style: Vec<String>, typo_family: Vec<String>, typo_style: Vec<String>, full: Vec<String> }

impl Names {
    pub(crate) fn matches(&self, key: &str) -> bool {
        let regular = |styles: &[String]| styles.iter().any(|s| matches!(s.as_str(), "regular" | "roman" | "book" | "normal"));
        let typo_style = if self.typo_style.is_empty() { &self.style } else { &self.typo_style };
        self.full.iter().any(|n| n == key)
            || (regular(&self.style) && self.family.iter().any(|n| n == key))
            || (regular(typo_style) && self.typo_family.iter().any(|n| n == key))
    }
}

fn file_faces(path: &Path) -> Option<Vec<Names>> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    faces(&mut |off: u64, n: usize| {
        if off.checked_add(n as u64)? > len {
            return None;
        }
        let mut buf = vec![0; n];
        f.seek(SeekFrom::Start(off)).ok()?;
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    })
}

/// The names of each face in an sfnt file or collection, read through `read(offset, length)`.
pub(crate) fn faces(read: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<Vec<Names>> {
    let head = read(0, 12)?;
    let offsets: Vec<u64> = if &head[0..4] == b"ttcf" {
        let n = be32(&head, 8)?.min(256) as usize;
        let table = read(12, n * 4)?;
        (0..n).filter_map(|i| be32(&table, i * 4).map(u64::from)).collect()
    } else {
        vec![0]
    };
    Some(offsets.into_iter().map(|off| face_names(read, off).unwrap_or_default()).collect())
}

fn face_names(read: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>, off: u64) -> Option<Names> {
    let dir = read(off, 12)?;
    let tables = be16(&dir, 4)? as usize;
    let recs = read(off + 12, tables * 16)?;
    let rec = (0..tables).map(|i| i * 16).find(|&r| recs.get(r..r + 4) == Some(b"name"))?;
    let (at, len) = (be32(&recs, rec + 8)? as u64, (be32(&recs, rec + 12)? as usize).min(1 << 20));
    let t = read(at, len)?;
    let (count, strings) = (be16(&t, 2)? as usize, be16(&t, 4)? as usize);
    let mut names = Names::default();
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, encoding, id) = (be16(&t, r)?, be16(&t, r + 2)?, be16(&t, r + 6)?);
        let (n, o) = (be16(&t, r + 8)? as usize, be16(&t, r + 10)? as usize);
        let Some(raw) = t.get(strings + o..strings + o + n) else { continue };
        let text = match (platform, encoding) {
            (0, _) | (3, 1) | (3, 10) => String::from_utf16_lossy(&(0..raw.len() / 2).map(|i| u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]])).collect::<Vec<_>>()),
            (1, 0) => raw.iter().map(|&b| b as char).collect(),
            _ => continue,
        }
        .trim()
        .to_lowercase();
        match id {
            1 => names.family.push(text),
            2 => names.style.push(text),
            16 => names.typo_family.push(text),
            17 => names.typo_style.push(text),
            4 | 6 => names.full.push(text),
            _ => {}
        }
    }
    Some(names)
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{bundled, face_of, register, width_in};

    fn names_of(bytes: &[u8]) -> Vec<Names> {
        faces(&mut |off, n| bytes.get(off as usize..off as usize + n).map(<[u8]>::to_vec)).unwrap()
    }

    #[test]
    fn reads_family_style_and_full_names() {
        let faces = names_of(bundled::INTER_SEMIBOLD);
        assert_eq!(faces.len(), 1);
        let n = &faces[0];
        assert!(n.matches("inter semibold"), "{n:?}");
        assert!(n.matches("inter-semibold"), "{n:?}");
        assert!(!n.matches("inter"), "a SemiBold face is not the family's regular: {n:?}");
        assert!(names_of(bundled::INTER_REGULAR)[0].matches("inter"));
        assert!(names_of(bundled::SOURCE_SANS_3_REGULAR)[0].matches("source sans 3"));
    }

    #[test]
    fn rejects_bytes_that_are_not_a_font() {
        assert!(faces(&mut |off, n| b"not a font at all".get(off as usize..off as usize + n).map(<[u8]>::to_vec)).unwrap_or_default().iter().all(|n| !n.matches("not")));
        assert_eq!(system("No Such Font Anywhere 1234"), None);
    }

    #[test]
    fn every_bundled_face_registers_once_and_draws() {
        let all = [
            bundled::INTER_REGULAR, bundled::INTER_SEMIBOLD, bundled::INTER_BOLD,
            bundled::SOURCE_SANS_3_REGULAR, bundled::SOURCE_SANS_3_SEMIBOLD, bundled::SOURCE_SANS_3_BOLD,
            bundled::JETBRAINS_MONO_REGULAR, bundled::JETBRAINS_MONO_BOLD,
            bundled::BARLOW_CONDENSED_REGULAR, bundled::BARLOW_CONDENSED_SEMIBOLD,
            bundled::DSEG7_CLASSIC_REGULAR, bundled::DSEG7_CLASSIC_BOLD, bundled::DSEG14_CLASSIC_REGULAR,
        ];
        for bytes in all {
            let id = register(bytes).expect("bundled face parses");
            assert_ne!(id, FontId::DEFAULT);
            assert_eq!(register(bytes), Some(id), "same bytes, same id");
            assert!(width_in(id, "88", 16.0) > 0.0);
            assert!(!std::ptr::eq(face_of(id), face_of(FontId::DEFAULT)));
        }
        // Monospaced digits: "1" is as wide as "8".
        let mono = register(bundled::JETBRAINS_MONO_REGULAR).unwrap();
        assert!((width_in(mono, "1", 16.0) - width_in(mono, "8", 16.0)).abs() < 0.01);
    }

    /// Every macOS ships Helvetica and every Windows ships Arial; Linux has no font it must have.
    #[test]
    fn finds_a_font_the_system_always_has() {
        let name = if cfg!(target_os = "macos") { "Helvetica" } else if cfg!(windows) { "Arial" } else { return };
        let id = system(name).expect("system font found");
        assert_eq!(system(&name.to_uppercase()), Some(id), "case is ignored and the answer kept");
        assert!(width_in(id, "Hello", 16.0) > 0.0);
    }
}
