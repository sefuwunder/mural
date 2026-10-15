//! Brush engine: procedural brush-tip textures + stroke stamping.
//!
//! Tips are generated white-with-alpha and tinted at draw time with
//! `set_color_mod`, so one tip per (tool, pixel-size) serves all 16 colors.

use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::render::{BlendMode, Canvas, Texture, TextureCreator};
use sdl2::surface::Surface;
use sdl2::video::{Window, WindowContext};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tool {
    Pencil,
    Pen,
    Marker,
    OilStick,
}

impl Tool {
    /// stamp spacing as a fraction of the tip diameter
    pub fn spacing(self) -> f32 {
        match self {
            Tool::Pencil | Tool::Pen => 0.35,
            Tool::Marker => 0.45,
            Tool::OilStick => 0.55,
        }
    }
    pub fn default_size(self) -> f32 {
        match self {
            Tool::Pencil => 3.0,
            Tool::Pen => 5.0,
            Tool::Marker => 22.0,
            Tool::OilStick => 30.0,
        }
    }
}

fn hash01(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x85eb_ca6b)
        ^ (y as u32).wrapping_mul(0xc2b2_ae35)
        ^ seed.wrapping_mul(0x27d4_eb2f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h as f32) / (u32::MAX as f32)
}

/// Build a white-with-alpha brush tip of `d` px diameter.
pub fn make_tip<'a>(
    tc: &'a TextureCreator<WindowContext>,
    tool: Tool,
    d: u32,
) -> Result<Texture<'a>, String> {
    let d = d.max(2);
    let r = d as f32 / 2.0;
    let mut surf = Surface::new(d, d, PixelFormatEnum::RGBA32).map_err(|e| e.to_string())?;
    surf.with_lock_mut(|px| {
        for y in 0..d {
            for x in 0..d {
                let dx = x as f32 + 0.5 - r;
                let dy = y as f32 + 0.5 - r;
                let dist = (dx * dx + dy * dy).sqrt() / r; // 0 center .. 1+ edge
                // 1px antialiased edge
                let edge = ((1.0 - dist) * r).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    continue;
                }
                let mut a = edge;
                match tool {
                    Tool::Pen => {}
                    Tool::Pencil => {
                        // graphite grain
                        a *= 0.68 + 0.32 * hash01(x as i32, y as i32, 7);
                    }
                    Tool::Marker => {
                        // soft alcohol-marker falloff, translucent
                        a = (1.0 - dist.powf(1.6)).clamp(0.0, 1.0).powf(0.8) * 0.42;
                    }
                    Tool::OilStick => {
                        // impasto pits
                        if hash01(x as i32, y as i32, 99) < 0.16 {
                            a *= 0.42;
                        } else {
                            a *= 0.92 + 0.08 * hash01(x as i32, y as i32, 100);
                        }
                    }
                }
                let o = ((y * d + x) * 4) as usize;
                px[o] = 255;
                px[o + 1] = 255;
                px[o + 2] = 255;
                px[o + 3] = (a.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    });
    let tex = tc
        .create_texture_from_surface(&surf)
        .map_err(|e| e.to_string())?;
    Ok(tex)
}

/// Cache of brush tips keyed by (tool, diameter px).
pub struct TipCache<'a> {
    tips: HashMap<(Tool, u32), Texture<'a>>,
}

// Textures borrow the TextureCreator, so the cache must not outlive it
// (both live in App, creator declared first).
impl<'a> TipCache<'a> {
    pub fn new() -> Self {
        Self { tips: HashMap::new() }
    }
    pub fn get(
        &mut self,
        tc: &'a TextureCreator<WindowContext>,
        tool: Tool,
        diameter_px: u32,
    ) -> Result<&mut Texture<'a>, String> {
        let key = (tool, diameter_px.max(2));
        if !self.tips.contains_key(&key) {
            let mut tex = make_tip(tc, tool, key.1)?;
            tex.set_blend_mode(BlendMode::Blend);
            self.tips.insert(key, tex);
        }
        Ok(self.tips.get_mut(&key).expect("tip"))
    }
}

/// Stamp a brush tip along a segment. `stamp` is called with (x, y, diameter_px).
pub fn walk_segment(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    diameter: f32,
    spacing_frac: f32,
    mut stamp: impl FnMut(f32, f32, f32),
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    let step = (diameter * spacing_frac).max(0.75);
    if len < 0.001 {
        stamp(x0, y0, diameter);
        return;
    }
    let n = (len / step).ceil() as i32;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        stamp(x0 + dx * t, y0 + dy * t, diameter);
    }
}

/// Blit one stamp centered at (x, y) with the given diameter (all in the
/// target's pixel space). The caller sets color modulation on the tip.
pub fn blit_stamp(
    canvas: &mut Canvas<Window>,
    tip: &Texture,
    x: f32,
    y: f32,
    diameter: f32,
) -> Result<(), String> {
    let d = diameter.max(1.0);
    let dst = Rect::new((x - d / 2.0) as i32, (y - d / 2.0) as i32, d as u32, d as u32);
    canvas.copy(tip, None, dst).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_spacing_sane() {
        for t in [Tool::Pencil, Tool::Pen, Tool::Marker, Tool::OilStick] {
            assert!(t.spacing() > 0.0 && t.spacing() < 1.0);
            assert!(t.default_size() > 0.0);
        }
    }
    #[test]
    fn walk_segment_covers_endpoints() {
        let mut pts = vec![];
        walk_segment(0.0, 0.0, 10.0, 0.0, 4.0, 0.5, |x, y, _| pts.push((x, y)));
        assert!(!pts.is_empty());
        assert!((pts[0].0 - 0.0).abs() < 0.01);
        assert!((pts.last().unwrap().0 - 10.0).abs() < 0.01);
    }
    #[test]
    fn walk_segment_degenerate() {
        let mut n = 0;
        walk_segment(5.0, 5.0, 5.0, 5.0, 4.0, 0.5, |_, _, _| n += 1);
        assert_eq!(n, 1);
    }
    #[test]
    fn hash_deterministic() {
        assert_eq!(hash01(3, 7, 99), hash01(3, 7, 99));
        assert!(hash01(0, 0, 0) >= 0.0 && hash01(0, 0, 0) < 1.0);
    }
}
