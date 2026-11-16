//! The porting guides of `docs/porting`, compiled.
//!
//! Each guide is the documentation of one module below, so `cargo test`
//! compiles and runs every Rust code block in it as a doc test: the worked
//! ports, their checks and the conformance kit. A guide cannot describe an
//! API that no longer exists, because its code would stop compiling.
//!
//! The unit tests keep the rest of the promise: a guide holds no code block
//! that is not a Rust doc test, every file in `docs/porting` is one of the
//! guides compiled here, and `docs/PORTING.md` links each of them.
//!
//! ```text
//! cargo test -p example-porting-guides
//! ```

/// Declares one module per guide, documented by the guide's text (for the doc
/// tests), and the list of guides with that same text (for the unit tests).
macro_rules! guides {
    ($($module:ident => $file:literal),+ $(,)?) => {
        $(
            #[cfg(doctest)]
            #[doc = include_str!(concat!("../../../docs/porting/", $file))]
            pub mod $module {}
        )+

        /// Every guide: its file name in `docs/porting` and its text.
        pub const GUIDES: &[(&str, &str)] = &[
            $(($file, include_str!(concat!("../../../docs/porting/", $file)))),+
        ];
    };
}

guides! {
    iplug2 => "iplug2.md",
    dpf => "dpf.md",
    nih_plug => "nih-plug.md",
    juce => "juce.md",
    vst3 => "vst3.md",
    clap => "clap.md",
    audio_unit => "audio-unit.md",
}

/// The info strings of the fenced code blocks in a Markdown text, in order
/// (`"rust"` for a block opened with three backticks and `rust`), and the
/// number of the first line outside a fence that would start an indented code
/// block, if there is one.
pub fn code_blocks(markdown: &str) -> (Vec<&str>, Option<usize>) {
    let mut blocks = Vec::new();
    let mut indented = None;
    let mut inside = false;
    for (index, line) in markdown.lines().enumerate() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            if !inside {
                blocks.push(info.trim());
            }
            inside = !inside;
        } else if !inside && indented.is_none() && (line.starts_with("    ") || line.starts_with('\t')) {
            indented = Some(index + 1);
        }
    }
    (blocks, indented)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn every_code_block_in_a_guide_is_a_rust_doc_test() {
        for (file, text) in GUIDES {
            let (blocks, indented) = code_blocks(text);
            assert!(!blocks.is_empty(), "{file} has no worked port");
            for info in blocks {
                // Plain `rust`: no `ignore`, `no_run` or `text`, which would
                // leave the block unchecked.
                assert_eq!(info, "rust", "{file}: a code block opened with ```{info} is not a doc test that runs");
            }
            // rustdoc compiles an indented block as Rust; a guide has none.
            assert_eq!(indented, None, "{file}: an indented code block");
        }
    }

    #[test]
    fn every_worked_port_runs_the_conformance_kit() {
        for (file, text) in GUIDES {
            assert!(text.contains("conformance(") && text.contains(".assert_pass()"), "{file}: its port is not run through the conformance kit");
            assert!(text.contains("install_allocator!()"), "{file}: the allocation check would be skipped");
        }
    }

    #[test]
    fn every_file_in_docs_porting_is_a_compiled_guide() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/porting");
        let on_disk: BTreeSet<String> = std::fs::read_dir(&folder)
            .unwrap_or_else(|e| panic!("{}: {e}", folder.display()))
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let compiled: BTreeSet<String> = GUIDES.iter().map(|(file, _)| (*file).to_string()).collect();
        assert_eq!(on_disk, compiled, "docs/porting and the guides! list in this file differ");
    }

    #[test]
    fn the_porting_document_links_every_guide() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        let porting = std::fs::read_to_string(docs.join("PORTING.md")).unwrap();
        for (file, text) in GUIDES {
            assert!(porting.contains(&format!("(porting/{file})")), "docs/PORTING.md does not link porting/{file}");
            // And each guide points back at the order of work.
            assert!(text.contains("(../PORTING.md)"), "{file} does not link docs/PORTING.md");
        }
    }

    #[test]
    fn code_blocks_are_found() {
        let (blocks, indented) = code_blocks("a\n```rust\nlet x = 1;\n    indented inside\n```\n\n```text\nx\n```\n\n    stray\n");
        assert_eq!(blocks, ["rust", "text"]);
        assert_eq!(indented, Some(11));
        assert_eq!(code_blocks("| a | b |\n|---|---|\n- item\n  continued\n"), (vec![], None));
    }
}
