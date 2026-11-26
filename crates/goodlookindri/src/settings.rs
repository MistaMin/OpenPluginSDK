//! Key/value settings CSV. Keys are written in sorted order. Malformed text leaves the table unchanged.
use crate::design::{fields, quote};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsTable { values: BTreeMap<String, String> }

impl SettingsTable {
    pub const TITLE: &'static str = "GoodLookinUI settings version 1";
    pub const HEADER: &'static str = "key,value";

    fn valid(k: &str, v: &str) -> bool {
        !k.is_empty() && !k.contains(['\r', '\n']) && !v.contains(['\r', '\n'])
    }
    pub fn set(&mut self, key: &str, value: &str) {
        if Self::valid(key, value) { self.values.insert(key.to_string(), value.to_string()); }
    }
    pub fn has(&self, key: &str) -> bool { self.values.contains_key(key) }
    pub fn get(&self, key: &str, fallback: &str) -> String {
        self.values.get(key).cloned().unwrap_or_else(|| fallback.to_string())
    }
    pub fn erase(&mut self, key: &str) { self.values.remove(key); }
    pub fn all(&self) -> &BTreeMap<String, String> { &self.values }

    pub fn to_csv(&self) -> String {
        let mut out = format!("{}\n{}\n", Self::TITLE, Self::HEADER);
        for (k, v) in &self.values {
            out.push_str(&quote(k).unwrap());
            out.push(',');
            out.push_str(&quote(v).unwrap());
            out.push('\n');
        }
        out
    }
    pub fn from_csv(&mut self, text: &str) -> bool {
        let parsed = (|| {
            let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
            if lines.next()? != Self::TITLE { return None; }
            if lines.next()? != Self::HEADER { return None; }
            let mut map = BTreeMap::new();
            for line in lines {
                if line.is_empty() { continue; }
                let f = fields(line).ok()?;
                if f.len() != 2 || !Self::valid(&f[0], &f[1]) { return None; }
                if map.insert(f[0].clone(), f[1].clone()).is_some() { return None; }
            }
            Some(map)
        })();
        match parsed { Some(m) => { self.values = m; true } None => false }
    }
}
