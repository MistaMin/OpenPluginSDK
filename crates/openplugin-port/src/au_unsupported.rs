//! Audio Units exist only on macOS. On other systems the Audio Unit entry
//! points exist so the callers compile, and each says so.

use std::path::Path;

use crate::manifest::ProbeResult;

const WHY: &str = "Audio Units are macOS-only";

/// Not macOS: always `Err("Audio Units are macOS-only")`.
pub fn probe(_bundle_path: &Path) -> Result<ProbeResult, String> {
    Err(WHY.into())
}

/// Not macOS: always `Err("Audio Units are macOS-only")`.
pub fn capture(_job: &crate::capture::Job, _sink: &mut crate::capture::Sink) -> Result<(), String> {
    Err(WHY.into())
}

/// Not macOS: always `Err("Audio Units are macOS-only")`.
pub fn render(_job: &crate::render::Job, _feed: &crate::render::Feed) -> Result<crate::render::Rendered, String> {
    Err(WHY.into())
}
