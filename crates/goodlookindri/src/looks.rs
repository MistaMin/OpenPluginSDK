//! Per (parameter, choice) look table. Empty strings mean "not set".
use crate::design::{fields, find_faceplate, find_style, quote};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub parameter: String,
    pub choice: String,
    pub colour: String,
    pub faceplate: String,
    pub style: String,
    pub plate: String,
}

pub fn valid_look(l: &Look) -> bool {
    if l.parameter.is_empty() || l.choice.is_empty() { return false; }
    if !l.colour.is_empty() && (l.colour.len() != 7 || !l.colour.starts_with('#') || !l.colour[1..].chars().all(|c| c.is_ascii_hexdigit())) { return false; }
    if !l.style.is_empty() && find_style(&l.style).is_none() { return false; }
    if !l.plate.is_empty() && find_faceplate(&l.plate).is_none() { return false; }
    if !l.faceplate.is_empty() && find_faceplate(&l.faceplate).is_none() { return false; }
    true
}

#[derive(Clone, Debug, Default)]
pub struct LookTable { defaults: Vec<Look>, rows: Vec<Look> }

impl LookTable {
    pub const HEADER: &'static str = "parameter,choice,colour,faceplate,style,plate";
    pub const TITLE: &'static str = "GoodLookinUI looks version 1";

    pub fn from_defaults(defaults: Vec<Look>) -> Self {
        let rows = defaults.clone();
        Self { defaults, rows }
    }
    pub fn reset_to_defaults(&mut self) { self.rows = self.defaults.clone(); }
    pub fn find(&self, parameter: &str, choice: &str) -> Option<&Look> {
        self.rows.iter().find(|r| r.parameter == parameter && r.choice == choice)
    }
    pub fn set(&mut self, l: Look) -> bool {
        if !valid_look(&l) { return false; }
        if let Some(r) = self.rows.iter_mut().find(|r| r.parameter == l.parameter && r.choice == l.choice) { *r = l; }
        else { self.rows.push(l); }
        true
    }
    pub fn reset_entry(&mut self, parameter: &str, choice: &str) {
        if let Some(d) = self.defaults.iter().find(|r| r.parameter == parameter && r.choice == choice).cloned() {
            let _ = self.set(d);
        } else {
            self.rows.retain(|r| !(r.parameter == parameter && r.choice == choice));
        }
    }
    pub fn all(&self) -> &[Look] { &self.rows }
    pub fn to_csv(&self) -> String {
        let mut out = format!("{}\n{}\n", Self::TITLE, Self::HEADER);
        for r in &self.rows {
            for (i, s) in [&r.parameter, &r.choice, &r.colour, &r.faceplate, &r.style, &r.plate].iter().enumerate() {
                if i > 0 { out.push(','); }
                out.push_str(&quote(s).unwrap());
            }
            out.push('\n');
        }
        out
    }
    pub fn from_csv(&mut self, text: &str) -> bool {
        let parsed = (|| {
            let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
            if lines.next()? != Self::TITLE { return None; }
            if lines.next()? != Self::HEADER { return None; }
            let mut parsed = Vec::new();
            for line in lines {
                if line.is_empty() { continue; }
                let f = fields(line).ok()?;
                if f.len() != 6 { return None; }
                let l = Look { parameter: f[0].clone(), choice: f[1].clone(), colour: f[2].clone(), faceplate: f[3].clone(), style: f[4].clone(), plate: f[5].clone() };
                if !valid_look(&l) { return None; }
                parsed.push(l);
            }
            Some(parsed)
        })();
        if let Some(parsed) = parsed { for l in parsed { self.set(l); } true } else { false }
    }
}
