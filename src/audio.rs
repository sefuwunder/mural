//! Voice clips: 12 kHz / 16-bit capture companded with G.711 μ-law to 8-bit,
//! i.e. exactly 12_000 * 8 = 96 kbps. Pure Rust, no dependencies.
//! The μ-law byte stream is base64-encoded for storage in mural.json.

use crate::canvas::Rect;
use serde::{Deserialize, Serialize};

/// Capture sample rate. 12 kHz covers the voice band (300–3400 Hz) with room
/// to spare; 8-bit μ-law gives 96 kbps exactly.
pub const SAMPLE_RATE: i32 = 12_000;
/// Hard cap on a single clip, in seconds. 180 s -> 2.16 MB of μ-law bytes
/// (~2.9 MB base64 in the JSON).
pub const CLIP_MAX_SECS: usize = 180;
/// Marker radius in world units.
pub const CLIP_RADIUS: f64 = 44.0;

/// G.711 μ-law encode: i16 -> u8 (Sun/standard implementation).
pub fn mulaw_encode(pcm: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const SEG_END: [i32; 8] = [0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF, 0x3FFF, 0x7FFF];
    let mut v = pcm as i32;
    let mask: i32 = if v < 0 {
        v = -v;
        0x7F
    } else {
        0xFF
    };
    if v > 32635 {
        v = 32635;
    }
    v += BIAS;
    let mut seg = 0;
    while seg < 8 && v > SEG_END[seg] {
        seg += 1;
    }
    let uval = if seg >= 8 {
        0x7F
    } else {
        ((seg as i32) << 4) | ((v >> (seg + 3)) & 0x0F)
    };
    (uval ^ mask) as u8
}

/// G.711 μ-law decode: u8 -> i16.
pub fn mulaw_decode(u: u8) -> i16 {
    let u = (u as i32) ^ 0xFF;
    let mut t = ((u & 0x0F) << 3) + 0x84;
    t <<= (u & 0x70) >> 4;
    let v = if (u & 0x80) != 0 { 0x84 - t } else { t - 0x84 };
    v.clamp(-32768, 32767) as i16
}

const B64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Minimal base64 encoder (standard alphabet, `=` padding).
pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            n |= (b as u32) << (16 - 8 * i);
        }
        let pad = 3 - chunk.len();
        for i in 0..4 - pad {
            out.push(B64_ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        for _ in 0..pad {
            out.push('=');
        }
    }
    out
}

fn b64val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Minimal base64 decoder. Rejects anything outside the standard alphabet
/// (besides trailing `=` padding and ASCII whitespace).
pub fn b64decode(s: &str) -> Result<Vec<u8>, String> {
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return Err("bad base64 length".into());
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks(4) {
        let mut n: u32 = 0;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                if i < 2 {
                    return Err("bad base64 padding".into());
                }
                pad += 1;
            } else if pad > 0 {
                return Err("bad base64 padding".into());
            } else {
                n |= b64val(c).ok_or("bad base64 char")? << (18 - 6 * i);
            }
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// A recorded voice clip pinned to the canvas.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioClip {
    pub id: u64,
    pub x: f64,       // world coords, marker center
    pub y: f64,
    pub seconds: f32, // decoded length; data.len() should equal seconds * 12000
    pub data: String,  // base64 μ-law bytes
}

impl AudioClip {
    /// Marker bounds in world coords (for hit-testing, dirty rects, export).
    pub fn rect(&self) -> Rect {
        let (x, y, r) = (self.x as f32, self.y as f32, CLIP_RADIUS as f32);
        Rect::new(x - r, y - r, x + r, y + r)
    }
    /// `m:ss` duration label.
    pub fn duration_label(&self) -> String {
        let s = self.seconds.max(0.0) as u32;
        format!("{}:{:02}", s / 60, s % 60)
    }
    pub fn pcm(&self) -> Result<Vec<i16>, String> {
        Ok(b64decode(&self.data)?.iter().map(|&u| mulaw_decode(u)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulaw_known_vectors() {
        // G.711 reference points
        assert_eq!(mulaw_encode(0), 0xFF);
        assert_eq!(mulaw_decode(0xFF), 0);
        assert_eq!(mulaw_encode(-1), 0x7F);
        assert_eq!(mulaw_decode(0x7F), 0);
        assert_eq!(mulaw_encode(32767), 0x80);
        assert_eq!(mulaw_encode(-32768), 0x00);
        // silence roundtrips to silence
        assert_eq!(mulaw_decode(mulaw_encode(0)), 0);
    }

    #[test]
    fn mulaw_roundtrip_fidelity() {
        // voice-like sweep: sine at several amplitudes plus edge values
        let mut max_err: i32 = 0;
        let mut sig_e: f64 = 0.0;
        let mut err_e: f64 = 0.0;
        let mut n: usize = 0;
        for amp in [500, 2000, 8000, 20000, 32000] {
            for k in 0..1200 {
                let s = ((k as f64 / 1200.0 * std::f64::consts::TAU * 7.0).sin()
                    * amp as f64) as i16;
                let back = mulaw_decode(mulaw_encode(s));
                let e = (s as i32 - back as i32).abs();
                max_err = max_err.max(e);
                sig_e += (s as f64) * (s as f64);
                err_e += (e as f64) * (e as f64);
                n += 1;
            }
        }
        assert!(n > 0);
        // μ-law is logarithmic: worst-case step is ~2% of full scale
        assert!(max_err < 700, "max abs error {}", max_err);
        let snr_db = 10.0 * (sig_e / err_e.max(1e-9)).log10();
        assert!(snr_db > 28.0, "SNR {:.1} dB too low for voice", snr_db);
    }

    #[test]
    fn mulaw_bitrate_is_96k() {
        // 12 kHz mono at 8 bits/sample = 96_000 bps
        assert_eq!(SAMPLE_RATE as usize * 8, 96_000);
    }

    #[test]
    fn base64_roundtrip() {
        for len in [0, 1, 2, 3, 4, 5, 100, 1000] {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let enc = b64encode(&data);
            assert_eq!(enc.len() % 4, 0);
            assert_eq!(b64decode(&enc).unwrap(), data, "len {}", len);
        }
        // known vector
        assert_eq!(b64encode(b"Man"), "TWFu");
        assert_eq!(b64decode("TWFu").unwrap(), b"Man");
        assert_eq!(b64decode("TWE=").unwrap(), b"Ma");
        assert!(b64decode("!!!").is_err());
        assert!(b64decode("abc").is_err());
    }

    #[test]
    fn clip_serde_roundtrip() {
        let ulaw: Vec<u8> = (0..24000).map(|i| mulaw_encode((i % 1000) as i16 - 500)).collect();
        let c = AudioClip {
            id: 42,
            x: 10.5,
            y: -3.25,
            seconds: 2.0,
            data: b64encode(&ulaw),
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: AudioClip = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.seconds, 2.0);
        assert_eq!(back.data, c.data);
        assert_eq!(back.pcm().unwrap().len(), 24000);
        assert_eq!(back.duration_label(), "0:02");
        let r = back.rect();
        assert!((r.x0 - (10.5 - CLIP_RADIUS as f32)).abs() < 1e-6);
        assert!((r.x1 - (10.5 + CLIP_RADIUS as f32)).abs() < 1e-6);
    }
}
