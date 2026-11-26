//! Developer-mode design autosave. Debounced, atomic (temp file then rename), and quiet when the bytes match.
//! Line endings already on disk are kept. Call [`DesignAutosave::flush`] when the editor closes.
//! The host owns the timer: call [`DesignAutosave::poll`] with elapsed milliseconds.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SaveEvent {
    pub name: String,
    pub file: PathBuf,
    pub wrote: bool,
    pub ok: bool,
    pub error: String,
}

pub struct DesignAutosave {
    dir: Option<PathBuf>,
    delay_ms: u64,
    elapsed_ms: u64,
    pending: BTreeMap<String, String>,
    pub last: Vec<SaveEvent>,
}
impl DesignAutosave {
    pub fn new(folder: impl Into<PathBuf>, delay_ms: u64) -> Self {
        Self { dir: Some(folder.into()), delay_ms, elapsed_ms: 0, pending: BTreeMap::new(), last: Vec::new() }
    }
    pub fn has_folder(&self) -> bool { self.dir.is_some() }
    pub fn file_for(&self, name: &str) -> PathBuf {
        self.dir.clone().unwrap_or_default().join(name)
    }
    pub fn request(&mut self, name: &str, text: &str) {
        if self.dir.is_none() { return; }
        self.pending.insert(name.to_string(), text.to_string());
        self.elapsed_ms = 0;
    }
    /// Host timer. When the quiet period elapses, queued files are written.
    pub fn poll(&mut self, dt_ms: u64) {
        if self.pending.is_empty() { return; }
        self.elapsed_ms += dt_ms;
        if self.elapsed_ms >= self.delay_ms { self.flush(); }
    }
    pub fn flush(&mut self) {
        let work = std::mem::take(&mut self.pending);
        for (name, text) in work { let _ = self.write_now(&name, &text); }
    }
    pub fn read(&self, name: &str) -> String {
        let f = self.file_for(name);
        fs::read_to_string(f).unwrap_or_default()
    }
    pub fn write_now(&mut self, name: &str, raw: &str) -> bool {
        let Some(dir) = self.dir.clone() else { return false; };
        let file = dir.join(name);
        let mut ev = SaveEvent { name: name.into(), file: file.clone(), wrote: false, ok: true, error: String::new() };
        let existing = fs::read_to_string(&file).ok();
        let crlf = existing.as_ref().map(|s| s.contains("\r\n")).unwrap_or(false);
        let text = if crlf {
            let mut t = String::new();
            for c in raw.chars() { if c == '\n' { t.push('\r'); } t.push(c); }
            t
        } else { raw.to_string() };
        if fs::create_dir_all(&dir).is_err() {
            ev.ok = false; ev.error = format!("cannot create {}", dir.display());
        } else if existing.as_deref() == Some(text.as_str()) {
            // unchanged
        } else {
            let tmp = file.with_extension("tmpwrite");
            let result = (|| {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(text.as_bytes())?;
                f.sync_all()?;
                fs::rename(&tmp, &file)?;
                Ok::<(), std::io::Error>(())
            })();
            match result {
                Ok(()) => ev.wrote = true,
                Err(e) => { ev.ok = false; ev.error = format!("cannot write {}: {e}", file.display()); let _ = fs::remove_file(&tmp); }
            }
        }
        let ok = ev.ok;
        self.last.push(ev);
        ok
    }
}
impl Drop for DesignAutosave { fn drop(&mut self) { self.flush(); } }

pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
    Ok(())
}
