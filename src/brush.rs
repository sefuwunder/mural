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
                        // greasy impasto: soft buttery radial falloff with a
                        // wet, lower-alpha center so overlapping dabs blend
                        // wet-on-wet instead of fully covering; pits keep the
                        // impasto character so it never reads as airbrushed.
                        let body = (1.0 - dist.powf(2.2)).clamp(0.0, 1.0);
                        a = body.powf(1.35) * 0.82;
                        if hash01(x as i32, y as i32, 99) < 0.14 {
                            a *= 0.55; // pits
                        } else {
                            a *= 0.94 + 0.06 * hash01(x as i32, y as i32, 100);
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

/// Separable box blur over RGBA8888 bytes (R,G,B,A memory order), used to
/// melt oil-stroke edges into the canvas. `radius` r gives a (2r+1) kernel.
/// Premultiplied-alpha correct so soft edges don't grow dark halos.
/// Pure CPU; callers must keep the region small (a stroke's dirty rect).
pub fn box_blur_rgba(px: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    assert_eq!(px.len(), w * h * 4);
    if radius == 0 || w == 0 || h == 0 {
        return px.to_vec();
    }
    let n = w * h;
    // premultiply into f32 planes
    let mut pr = vec![0.0f32; n];
    let mut pg = vec![0.0f32; n];
    let mut pb = vec![0.0f32; n];
    let mut pa = vec![0.0f32; n];
    for i in 0..n {
        let a = px[i * 4 + 3] as f32 / 255.0;
        pa[i] = a;
        pr[i] = px[i * 4] as f32 / 255.0 * a;
        pg[i] = px[i * 4 + 1] as f32 / 255.0 * a;
        pb[i] = px[i * 4 + 2] as f32 / 255.0 * a;
    }
    let mut tr = vec![0.0f32; n];
    let mut tg = vec![0.0f32; n];
    let mut tb = vec![0.0f32; n];
    let mut ta = vec![0.0f32; n];
    let k = 2 * radius + 1;
    let kinv = 1.0 / k as f32;
    // horizontal pass, clamp-to-edge, sliding window
    for y in 0..h {
        let row = y * w;
        let (sr, sg, sb, sa) = (&pr, &pg, &pb, &pa);
        let (dr, dg, db, da) = (&mut tr, &mut tg, &mut tb, &mut ta);
        let mut acc_r = 0.0f32;
        let mut acc_g = 0.0f32;
        let mut acc_b = 0.0f32;
        let mut acc_a = 0.0f32;
        for dx in -(radius as isize)..=(radius as isize) {
            let x = dx.clamp(0, w as isize - 1) as usize;
            acc_r += sr[row + x];
            acc_g += sg[row + x];
            acc_b += sb[row + x];
            acc_a += sa[row + x];
        }
        for x in 0..w {
            dr[row + x] = acc_r * kinv;
            dg[row + x] = acc_g * kinv;
            db[row + x] = acc_b * kinv;
            da[row + x] = acc_a * kinv;
            let x_out = (x as isize - radius as isize).clamp(0, w as isize - 1) as usize;
            let x_in = (x as isize + radius as isize + 1).clamp(0, w as isize - 1) as usize;
            acc_r += sr[row + x_in] - sr[row + x_out];
            acc_g += sg[row + x_in] - sg[row + x_out];
            acc_b += sb[row + x_in] - sb[row + x_out];
            acc_a += sa[row + x_in] - sa[row + x_out];
        }
    }
    // vertical pass into fresh buffers, then un-premultiply
    let mut or_ = vec![0.0f32; n];
    let mut og = vec![0.0f32; n];
    let mut ob = vec![0.0f32; n];
    let mut oa = vec![0.0f32; n];
    for x in 0..w {
        let mut acc_r = 0.0f32;
        let mut acc_g = 0.0f32;
        let mut acc_b = 0.0f32;
        let mut acc_a = 0.0f32;
        for dy in -(radius as isize)..=(radius as isize) {
            let y = dy.clamp(0, h as isize - 1) as usize;
            acc_r += tr[y * w + x];
            acc_g += tg[y * w + x];
            acc_b += tb[y * w + x];
            acc_a += ta[y * w + x];
        }
        for y in 0..h {
            or_[y * w + x] = acc_r * kinv;
            og[y * w + x] = acc_g * kinv;
            ob[y * w + x] = acc_b * kinv;
            oa[y * w + x] = acc_a * kinv;
            let y_out = (y as isize - radius as isize).clamp(0, h as isize - 1) as usize;
            let y_in = (y as isize + radius as isize + 1).clamp(0, h as isize - 1) as usize;
            acc_r += tr[y_in * w + x] - tr[y_out * w + x];
            acc_g += tg[y_in * w + x] - tg[y_out * w + x];
            acc_b += tb[y_in * w + x] - tb[y_out * w + x];
            acc_a += ta[y_in * w + x] - ta[y_out * w + x];
        }
    }
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        let a = oa[i].clamp(0.0, 1.0);
        out[i * 4 + 3] = (a * 255.0).round() as u8;
        if a > 1e-6 {
            out[i * 4] = (or_[i] / a).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8;
            out[i * 4 + 1] = (og[i] / a).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8;
            out[i * 4 + 2] = (ob[i] / a).clamp(0.0, 1.0).mul_add(255.0, 0.5) as u8;
        }
    }
    out
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
    fn solid_rgba(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&px);
        }
        v
    }
    #[test]
    fn blur_uniform_is_identity() {
        // a uniform field must survive the blur unchanged (no halos, no drift)
        let px = solid_rgba(24, 18, [200, 100, 50, 255]);
        let out = box_blur_rgba(&px, 24, 18, 2);
        assert_eq!(out.len(), px.len());
        for (a, b) in px.iter().zip(out.iter()) {
            assert!(((*a as i16) - (*b as i16)).abs() <= 1, "uniform blur drifted");
        }
        // transparent field stays transparent
        let px = solid_rgba(10, 10, [0, 0, 0, 0]);
        let out = box_blur_rgba(&px, 10, 10, 3);
        assert!(out.iter().all(|&b| b == 0));
    }
    #[test]
    fn blur_spreads_soft_edge_without_dark_halo() {
        // opaque white disc on transparent bg: blur must soften the edge
        // (mid alpha appears) but never invent dark RGB halos
        let (w, h) = (32, 32);
        let mut px = solid_rgba(w, h, [0, 0, 0, 0]);
        for y in 10..22 {
            for x in 10..22 {
                let o = (y * w + x) * 4;
                px[o] = 255;
                px[o + 1] = 255;
                px[o + 2] = 255;
                px[o + 3] = 255;
            }
        }
        let out = box_blur_rgba(&px, w, h, 2);
        // center stays fully opaque white
        let c = (16 * w + 16) * 4;
        assert_eq!(out[c + 3], 255);
        assert!(out[c] > 250 && out[c + 1] > 250 && out[c + 2] > 250);
        // just outside the disc: partial alpha, still white (no dark halo)
        let e = (8 * w + 16) * 4;
        assert!(out[e + 3] > 0 && out[e + 3] < 255, "edge should be softened");
        assert!(out[e] > 200 && out[e + 1] > 200 && out[e + 2] > 200, "no dark halo");
        // far corner untouched
        let f = (1 * w + 1) * 4;
        assert_eq!(out[f + 3], 0);
        // radius 0 is a no-op copy
        let same = box_blur_rgba(&px, w, h, 0);
        assert_eq!(same, px);
    }
    #[test]
    fn blur_stays_in_bounds() {
        // 1px-wide regions and tiny images must not panic or write OOB
        for (w, h) in [(1, 1), (3, 1), (1, 7), (5, 5)] {
            let px = solid_rgba(w, h, [10, 20, 30, 40]);
            let out = box_blur_rgba(&px, w, h, 4);
            assert_eq!(out.len(), w * h * 4);
        }
    }
}
