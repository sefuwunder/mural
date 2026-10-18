//! Canvas model: camera, drawable items, hit-testing. No SDL here
//! except Color (pure data otherwise, so save/load and tests stay clean).

use crate::brush::Tool;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub cx: f32, // world coords at screen center
    pub cy: f32,
    pub zoom: f32, // screen px per world px
}

impl Camera {
    pub fn new() -> Self {
        Self { cx: 0.0, cy: 0.0, zoom: 1.0 }
    }
    pub fn world_to_screen(&self, wx: f32, wy: f32, sw: f32, sh: f32) -> (f32, f32) {
        ((wx - self.cx) * self.zoom + sw / 2.0, (wy - self.cy) * self.zoom + sh / 2.0)
    }
    pub fn screen_to_world(&self, sx: f32, sy: f32, sw: f32, sh: f32) -> (f32, f32) {
        ((sx - sw / 2.0) / self.zoom + self.cx, (sy - sh / 2.0) / self.zoom + self.cy)
    }
    /// Zoom keeping the world point under the cursor fixed.
    pub fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32, sw: f32, sh: f32) {
        let (wx, wy) = self.screen_to_world(sx, sy, sw, sh);
        self.zoom = (self.zoom * factor).clamp(0.1, 8.0);
        self.cx = wx - (sx - sw / 2.0) / self.zoom;
        self.cy = wy - (sy - sh / 2.0) / self.zoom;
    }
    /// Visible world rect.
    pub fn view_rect(&self, sw: f32, sh: f32) -> Rect {
        let (x0, y0) = self.screen_to_world(0.0, 0.0, sw, sh);
        let (x1, y1) = self.screen_to_world(sw, sh, sw, sh);
        Rect { x0, y0, x1, y1 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 <= o.x1 && self.x1 >= o.x0 && self.y0 <= o.y1 && self.y1 >= o.y0
    }
    pub fn expand(&self, m: f32) -> Rect {
        Rect::new(self.x0 - m, self.y0 - m, self.x1 + m, self.y1 + m)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(
            self.x0.min(o.x0),
            self.y0.min(o.y0),
            self.x1.max(o.x1),
            self.y1.max(o.y1),
        )
    }
    pub fn w(&self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }
    pub fn h(&self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stroke {
    pub id: u64,
    pub tool: ToolSer,
    pub color: usize, // palette index
    pub size: f32,    // world px diameter
    pub points: Vec<(f32, f32)>, // world coords
    /// Seconds since stroke start, parallel to `points`. Recorded for new
    /// strokes; empty for legacy files (replay falls back to steady pace).
    #[serde(default)]
    pub times: Vec<f32>,
}

// serde-friendly mirror of brush::Tool (keeps brush SDL-free-ish)
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolSer {
    Pencil,
    Pen,
    Marker,
    OilStick,
    Charcoal,
    Smudge,
}

impl From<Tool> for ToolSer {
    fn from(t: Tool) -> Self {
        match t {
            Tool::Pencil => ToolSer::Pencil,
            Tool::Pen => ToolSer::Pen,
            Tool::Marker => ToolSer::Marker,
            Tool::OilStick => ToolSer::OilStick,
            Tool::Charcoal => ToolSer::Charcoal,
            Tool::Smudge => ToolSer::Smudge,
        }
    }
}
impl From<ToolSer> for Tool {
    fn from(t: ToolSer) -> Self {
        match t {
            ToolSer::Pencil => Tool::Pencil,
            ToolSer::Pen => Tool::Pen,
            ToolSer::Marker => Tool::Marker,
            ToolSer::OilStick => Tool::OilStick,
            ToolSer::Charcoal => Tool::Charcoal,
            ToolSer::Smudge => Tool::Smudge,
        }
    }
}

impl Stroke {
    pub fn bbox(&self) -> Rect {
        let (mut x0, mut y0) = (f32::INFINITY, f32::INFINITY);
        let (mut x1, mut y1) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in &self.points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        if x0.is_infinite() {
            x0 = 0.0;
            y0 = 0.0;
            x1 = 0.0;
            y1 = 0.0;
        }
        Rect::new(x0, y0, x1, y1).expand(self.size)
    }
    /// distance from world point to the polyline
    pub fn dist_to(&self, wx: f32, wy: f32) -> f32 {
        let mut best = f32::INFINITY;
        if self.points.len() == 1 {
            let (x, y) = self.points[0];
            return ((wx - x).powi(2) + (wy - y).powi(2)).sqrt();
        }
        for w in self.points.windows(2) {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            best = best.min(dist_point_seg(wx, wy, ax, ay, bx, by));
        }
        best
    }
}

fn dist_point_seg(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-9 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0)
    };
    let cx = ax + dx * t;
    let cy = ay + dy * t;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageMeta {
    pub id: u64,
    pub path: String,
    pub x: f32, // top-left, world
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ImageMeta {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextMeta {
    pub id: u64,
    pub text: String,
    pub x: f32, // baseline-ish top-left, world
    pub y: f32,
    pub size: f32, // world px cap height-ish
    pub color: usize,
    pub w: f32, // measured, world px
    pub h: f32,
    #[serde(default)]
    pub box_w: f32, // >0: bounded text box (world px); 0 = point text
    #[serde(default)]
    pub box_h: f32,
    #[serde(default)]
    pub align: u8, // 0 left, 1 center, 2 right
    #[serde(default)]
    pub shape: u8, // 0 none, 1 rect, 2 ellipse
}

impl TextMeta {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.w, self.y + self.h)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelKind {
    Stroke,
    Image,
    Text,
    Vector,
    Clip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnimKind {
    Still,
    Draw,
    Wave,
    Spin,
    Pulse,
    March,
}

impl AnimKind {
    pub fn all() -> [AnimKind; 6] {
        [AnimKind::Still, AnimKind::Draw, AnimKind::Wave, AnimKind::Spin, AnimKind::Pulse, AnimKind::March]
    }
    pub fn label(self) -> &'static str {
        match self {
            AnimKind::Still => "Still",
            AnimKind::Draw => "Draw",
            AnimKind::Wave => "Wave",
            AnimKind::Spin => "Spin",
            AnimKind::Pulse => "Pulse",
            AnimKind::March => "March",
        }
    }
    pub fn animated(self) -> bool {
        !matches!(self, AnimKind::Still)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorMeta {
    pub id: u64,
    pub polys: Vec<Vec<(f32, f32)>>, // local px coords (trace resolution)
    pub w: f32,
    pub h: f32, // local px dims
    pub x: f32,
    pub y: f32, // world top-left
    pub sw: f32,
    pub sh: f32, // world size
    pub color: usize,
    pub filled: bool,
    pub anim: AnimKind,
    pub speed: f32,
    pub playing: bool,
    #[serde(default)]
    pub t: f32,
}

impl VectorMeta {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.sw, self.y + self.sh)
    }
}

/// Merge overlapping rects into their unions, so a region blurred once per
/// pass never gets double-blurred when several dirty entries cover it
/// (e.g. add + undo + redo of the same oil stroke).
pub fn union_rects(rs: Vec<Rect>) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for r in rs {
        let mut cur = r;
        let mut i = 0;
        while i < out.len() {
            if out[i].intersects(&cur) {
                let m = out.remove(i);
                cur = cur.union(&m);
            } else {
                i += 1;
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_roundtrip() {
        let mut c = Camera::new();
        c.cx = 100.0;
        c.cy = -50.0;
        c.zoom = 2.0;
        let (sx, sy) = c.world_to_screen(150.0, 0.0, 800.0, 600.0);
        let (wx, wy) = c.screen_to_world(sx, sy, 800.0, 600.0);
        assert!((wx - 150.0).abs() < 0.001 && (wy - 0.0).abs() < 0.001);
    }
    #[test]
    fn zoom_at_keeps_cursor_anchor() {
        let mut c = Camera::new();
        let (wx, wy) = c.screen_to_world(600.0, 400.0, 800.0, 600.0);
        c.zoom_at(2.0, 600.0, 400.0, 800.0, 600.0);
        let (wx2, wy2) = c.screen_to_world(600.0, 400.0, 800.0, 600.0);
        assert!((wx - wx2).abs() < 0.01 && (wy - wy2).abs() < 0.01);
        assert!((c.zoom - 2.0).abs() < 0.001);
    }
    #[test]
    fn union_rects_merges_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 15.0, 15.0);
        let c = Rect::new(100.0, 100.0, 110.0, 110.0);
        let out = union_rects(vec![a, b, c, a]);
        assert_eq!(out.len(), 2, "overlapping+duplicate rects must merge");
        let m = out.iter().find(|r| r.x0 == 0.0).unwrap();
        assert_eq!((m.x1, m.y1), (15.0, 15.0));
        assert!(out.iter().any(|r| r.x0 == 100.0));
        assert!(union_rects(vec![]).is_empty());
    }
    #[test]
    fn zoom_clamped() {
        let mut c = Camera::new();
        c.zoom_at(1000.0, 0.0, 0.0, 800.0, 600.0);
        assert!(c.zoom <= 8.0);
        c.zoom_at(0.0001, 0.0, 0.0, 800.0, 600.0);
        assert!(c.zoom >= 0.1);
    }
    #[test]
    fn stroke_bbox_and_dist() {
        let s = Stroke {
            id: 1,
            tool: ToolSer::Pen,
            color: 0,
            size: 4.0,
            points: vec![(0.0, 0.0), (10.0, 0.0)],
            times: vec![],
        };
        let b = s.bbox();
        assert!(b.x0 <= -4.0 && b.x1 >= 14.0);
        assert!(s.dist_to(5.0, 3.0) < 3.01);
        assert!(s.dist_to(5.0, 30.0) > 20.0);
    }
    #[test]
    fn rect_ops() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 15.0, 15.0);
        assert!(a.intersects(&b));
        assert!(a.contains(5.0, 5.0));
        assert!(!a.contains(11.0, 5.0));
        let u = a.union(&b);
        assert_eq!((u.x1, u.y1), (15.0, 15.0));
    }
    #[test]
    fn tool_serde_roundtrip() {
        let t = ToolSer::Marker;
        assert_eq!(Tool::from(t), Tool::Marker);
        assert_eq!(ToolSer::from(Tool::OilStick), ToolSer::OilStick);
    }
}
