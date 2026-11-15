//! A small WAV reader and writer: 8/16/24/32-bit integer and 32/64-bit float
//! PCM in, 32-bit float out. No dependencies.

use std::path::Path;

/// Planar audio: one `Vec<f32>` per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Decode WAV bytes.
pub fn decode(bytes: &[u8]) -> Result<Audio, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut at = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32le(bytes, at + 4) as usize;
        let body = at + 8;
        let end = body.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end - body >= 16 {
            let mut tag = u16le(bytes, body);
            let channels = u16le(bytes, body + 2);
            let rate = u32le(bytes, body + 4);
            let bits = u16le(bytes, body + 14);
            if tag == 0xFFFE && end - body >= 26 {
                tag = u16le(bytes, body + 24);
            }
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            data = Some(&bytes[body..end]);
        }
        at = body.saturating_add(size).saturating_add(size & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if channels == 0 {
        return Err("zero channels".into());
    }
    let nch = channels as usize;
    let width = (bits / 8) as usize;
    if width == 0 || bits % 8 != 0 {
        return Err(format!("unsupported sample size {bits} bits"));
    }
    let frames = data.len() / (width * nch);
    let mut out = vec![Vec::with_capacity(frames); nch];
    for f in 0..frames {
        for (c, ch) in out.iter_mut().enumerate() {
            let p = (f * nch + c) * width;
            let s = &data[p..p + width];
            let v = match (tag, bits) {
                (1, 8) => (f32::from(s[0]) - 128.0) / 128.0,
                (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0,
                (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                (1, 32) => (f64::from(i32::from_le_bytes([s[0], s[1], s[2], s[3]])) / 2_147_483_648.0) as f32,
                (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                (3, 64) => f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32,
                _ => return Err(format!("unsupported WAV format tag {tag} with {bits} bits")),
            };
            ch.push(v);
        }
    }
    Ok(Audio { rate, channels: out })
}

pub fn read(path: &Path) -> Result<Audio, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Encode as 32-bit float WAV.
pub fn encode(audio: &Audio) -> Vec<u8> {
    let nch = audio.channels.len().max(1);
    let frames = audio.frames();
    let data_len = frames * nch * 4;
    let mut b = Vec::with_capacity(44 + data_len);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&(nch as u16).to_le_bytes());
    b.extend_from_slice(&audio.rate.to_le_bytes());
    b.extend_from_slice(&(audio.rate * nch as u32 * 4).to_le_bytes());
    b.extend_from_slice(&((nch * 4) as u16).to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data_len as u32).to_le_bytes());
    for f in 0..frames {
        for c in 0..nch {
            b.extend_from_slice(&audio.channels.get(c).map_or(0.0, |ch| ch[f]).to_le_bytes());
        }
    }
    b
}

pub fn write(path: &Path, audio: &Audio) -> Result<(), String> {
    std::fs::write(path, encode(audio)).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(bits: u16, tag: u16, samples: &[u8], channels: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        b.extend_from_slice(samples);
        b
    }

    #[test]
    fn reads_every_sample_format() {
        let a = decode(&pcm(16, 1, &[0x00, 0x40, 0x00, 0xC0], 1)).unwrap();
        assert_eq!(a.channels[0], vec![0.5, -0.5]);
        let a = decode(&pcm(24, 1, &[0, 0, 0x40, 0, 0, 0xC0], 1)).unwrap();
        assert_eq!(a.channels[0], vec![0.5, -0.5]);
        let a = decode(&pcm(32, 1, &[0, 0, 0, 0x40, 0, 0, 0, 0xC0], 1)).unwrap();
        assert_eq!(a.channels[0], vec![0.5, -0.5]);
        let mut f = Vec::new();
        f.extend_from_slice(&0.25f32.to_le_bytes());
        f.extend_from_slice(&(-1.0f32).to_le_bytes());
        let a = decode(&pcm(32, 3, &f, 2)).unwrap();
        assert_eq!((a.rate, a.channels), (44100, vec![vec![0.25], vec![-1.0]]));
        let a = decode(&pcm(8, 1, &[128, 192], 1)).unwrap();
        assert_eq!(a.channels[0], vec![0.0, 0.5]);
        assert!(decode(b"nope").is_err());
    }

    #[test]
    fn write_then_read_is_exact() {
        let a = Audio { rate: 48000, channels: vec![vec![0.1, -0.7, 1.0], vec![0.0, 0.3, -0.2]] };
        assert_eq!(decode(&encode(&a)).unwrap(), a);
    }
}
