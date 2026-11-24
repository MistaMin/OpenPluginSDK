//! AAX: identity only. The AAX SDK is not redistributable and an AAX plug-in
//! needs Pro Tools to host it, so nothing is loaded. The bundle's
//! `Info.plist` gives the name and version, and any page-table XML in
//! `Contents/Resources` gives the manufacturer, product and plug-in type ids
//! (`manID`, `prodID`, `plugID`).

use std::path::Path;

use crate::bundle;
use crate::manifest::*;

/// The value of `name='...'` or `name="..."` in a tag's text.
fn attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        let key = format!("{name}={quote}");
        if let Some(at) = tag.find(&key) {
            let rest = &tag[at + key.len()..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn xml_files(resources: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(resources) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml")) {
            out.push(p);
        } else if p.is_dir() {
            if let Ok(inner) = std::fs::read_dir(&p) {
                out.extend(inner.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("xml"))));
            }
        }
    }
    out.sort();
    out
}

/// Read an AAX bundle's identity from its `Info.plist` and page-table XML, without loading it.
/// Runs in the calling process (nothing is executed). `Err` only when the path is not a folder;
/// a missing `Info.plist` or page table leaves those fields empty (the name falls back to the
/// folder name).
pub fn probe(bundle_path: &Path) -> Result<ProbeResult, String> {
    if !bundle_path.is_dir() {
        return Err("an AAX plug-in is a bundle folder".into());
    }
    let plist = bundle::info_plist(bundle_path);
    let mut plugin = Plugin::default();
    if let Some(p) = &plist {
        plugin.name = bundle::plist_str(p, "CFBundleName").unwrap_or_default().to_string();
        plugin.version = bundle::plist_str(p, "CFBundleShortVersionString").unwrap_or_default().to_string();
    }
    if plugin.name.is_empty() {
        plugin.name = bundle_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }

    let mut ids = AaxIds { found_in: "page-table".into(), ..AaxIds::default() };
    let mut knobs: Vec<String> = Vec::new();
    for file in xml_files(&bundle_path.join("Contents").join("Resources")) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let mut rest = text.as_str();
        while let Some(at) = rest.find("<Plugin ") {
            let after = &rest[at..];
            let end = after.find('>').unwrap_or(after.len());
            let tag = &after[..end];
            if let (Some(man), Some(prod), Some(plug)) = (attr(tag, "manID"), attr(tag, "prodID"), attr(tag, "plugID")) {
                if ids.manufacturer.is_empty() {
                    ids.manufacturer = man;
                    ids.product = prod;
                }
                if !ids.plugin_ids.contains(&plug) {
                    ids.plugin_ids.push(plug);
                }
            }
            rest = &after[end..];
        }
        let mut rest = text.as_str();
        while let Some(at) = rest.find("knobID=") {
            let after = &rest[at..];
            if let Some(v) = attr(&after[..after.find("/>").unwrap_or(after.len())], "knobID") {
                if !v.is_empty() && !knobs.contains(&v) {
                    knobs.push(v);
                }
            }
            rest = &after[7..];
        }
    }
    if ids.manufacturer.is_empty() {
        plugin.warnings.push("no page table with manID, prodID and plugID in Contents/Resources: the AAX ids are inside the compiled binary and cannot be read without hosting it".into());
    } else {
        plugin.ids.aax = Some(ids);
    }
    if !knobs.is_empty() {
        plugin.warnings.push(format!("the page table names {} parameter ids: {}", knobs.len(), knobs.join(", ")));
    }
    Ok(ProbeResult { plugins: vec![plugin], notes: vec!["AAX is not hosted: no parameter ranges, buses or latency".into()] })
}
