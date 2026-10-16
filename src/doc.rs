//! Document: strokes/images/texts/clips + undo/redo + save/load.
//! Pure data (no SDL) except via canvas types.

use crate::audio::AudioClip;
use crate::canvas::{ImageMeta, Rect, Stroke, TextMeta, VectorMeta, ToolSer};
use crate::palette::COLOR_SETS;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
enum Action {
    AddStroke { stroke: Stroke },
    DelStroke { stroke: Stroke, index: usize },
    AddImage { meta: ImageMeta },
    DelImage { meta: ImageMeta, index: usize },
    AddText { meta: TextMeta },
    DelText { meta: TextMeta, index: usize },
    AddVector { meta: VectorMeta },
    DelVector { meta: VectorMeta, index: usize },
    AddClip { clip: AudioClip },
    DelClip { clip: AudioClip, index: usize },
    MoveVector { id: u64, dx: f32, dy: f32 },
    MoveImage { id: u64, dx: f32, dy: f32 },
    MoveText { id: u64, dx: f32, dy: f32 },
    MoveClip { id: u64, dx: f32, dy: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DocData {
    pub strokes: Vec<Stroke>,
    pub images: Vec<ImageMeta>,
    pub texts: Vec<TextMeta>,
    #[serde(default)]
    pub vectors: Vec<VectorMeta>,
    #[serde(default)]
    pub clips: Vec<AudioClip>,
    #[serde(default = "default_bg")]
    pub bg: [u8; 3],
    #[serde(default = "default_palette")]
    pub palette: [[u8; 3]; 16],
    /// Active interchangeable color set (index into palette::COLOR_SETS).
    /// Legacy files without this field load as 0 ("Studio").
    #[serde(default)]
    pub color_set: usize,
}

pub fn default_bg() -> [u8; 3] {
    let c = crate::palette::PAPER;
    [c.r, c.g, c.b]
}

pub fn default_palette() -> [[u8; 3]; 16] {
    crate::palette::default_rgb()
}

pub struct Doc {
    pub data: DocData,
    next_id: u64,
    undo: Vec<Action>,
    redo: Vec<Action>,
    dirty: Vec<Rect>,     // paint-tile regions needing re-rasterization
    oil_dirty: Vec<Rect>, // oil-stroke regions needing the grease blur (subset of dirty)
}

impl Doc {
    pub fn new() -> Self {
        let mut data = DocData::default();
        // Derived Default would leave bg/palette as zeros (black); the serde
        // defaults only apply when loading JSON, not for fresh documents.
        data.bg = default_bg();
        data.palette = default_palette();
        Self {
            data,
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: Vec::new(),
            oil_dirty: Vec::new(),
        }
    }
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    /// Regions whose paint tiles must be re-rasterized. Drains.
    pub fn take_dirty(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.dirty)
    }

    /// Regions of freshly rasterized oil strokes that want the grease blur.
    /// Drained by the app's re-raster pass; bounded to these rects only.
    pub fn take_oil_dirty(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.oil_dirty)
    }

    fn mark_stroke_dirty(&mut self, s: &Stroke) {
        let b = s.bbox();
        self.dirty.push(b);
        if s.tool == ToolSer::OilStick {
            self.oil_dirty.push(b);
        }
    }

    /// Load a color set's 16 colors into the runtime palette (existing
    /// strokes recolor by index, same as the swatch editor). Persists via
    /// `DocData.color_set`.
    pub fn apply_color_set(&mut self, idx: usize) {
        let i = idx % COLOR_SETS.len();
        self.data.color_set = i;
        self.data.palette = COLOR_SETS[i].colors;
    }
    fn push_undo(&mut self, a: Action) {
        self.undo.push(a);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    // ---- strokes ----
    pub fn add_stroke(&mut self, s: Stroke) {
        self.mark_stroke_dirty(&s);
        self.data.strokes.push(s.clone());
        self.push_undo(Action::AddStroke { stroke: s });
    }
    pub fn delete_stroke(&mut self, id: u64) -> bool {
        if let Some(i) = self.data.strokes.iter().position(|s| s.id == id) {
            let s = self.data.strokes.remove(i);
            self.mark_stroke_dirty(&s);
            self.push_undo(Action::DelStroke { stroke: s, index: i });
            return true;
        }
        false
    }

    // ---- images ----
    pub fn add_image(&mut self, m: ImageMeta) {
        self.data.images.push(m.clone());
        self.push_undo(Action::AddImage { meta: m });
    }
    pub fn delete_image(&mut self, id: u64) -> Option<ImageMeta> {
        let i = self.data.images.iter().position(|m| m.id == id)?;
        let m = self.data.images.remove(i);
        self.push_undo(Action::DelImage { meta: m.clone(), index: i });
        Some(m)
    }
    pub fn move_image(&mut self, id: u64, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if let Some(m) = self.data.images.iter_mut().find(|m| m.id == id) {
            m.x += dx;
            m.y += dy;
            self.push_undo(Action::MoveImage { id, dx, dy });
        }
    }

    // ---- texts ----
    pub fn add_text(&mut self, m: TextMeta) {
        self.data.texts.push(m.clone());
        self.push_undo(Action::AddText { meta: m });
    }
    pub fn delete_text(&mut self, id: u64) -> Option<TextMeta> {
        let i = self.data.texts.iter().position(|m| m.id == id)?;
        let m = self.data.texts.remove(i);
        self.push_undo(Action::DelText { meta: m.clone(), index: i });
        Some(m)
    }
    pub fn move_text(&mut self, id: u64, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if let Some(m) = self.data.texts.iter_mut().find(|m| m.id == id) {
            m.x += dx;
            m.y += dy;
            self.push_undo(Action::MoveText { id, dx, dy });
        }
    }
    // ---- vectors ----
    pub fn add_vector(&mut self, m: VectorMeta) {
        self.data.vectors.push(m.clone());
        self.push_undo(Action::AddVector { meta: m });
    }
    pub fn delete_vector(&mut self, id: u64) -> Option<VectorMeta> {
        let i = self.data.vectors.iter().position(|m| m.id == id)?;
        let m = self.data.vectors.remove(i);
        self.push_undo(Action::DelVector { meta: m.clone(), index: i });
        Some(m)
    }
    pub fn move_vector(&mut self, id: u64, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if let Some(m) = self.data.vectors.iter_mut().find(|m| m.id == id) {
            m.x += dx;
            m.y += dy;
            self.push_undo(Action::MoveVector { id, dx, dy });
        }
    }
    // ---- clips ----
    pub fn add_clip(&mut self, c: AudioClip) {
        self.data.clips.push(c.clone());
        self.push_undo(Action::AddClip { clip: c });
    }
    pub fn delete_clip(&mut self, id: u64) -> Option<AudioClip> {
        let i = self.data.clips.iter().position(|c| c.id == id)?;
        let c = self.data.clips.remove(i);
        self.push_undo(Action::DelClip { clip: c.clone(), index: i });
        Some(c)
    }
    pub fn move_clip(&mut self, id: u64, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        if let Some(c) = self.data.clips.iter_mut().find(|c| c.id == id) {
            c.x += dx as f64;
            c.y += dy as f64;
            self.push_undo(Action::MoveClip { id, dx, dy });
        }
    }

    // ---- undo/redo ----
    /// Paint-tile regions are pushed to the dirty list; the app re-syncs
    /// image/text GPU textures from doc data afterwards.
    pub fn undo(&mut self) -> Option<()> {
        let a = self.undo.pop()?;
        let fx = self.apply_inverse(a);
        self.redo.push(fx.1);
        Some(fx.0)
    }
    pub fn redo(&mut self) -> Option<()> {
        let a = self.redo.pop()?;
        let fx = self.apply_forward(a);
        self.undo.push(fx.1);
        Some(fx.0)
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn apply_inverse(&mut self, a: Action) -> ((), Action) {
        match a {
            Action::AddStroke { stroke } => {
                let id = stroke.id;
                self.data.strokes.retain(|s| s.id != id);
                self.mark_stroke_dirty(&stroke);
                (
                    (),
                    Action::AddStroke { stroke },
                )
            }
            Action::DelStroke { stroke, index } => {
                let i = index.min(self.data.strokes.len());
                self.data.strokes.insert(i, stroke.clone());
                self.mark_stroke_dirty(&stroke);
                (
                    (),
                    Action::DelStroke { stroke, index: i },
                )
            }
            Action::AddImage { meta } => {
                let id = meta.id;
                self.data.images.retain(|m| m.id != id);
                (
                    (),
                    Action::AddImage { meta },
                )
            }
            Action::DelImage { meta, index } => {
                let i = index.min(self.data.images.len());
                self.data.images.insert(i, meta.clone());
                (
                    (),
                    Action::DelImage { meta, index: i },
                )
            }
            Action::AddText { meta } => {
                let id = meta.id;
                self.data.texts.retain(|m| m.id != id);
                (
                    (),
                    Action::AddText { meta },
                )
            }
            Action::DelText { meta, index } => {
                let i = index.min(self.data.texts.len());
                self.data.texts.insert(i, meta.clone());
                (
                    (),
                    Action::DelText { meta, index: i },
                )
            }
            Action::MoveImage { id, dx, dy } => {
                if let Some(m) = self.data.images.iter_mut().find(|m| m.id == id) {
                    m.x -= dx;
                    m.y -= dy;
                }
                (
                    (),
                    Action::MoveImage { id, dx, dy },
                )
            }
            Action::MoveText { id, dx, dy } => {
                if let Some(m) = self.data.texts.iter_mut().find(|m| m.id == id) {
                    m.x -= dx;
                    m.y -= dy;
                }
                (
                    (),
                    Action::MoveText { id, dx, dy },
                )
            }
            Action::AddVector { meta } => {
                let id = meta.id;
                self.data.vectors.retain(|m| m.id != id);
                (
                    (),
                    Action::AddVector { meta },
                )
            }
            Action::DelVector { meta, index } => {
                let i = index.min(self.data.vectors.len());
                self.data.vectors.insert(i, meta.clone());
                (
                    (),
                    Action::DelVector { meta, index: i },
                )
            }
            Action::MoveVector { id, dx, dy } => {
                if let Some(m) = self.data.vectors.iter_mut().find(|m| m.id == id) {
                    m.x -= dx;
                    m.y -= dy;
                }
                (
                    (),
                    Action::MoveVector { id, dx, dy },
                )
            }
            Action::AddClip { clip } => {
                let id = clip.id;
                self.data.clips.retain(|c| c.id != id);
                (
                    (),
                    Action::AddClip { clip },
                )
            }
            Action::DelClip { clip, index } => {
                let i = index.min(self.data.clips.len());
                self.data.clips.insert(i, clip.clone());
                (
                    (),
                    Action::DelClip { clip, index: i },
                )
            }
            Action::MoveClip { id, dx, dy } => {
                if let Some(c) = self.data.clips.iter_mut().find(|c| c.id == id) {
                    c.x -= dx as f64;
                    c.y -= dy as f64;
                }
                (
                    (),
                    Action::MoveClip { id, dx, dy },
                )
            }
        }
    }

    fn apply_forward(&mut self, a: Action) -> ((), Action) {
        // forward of inverse(a) == a again; reuse inverse since every
        // action here is its own inverse pair partner
        match a {
            Action::AddStroke { stroke } => {
                self.data.strokes.push(stroke.clone());
                self.mark_stroke_dirty(&stroke);
                let fx = ();
                (fx, Action::AddStroke { stroke })
            }
            Action::DelStroke { stroke, index } => {
                let id = stroke.id;
                self.data.strokes.retain(|s| s.id != id);
                self.mark_stroke_dirty(&stroke);
                let fx = ();
                (fx, Action::DelStroke { stroke, index })
            }
            Action::AddImage { meta } => {
                self.data.images.push(meta.clone());
                let fx = ();
                (fx, Action::AddImage { meta })
            }
            Action::DelImage { meta, index } => {
                let id = meta.id;
                self.data.images.retain(|m| m.id != id);
                let fx = ();
                (fx, Action::DelImage { meta, index })
            }
            Action::AddText { meta } => {
                self.data.texts.push(meta.clone());
                let fx = ();
                (fx, Action::AddText { meta })
            }
            Action::DelText { meta, index } => {
                let id = meta.id;
                self.data.texts.retain(|m| m.id != id);
                let fx = ();
                (fx, Action::DelText { meta, index })
            }
            Action::MoveImage { id, dx, dy } => {
                if let Some(m) = self.data.images.iter_mut().find(|m| m.id == id) {
                    m.x += dx;
                    m.y += dy;
                }
                let fx = ();
                (fx, Action::MoveImage { id, dx, dy })
            }
            Action::MoveText { id, dx, dy } => {
                if let Some(m) = self.data.texts.iter_mut().find(|m| m.id == id) {
                    m.x += dx;
                    m.y += dy;
                }
                let fx = ();
                (fx, Action::MoveText { id, dx, dy })
            }
            Action::AddVector { meta } => {
                self.data.vectors.push(meta.clone());
                ((), Action::AddVector { meta })
            }
            Action::DelVector { meta, index } => {
                let id = meta.id;
                self.data.vectors.retain(|m| m.id != id);
                ((), Action::DelVector { meta, index })
            }
            Action::MoveVector { id, dx, dy } => {
                if let Some(m) = self.data.vectors.iter_mut().find(|m| m.id == id) {
                    m.x += dx;
                    m.y += dy;
                }
                ((), Action::MoveVector { id, dx, dy })
            }
            Action::AddClip { clip } => {
                self.data.clips.push(clip.clone());
                ((), Action::AddClip { clip })
            }
            Action::DelClip { clip, index } => {
                let id = clip.id;
                self.data.clips.retain(|c| c.id != id);
                ((), Action::DelClip { clip, index })
            }
            Action::MoveClip { id, dx, dy } => {
                if let Some(c) = self.data.clips.iter_mut().find(|c| c.id == id) {
                    c.x += dx as f64;
                    c.y += dy as f64;
                }
                ((), Action::MoveClip { id, dx, dy })
            }
        }
    }

    // ---- persistence ----
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&self.data).map_err(|e| e.to_string())
    }
    pub fn from_json(s: &str) -> Result<DocData, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
    pub fn load_data(&mut self, data: DocData) {
        self.data = data;
        self.next_id = self
            .data
            .strokes
            .iter()
            .map(|s| s.id)
            .chain(self.data.images.iter().map(|m| m.id))
            .chain(self.data.texts.iter().map(|m| m.id))
            .chain(self.data.vectors.iter().map(|m| m.id))
            .chain(self.data.clips.iter().map(|c| c.id))
            .max()
            .unwrap_or(0)
            + 1;
        self.undo.clear();
        self.redo.clear();
        // Mark the actual content dirty. A giant/infinite rect here would make
        // tiles_in_rect() try to allocate terabytes and crash the app.
        if let Some(b) = self.content_bbox() {
            self.dirty.push(b.expand(200.0));
        }
        // Loaded oil strokes get the grease blur on their first raster too.
        for s in &self.data.strokes {
            if s.tool == ToolSer::OilStick {
                self.oil_dirty.push(s.bbox());
            }
        }
    }

    pub fn content_bbox(&self) -> Option<Rect> {
        let mut r: Option<Rect> = None;
        let mut acc = |b: Rect| {
            r = Some(match r {
                Some(cur) => cur.union(&b),
                None => b,
            })
        };
        for s in &self.data.strokes {
            acc(s.bbox());
        }
        for m in &self.data.images {
            acc(m.rect());
        }
        for t in &self.data.texts {
            acc(t.rect());
        }
        for v in &self.data.vectors {
            acc(v.rect());
        }
        for c in &self.data.clips {
            acc(c.rect());
        }
        r
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{b64encode, mulaw_encode};
    use crate::canvas::ToolSer;

    fn clip(id: u64) -> AudioClip {
        let ulaw: Vec<u8> = (0..1200).map(|i| mulaw_encode(i as i16 - 600)).collect();
        AudioClip { id, x: 10.0, y: 20.0, seconds: 0.1, data: b64encode(&ulaw) }
    }
    #[test]
    fn clip_undo_redo() {
        let mut d = Doc::new();
        d.add_clip(clip(9));
        assert_eq!(d.data.clips.len(), 1);
        d.undo();
        assert!(d.data.clips.is_empty());
        d.redo();
        assert_eq!(d.data.clips.len(), 1);
        assert_eq!(d.data.clips[0].id, 9);
    }
    #[test]
    fn clip_delete_move_undo() {
        let mut d = Doc::new();
        d.add_clip(clip(9));
        d.add_clip(clip(10));
        d.delete_clip(9);
        assert_eq!(d.data.clips.len(), 1);
        assert_eq!(d.data.clips[0].id, 10);
        d.undo();
        assert_eq!(d.data.clips.len(), 2);
        assert_eq!(d.data.clips[0].id, 9);
        d.move_clip(10, 5.0, -2.0);
        assert!((d.data.clips[1].x - 15.0).abs() < 1e-9);
        d.undo();
        assert!((d.data.clips[1].x - 10.0).abs() < 1e-9);
    }
    #[test]
    fn clip_json_roundtrip() {
        let mut d = Doc::new();
        d.add_clip(clip(11));
        let j = d.to_json().unwrap();
        assert!(j.contains("\"clips\""), "clips must persist in JSON");
        let data = Doc::from_json(&j).unwrap();
        assert_eq!(data.clips.len(), 1);
        assert_eq!(data.clips[0].id, 11);
        assert_eq!(data.clips[0].data.len(), clip(11).data.len());
        // old files without clips still load
        let legacy = Doc::from_json(r#"{"strokes":[],"images":[],"texts":[]}"#).unwrap();
        assert!(legacy.clips.is_empty());
    }
    #[test]
    fn clip_in_content_bbox() {
        let mut d = Doc::new();
        d.add_clip(clip(12));
        let b = d.content_bbox().expect("bbox");
        assert!(b.x0 < 10.0 && b.x1 > 10.0, "clip must widen the bbox");
    }

    fn stroke(id: u64) -> Stroke {
        Stroke { id, tool: ToolSer::Pen, color: 0, size: 4.0, points: vec![(0.0, 0.0), (5.0, 5.0)] }
    }
    fn oil_stroke(id: u64) -> Stroke {
        Stroke { id, tool: ToolSer::OilStick, color: 3, size: 30.0, points: vec![(0.0, 0.0), (40.0, 10.0)] }
    }
    #[test]
    fn color_set_switch_and_roundtrip() {
        let mut d = Doc::new();
        assert_eq!(d.data.color_set, 0);
        d.apply_color_set(2);
        assert_eq!(d.data.color_set, 2);
        assert_eq!(d.data.palette, COLOR_SETS[2].colors);
        assert_eq!(d.data.palette[0], [0xff, 0x00, 0x6e]); // Pop Hot Pink
        // existing strokes recolor by index (same semantics as swatch editor)
        d.add_stroke(stroke(1));
        d.apply_color_set(4);
        assert_eq!(d.data.strokes[0].color, 0);
        // wraps
        d.apply_color_set(99);
        assert_eq!(d.data.color_set, 99 % COLOR_SETS.len());
        // persists through save/load
        d.apply_color_set(3);
        let j = d.to_json().unwrap();
        assert!(j.contains("\"color_set\""), "set index must persist in JSON");
        let data = Doc::from_json(&j).unwrap();
        assert_eq!(data.color_set, 3);
        assert_eq!(data.palette, COLOR_SETS[3].colors);
        // legacy files without the field load as Studio
        let legacy = Doc::from_json(r#"{"strokes":[],"images":[],"texts":[]}"#).unwrap();
        assert_eq!(legacy.color_set, 0);
        assert_eq!(legacy.palette, COLOR_SETS[0].colors);
    }
    #[test]
    fn oil_dirty_tracks_oil_strokes_only() {
        let mut d = Doc::new();
        d.add_stroke(stroke(1)); // pen
        assert!(d.take_oil_dirty().is_empty(), "pen must not mark oil_dirty");
        d.add_stroke(oil_stroke(2));
        let od = d.take_oil_dirty();
        assert_eq!(od.len(), 1, "oil stroke must mark oil_dirty");
        assert!(od[0].x1 > 30.0, "oil dirty rect must cover the stroke");
        // delete + undo/redo also track
        d.delete_stroke(2);
        assert_eq!(d.take_oil_dirty().len(), 1);
        d.undo();
        assert_eq!(d.take_oil_dirty().len(), 1);
        d.redo();
        assert_eq!(d.take_oil_dirty().len(), 1);
    }
    #[test]
    fn load_data_marks_oil_dirty() {
        let mut d = Doc::new();
        d.add_stroke(oil_stroke(7));
        let j = d.to_json().unwrap();
        let data = Doc::from_json(&j).unwrap();
        let mut d2 = Doc::new();
        d2.load_data(data);
        assert_eq!(d2.take_oil_dirty().len(), 1, "loaded oil strokes blur on first raster");
    }
    #[test]
    fn undo_redo_stroke() {
        let mut d = Doc::new();
        d.add_stroke(stroke(1));
        assert_eq!(d.data.strokes.len(), 1);
        d.undo();
        assert!(d.data.strokes.is_empty());
        d.redo();
        assert_eq!(d.data.strokes.len(), 1);
        assert!(!d.take_dirty().is_empty());
    }
    #[test]
    fn undo_delete_restores() {
        let mut d = Doc::new();
        d.add_stroke(stroke(1));
        d.add_stroke(stroke(2));
        d.delete_stroke(1);
        assert_eq!(d.data.strokes.len(), 1);
        d.undo();
        assert_eq!(d.data.strokes.len(), 2);
        assert_eq!(d.data.strokes[0].id, 1);
    }
    #[test]
    fn move_undo() {
        let mut d = Doc::new();
        d.add_image(ImageMeta { id: 7, path: "x".into(), x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        d.move_image(7, 5.0, -3.0);
        assert_eq!(d.data.images[0].x, 5.0);
        d.undo();
        assert_eq!(d.data.images[0].x, 0.0);
        d.redo();
        assert_eq!(d.data.images[0].x, 5.0);
    }
    #[test]
    fn redo_cleared_on_new_action() {
        let mut d = Doc::new();
        d.add_stroke(stroke(1));
        d.undo();
        d.add_stroke(stroke(2));
        assert!(!d.can_redo());
    }
    #[test]
    fn json_roundtrip() {
        let mut d = Doc::new();
        d.add_stroke(stroke(3));
        let j = d.to_json().unwrap();
        let data = Doc::from_json(&j).unwrap();
        assert_eq!(data.strokes.len(), 1);
        assert_eq!(data.strokes[0].id, 3);
    }
    #[test]
    fn fresh_doc_has_builtin_palette() {
        // Regression: the derived Default left palette as all-black, so every
        // swatch and stroke came out black on a fresh canvas.
        let d = Doc::new();
        assert_eq!(d.data.bg, default_bg());
        assert_eq!(d.data.palette, default_palette());
        assert!(d.data.palette.iter().any(|p| *p != [0, 0, 0]), "palette must not be all black");
        // serde defaults must agree with fresh-doc defaults
        let from_json = Doc::from_json(r#"{"strokes":[],"images":[],"texts":[]}"#).unwrap();
        assert_eq!(from_json.bg, default_bg());
        assert_eq!(from_json.palette, default_palette());
    }
    #[test]
    fn load_data_dirty_rect_is_bounded() {
        // Regression: load_data used to push a (-1e9..1e9) dirty rect, which
        // made tiles_in_rect() try to allocate ~122TB and crash the app on
        // every Ctrl+O.
        let mut d = Doc::new();
        d.add_stroke(stroke(4));
        let j = d.to_json().unwrap();
        let data = Doc::from_json(&j).unwrap();
        let mut d2 = Doc::new();
        d2.load_data(data);
        let dirty = d2.take_dirty();
        assert!(!dirty.is_empty());
        for r in &dirty {
            assert!((r.x1 - r.x0) < 100000.0, "dirty rect too wide: {:?}", r.x1 - r.x0);
            assert!((r.y1 - r.y0) < 100000.0, "dirty rect too tall: {:?}", r.y1 - r.y0);
        }
    }
}
