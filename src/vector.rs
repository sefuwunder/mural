//! Vectorize (via vtracer) + vector animation frame rendering.
//! All CPU-side; frames are uploaded as textures by main.

use crate::canvas::{AnimKind, VectorMeta};
use vtracer::ir::{PathCmd, VectorDoc};
use vtracer::{ColorImage, Config, FitMode, Preset};

/// One traced polygon with precomputed helpers for animation.
#[derive(Clone, Debug)]
pub struct TracedPoly {
    pub pts: Vec<(f32, f32)>, // local px coords
    pub centroid: (f32, f32),
    pub area: f32,
}

pub fn rebuild_polys(meta: &VectorMeta) -> Vec<TracedPoly> {
    meta.polys.iter().map(|p| analyze(p)).collect()
}

fn analyze(pts: &[(f32, f32)]) -> TracedPoly {
    let n = pts.len().max(1) as f32;
    let (sx, sy) = pts.iter().fold((0.0f32, 0.0f32), |(a, b), &(x, y)| (a + x, b + y));
    let mut area2 = 0.0;
    for w in pts.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        area2 += ax * by - bx * ay;
    }
    TracedPoly {
        pts: pts.to_vec(),
        centroid: (sx / n, sy / n),
        area: area2.abs() / 2.0,
    }
}

const TRACE_MAX_DIM: u32 = 192;
const MAX_POLYS: usize = 400;

/// Trace an RGBA image into polygons. Returns (polys, w, h) in trace-px space.
pub fn vectorize_rgba(pixels: &[u8], w: u32, h: u32) -> Result<(Vec<TracedPoly>, u32, u32), String> {
    let scale = (TRACE_MAX_DIM as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = ((w as f32 * scale).max(8.0) as u32, (h as f32 * scale).max(8.0) as u32);
    // downscale with image crate (nearest is fine for tracing)
    let src = image::RgbaImage::from_raw(w, h, pixels.to_vec()).ok_or("bad pixels")?;
    let small = image::imageops::resize(&src, tw, th, image::imageops::FilterType::Nearest);
    let img = ColorImage {
        pixels: small.into_raw(),
        width: tw as usize,
        height: th as usize,
    };
    let mut cfg = Config::from_preset(Preset::Bw);
    cfg.mode = FitMode::Polygon;
    cfg.filter_speckle = 12;
    cfg.binary_threshold = 128;
    let pipeline = cfg.build().map_err(|e| e.to_string())?;
    let seg = pipeline.segment(&img).map_err(|e| e.to_string())?;
    let doc: VectorDoc = pipeline.finish(&seg).map_err(|e| e.to_string())?;
    let mut polys: Vec<TracedPoly> = vec![];
    for shape in &doc.shapes {
        for sub in &shape.path.subpaths {
            let pts = flatten_subpath(&sub.commands);
            if pts.len() < 4 {
                continue;
            }
            let tp = analyze(&pts);
            if tp.area < 6.0 {
                continue;
            }
            polys.push(tp);
            if polys.len() >= MAX_POLYS {
                break;
            }
        }
        if polys.len() >= MAX_POLYS {
            break;
        }
    }
    // biggest first — stable, nicer draw-on order
    polys.sort_by(|a, b| b.area.partial_cmp(&a.area).unwrap_or(std::cmp::Ordering::Equal));
    if polys.is_empty() {
        return Err("nothing to trace (try a higher-contrast image)".into());
    }
    Ok((polys, tw, th))
}

fn flatten_subpath(cmds: &[PathCmd]) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = vec![];
    let mut cur = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    for cmd in cmds {
        match cmd {
            PathCmd::MoveTo(p) => {
                cur = (p.x as f32, p.y as f32);
                start = cur;
                pts.push(cur);
            }
            PathCmd::LineTo(p) => {
                cur = (p.x as f32, p.y as f32);
                pts.push(cur);
            }
            PathCmd::CubicTo(p1, p2, p3) => {
                let (x1, y1) = (p1.x as f32, p1.y as f32);
                let (x2, y2) = (p2.x as f32, p2.y as f32);
                let (x3, y3) = (p3.x as f32, p3.y as f32);
                let steps = 10;
                for i in 1..=steps {
                    let t = i as f32 / steps as f32;
                    let mt = 1.0 - t;
                    let x = mt * mt * mt * cur.0 + 3.0 * mt * mt * t * x1 + 3.0 * mt * t * t * x2 + t * t * t * x3;
                    let y = mt * mt * mt * cur.1 + 3.0 * mt * mt * t * y1 + 3.0 * mt * t * t * y2 + t * t * t * y3;
                    pts.push((x, y));
                }
                cur = (x3, y3);
            }
            PathCmd::Close => {
                pts.push(start);
                cur = start;
            }
        }
    }
    pts
}

// ---------- frame rendering ----------

fn blend_px(buf: &mut [u8], bw: u32, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 || x >= bw as i32 {
        return;
    }
    let bh = buf.len() as u32 / bw / 4;
    if y >= bh as i32 {
        return;
    }
    let o = ((y as u32 * bw + x as u32) * 4) as usize;
    let af = a as f32 / 255.0;
    buf[o] = (r as f32 * af + buf[o] as f32 * (1.0 - af)) as u8;
    buf[o + 1] = (g as f32 * af + buf[o + 1] as f32 * (1.0 - af)) as u8;
    buf[o + 2] = (b as f32 * af + buf[o + 2] as f32 * (1.0 - af)) as u8;
    buf[o + 3] = 255;
}

fn stamp_disc(buf: &mut [u8], bw: u32, x: f32, y: f32, rad: f32, col: (u8, u8, u8)) {
    let r = rad.ceil() as i32;
    for dy in -r..=r {
        for dx in -r..=r {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d <= rad {
                let a = ((1.0 - d / rad) * 255.0) as u8;
                blend_px(buf, bw, x as i32 + dx, y as i32 + dy, col.0, col.1, col.2, a.max(60));
            }
        }
    }
}

/// Walk a polyline stamping discs; used for outlines.
fn stroke_poly(buf: &mut [u8], bw: u32, pts: &[(f32, f32)], rad: f32, col: (u8, u8, u8)) {
    if pts.is_empty() {
        return;
    }
    if pts.len() == 1 {
        stamp_disc(buf, bw, pts[0].0, pts[0].1, rad, col);
        return;
    }
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let n = (len / rad.max(0.5)).ceil() as i32;
        for i in 0..=n.max(1) {
            let t = i as f32 / n.max(1) as f32;
            stamp_disc(buf, bw, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, rad, col);
        }
    }
}

/// Dashed outline with animated offset (marching ants).
fn stroke_dashed(buf: &mut [u8], bw: u32, pts: &[(f32, f32)], rad: f32, col: (u8, u8, u8), offset: f32) {
    if pts.len() < 2 {
        return;
    }
    const DASH: f32 = 9.0;
    const GAP: f32 = 7.0;
    let period = DASH + GAP;
    let mut dist = -offset.rem_euclid(period);
    let mut acc: Vec<(f32, f32)> = vec![];
    let mut flush = |acc: &mut Vec<(f32, f32)>| {
        if acc.len() > 1 {
            stroke_poly(buf, bw, acc, rad, col);
        }
        acc.clear();
    };
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        let seg_len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let mut s = 0.0;
        while s < seg_len {
            let phase = (dist + s).rem_euclid(period);
            let step = 1.0f32.min(seg_len - s);
            if phase < DASH {
                let t0 = s / seg_len;
                let t1 = (s + step) / seg_len;
                let p0 = (x0 + (x1 - x0) * t0, y0 + (y1 - y0) * t0);
                let p1 = (x0 + (x1 - x0) * t1, y0 + (y1 - y0) * t1);
                if acc.is_empty() {
                    acc.push(p0);
                }
                acc.push(p1);
            } else if !acc.is_empty() {
                flush(&mut acc);
            }
            s += step;
        }
        dist += seg_len;
    }
    flush(&mut acc);
}

fn fill_poly(buf: &mut [u8], bw: u32, bh: u32, pts: &[(f32, f32)], col: (u8, u8, u8)) {
    if pts.len() < 3 {
        return;
    }
    let mut min_y = bh as i32;
    let mut max_y = 0i32;
    for &(_, y) in pts {
        min_y = min_y.min(y.floor() as i32);
        max_y = max_y.max(y.ceil() as i32);
    }
    let min_y = min_y.max(0);
    let max_y = max_y.min(bh as i32 - 1);
    let n = pts.len();
    for y in min_y..=max_y {
        let yf = y as f32 + 0.5;
        let mut xs: Vec<f32> = vec![];
        for i in 0..n {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % n];
            if (ay <= yf && by > yf) || (by <= yf && ay > yf) {
                xs.push(ax + (yf - ay) * (bx - ax) / (by - ay));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let x0 = pair[0].ceil() as i32;
            let x1 = pair[1].floor() as i32;
            for x in x0..=x1 {
                blend_px(buf, bw, x, y, col.0, col.1, col.2, 255);
            }
        }
    }
}

/// Partial polyline by arclength fraction (for draw-on animation).
fn partial_poly(pts: &[(f32, f32)], frac: f32) -> Vec<(f32, f32)> {
    if frac <= 0.0 || pts.is_empty() {
        return vec![];
    }
    if frac >= 1.0 {
        return pts.to_vec();
    }
    let total: f32 = pts.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).sum();
    let target = total * frac;
    let mut out = vec![pts[0]];
    let mut acc = 0.0;
    for w in pts.windows(2) {
        let seg = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        if acc + seg >= target {
            let t = if seg < 1e-6 { 0.0 } else { (target - acc) / seg };
            out.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            break;
        }
        out.push(w[1]);
        acc += seg;
    }
    out
}

pub fn point_in_poly(px: f32, py: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Render one animation frame. Returns (RGBA pixels, w, h).
pub fn render_frame(meta: &VectorMeta, polys: &[TracedPoly], col: (u8, u8, u8)) -> (Vec<u8>, u32, u32) {
    let scale = (512.0 / meta.w.max(meta.h)).min(2.0).max(0.5);
    let bw = (meta.w * scale).ceil() as u32;
    let bh = (meta.h * scale).ceil() as u32;
    let mut buf = vec![0u8; (bw * bh * 4) as usize];
    let t = meta.t * meta.speed;
    let xf = |p: (f32, f32)| (p.0 * scale, p.1 * scale);
    for poly in polys {
        let pts: Vec<(f32, f32)> = poly.pts.iter().map(|&p| xf(p)).collect();
        let cen = xf(poly.centroid);
        match meta.anim {
            AnimKind::Still => {
                if meta.filled {
                    fill_poly(&mut buf, bw, bh, &pts, col);
                } else {
                    stroke_poly(&mut buf, bw, &pts, 1.6 * scale, col);
                }
            }
            AnimKind::Draw => {
                // reveal with a short hold at full
                let cyc = (t * 0.45) % 1.25;
                let frac = (cyc / 1.0).min(1.0);
                let part = partial_poly(&pts, frac);
                stroke_poly(&mut buf, bw, &part, 1.8 * scale, col);
                // pen tip dot at the head
                if let Some(&head) = part.last() {
                    stamp_disc(&mut buf, bw, head.0, head.1, 2.6 * scale, col);
                }
            }
            AnimKind::Wave => {
                let wv: Vec<(f32, f32)> = pts
                    .iter()
                    .map(|&(x, y)| {
                        (x + 5.0 * scale * (0.045 * y + t * 3.5).sin(), y + 5.0 * scale * (0.04 * x + t * 3.0).cos())
                    })
                    .collect();
                if meta.filled {
                    fill_poly(&mut buf, bw, bh, &wv, col);
                } else {
                    stroke_poly(&mut buf, bw, &wv, 1.6 * scale, col);
                }
            }
            AnimKind::Spin => {
                let th = t * 1.4;
                let (s, c) = th.sin_cos();
                let rp: Vec<(f32, f32)> = pts
                    .iter()
                    .map(|&(x, y)| {
                        let (dx, dy) = (x - cen.0, y - cen.1);
                        (cen.0 + dx * c - dy * s, cen.1 + dx * s + dy * c)
                    })
                    .collect();
                if meta.filled {
                    fill_poly(&mut buf, bw, bh, &rp, col);
                } else {
                    stroke_poly(&mut buf, bw, &rp, 1.6 * scale, col);
                }
            }
            AnimKind::Pulse => {
                let k = 1.0 + 0.10 * (t * 2.8).sin();
                let pp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (cen.0 + (x - cen.0) * k, cen.1 + (y - cen.1) * k)).collect();
                if meta.filled {
                    fill_poly(&mut buf, bw, bh, &pp, col);
                } else {
                    stroke_poly(&mut buf, bw, &pp, 1.6 * scale, col);
                }
            }
            AnimKind::March => {
                if meta.filled {
                    fill_poly(&mut buf, bw, bh, &pts, col);
                }
                // marching ants in paper-ish light tone over the fill
                stroke_dashed(&mut buf, bw, &pts, 1.4 * scale, (245, 242, 234), t * 42.0);
            }
        }
    }
    (buf, bw, bh)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analyze_centroid_length() {
        let p = analyze(&[(0.0, 0.0), (4.0, 0.0), (4.0, 3.0)]);
        assert!((p.centroid.0 - 8.0 / 3.0).abs() < 0.01);
        assert!((p.area - 6.0).abs() < 0.01);
    }
    #[test]
    fn point_in_poly_square() {
        let sq = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(point_in_poly(5.0, 5.0, &sq));
        assert!(!point_in_poly(15.0, 5.0, &sq));
    }
    #[test]
    fn partial_poly_halves() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let part = partial_poly(&pts, 0.5);
        assert!((part.last().unwrap().0 - 10.0).abs() < 0.01);
        assert!((part.last().unwrap().1 - 0.0).abs() < 0.01);
        assert!(partial_poly(&pts, 0.0).is_empty());
        assert_eq!(partial_poly(&pts, 1.0).len(), 3);
    }
    #[test]
    fn fill_poly_paints_inside() {
        let mut buf = vec![0u8; (20 * 20 * 4) as usize];
        fill_poly(&mut buf, 20, 20, &[(5.0, 5.0), (15.0, 5.0), (15.0, 15.0), (5.0, 15.0)], (255, 0, 0));
        let o = ((10 * 20 + 10) * 4) as usize;
        assert_eq!(&buf[o..o + 3], &[255, 0, 0]);
        let o2 = ((2 * 20 + 2) * 4) as usize;
        assert_eq!(buf[o2 + 3], 0); // outside stays transparent
    }
    #[test]
    fn frame_renders_something() {
        let meta = VectorMeta {
            id: 1,
            polys: vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]],
            w: 12.0,
            h: 12.0,
            x: 0.0,
            y: 0.0,
            sw: 12.0,
            sh: 12.0,
            color: 0,
            filled: true,
            anim: AnimKind::Still,
            speed: 1.0,
            playing: false,
            t: 0.0,
        };
        let polys = rebuild_polys(&meta);
        let (buf, bw, bh) = render_frame(&meta, &polys, (255, 255, 255));
        assert!(bw > 0 && bh > 0);
        assert!(buf.iter().skip(3).step_by(4).any(|&a| a > 0));
    }
    #[test]
    fn vectorize_checker() {
        // 64px checker -> should trace the dark squares
        let mut px = vec![0u8; (64 * 64 * 4) as usize];
        for y in 0..64 {
            for x in 0..64 {
                let dark = (x / 16 + y / 16) % 2 == 0;
                let o = ((y * 64 + x) * 4) as usize;
                let v = if dark { 20 } else { 240 };
                px[o] = v;
                px[o + 1] = v;
                px[o + 2] = v;
                px[o + 3] = 255;
            }
        }
        let (polys, tw, th) = vectorize_rgba(&px, 64, 64).expect("trace");
        assert!(!polys.is_empty());
        assert!(tw <= 192 && th <= 192);
    }
}
