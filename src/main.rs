//! mural — infinite canvas drawing app.
//! marker / pencil / pen / oil stick, 16 colors, image import, text fields.

mod brush;
mod canvas;
mod doc;
mod palette;
mod vector;

use brush::{blit_stamp, walk_segment, TipCache, Tool};
use canvas::{AnimKind, Camera, ImageMeta, Rect, SelKind, Stroke, TextMeta, VectorMeta};
use doc::Doc;
use vector::{point_in_poly, rebuild_polys, render_frame, vectorize_rgba, TracedPoly};
use palette::{COUNT as PALETTE_COUNT, PALETTE, PAPER, PAPER_DOT};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::mouse::MouseButton;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, Canvas, Texture, TextureCreator};
use sdl2::ttf::Font;
use sdl2::video::{Window, WindowContext};
use sdl2::Sdl;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const TILE_PX: f32 = 512.0;
const TOP_H: i32 = 44;
const TOOL_W: i32 = 60;
const BOT_H: i32 = 76;

const CHROME: Color = Color::RGB(0x1e, 0x1e, 0x24);
const CHROME2: Color = Color::RGB(0x2a, 0x2a, 0x32);
const INK: Color = Color::RGB(0xe8, 0xe8, 0xee);
const DIM: Color = Color::RGB(0x9a, 0x9a, 0xa6);
const ACCENT: Color = Color::RGB(0xff, 0xb0, 0x00);
const SEL: Color = Color::RGB(0x4a, 0x9e, 0xd6);

#[derive(PartialEq, Clone, Copy, Debug)]
enum UiTool {
    Select,
    Pan,
    Pencil,
    Pen,
    Marker,
    OilStick,
    Text,
}
impl UiTool {
    fn draw_tool(self) -> Option<Tool> {
        match self {
            UiTool::Pencil => Some(Tool::Pencil),
            UiTool::Pen => Some(Tool::Pen),
            UiTool::Marker => Some(Tool::Marker),
            UiTool::OilStick => Some(Tool::OilStick),
            _ => None,
        }
    }
    fn shortcut(self) -> &'static str {
        match self {
            UiTool::Select => "V",
            UiTool::Pan => "H",
            UiTool::Pencil => "P",
            UiTool::Pen => "N",
            UiTool::Marker => "M",
            UiTool::OilStick => "O",
            UiTool::Text => "T",
        }
    }
    fn label(self) -> &'static str {
        match self {
            UiTool::Select => "Move",
            UiTool::Pan => "Pan",
            UiTool::Pencil => "Pencil",
            UiTool::Pen => "Pen",
            UiTool::Marker => "Marker",
            UiTool::OilStick => "Oil",
            UiTool::Text => "Text",
        }
    }
}
const TOOLS: [UiTool; 7] = [
    UiTool::Select,
    UiTool::Pan,
    UiTool::Pencil,
    UiTool::Pen,
    UiTool::Marker,
    UiTool::OilStick,
    UiTool::Text,
];

fn tool_index(t: UiTool) -> usize {
    match t {
        UiTool::Pencil => 0,
        UiTool::Pen => 1,
        UiTool::Marker => 2,
        UiTool::OilStick => 3,
        _ => 0,
    }
}

struct ActiveStroke {
    tool: Tool,
    color: usize,
    size: f32,
    points: Vec<(f32, f32)>, // world
    last_sx: f32,
    last_sy: f32,
}

struct TextEdit {
    x: f32,
    y: f32, // world
    buf: String,
    existing: Option<u64>, // text id being re-edited
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    None,
    Pan,
    MoveItem,
    Stroke,
    Slider,
    Speed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelAction {
    Anim(AnimKind),
    PlayPause,
    Fill,
}

struct App<'tc, 'ttf> {
    canvas: Canvas<Window>,
    tc: &'tc TextureCreator<WindowContext>,
    font_big: Font<'ttf, 'static>,
    font_ui: Font<'ttf, 'static>,
    doc: Doc,
    cam: Camera,
    tiles: HashMap<(i32, i32), Texture<'tc>>,
    tips: TipCache<'tc>,
    img_tex: HashMap<u64, Texture<'tc>>,
    text_tex: HashMap<u64, Texture<'tc>>,
    label_cache: HashMap<String, Texture<'tc>>,
    vec_data: HashMap<u64, Vec<TracedPoly>>,
    vec_tex: HashMap<u64, (Texture<'tc>, u64)>,
    panel_hits: Vec<(SdlRect, PanelAction)>,
    panel_speed: Option<SdlRect>,
    last_frame: Instant,
    tool: UiTool,
    color_idx: usize,
    sizes: [f32; 4],
    selection: Option<(SelKind, u64)>,
    drag: DragKind,
    // drag state
    drag_sx: f32,
    drag_sy: f32,
    pan_cx: f32,
    pan_cy: f32,
    move_kind: Option<SelKind>,
    move_id: u64,
    move_ox: f32,
    move_oy: f32,
    move_dx: f32,
    move_dy: f32,
    active_stroke: Option<ActiveStroke>,
    space_down: bool,
    text_edit: Option<TextEdit>,
    import_buf: Option<String>,
    show_help: bool,
    toast: Option<(String, Instant)>,
    start: Instant,
    last_click_at: Option<Instant>,
    last_click_item: Option<(SelKind, u64)>,
    win_w: u32,
    win_h: u32,
}

fn tiles_in_rect(r: &Rect) -> Vec<(i32, i32)> {
    // Clamp the tile range: a huge rect (e.g. from a programming slip) must
    // never turn into a terabytes-large Vec and take the app down.
    const MAX_TILES_PER_AXIS: i32 = 4000; // 4000^2 tiles x 512px = plenty
    let x0 = (r.x0 / TILE_PX).floor() as i32;
    let y0 = (r.y0 / TILE_PX).floor() as i32;
    let x1 = (r.x1 / TILE_PX).floor() as i32;
    let y1 = (r.y1 / TILE_PX).floor() as i32;
    if x1 - x0 > MAX_TILES_PER_AXIS || y1 - y0 > MAX_TILES_PER_AXIS {
        return vec![];
    }
    let mut out = vec![];
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            out.push((tx, ty));
        }
    }
    out
}

fn find_font() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MURAL_FONT") {
        if Path::new(&p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    for p in [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    ] {
        if Path::new(p).exists() {
            return Some(PathBuf::from(p));
        }
    }
    None
}

impl<'tc, 'ttf> App<'tc, 'ttf> {
    fn new(
        canvas: Canvas<Window>,
        tc: &'tc TextureCreator<WindowContext>,
        font_big: Font<'ttf, 'static>,
        font_ui: Font<'ttf, 'static>,
        win_w: u32,
        win_h: u32,
    ) -> Self {
        Self {
            canvas,
            tc,
            font_big,
            font_ui,
            doc: Doc::new(),
            cam: Camera::new(),
            tiles: HashMap::new(),
            tips: TipCache::new(),
            img_tex: HashMap::new(),
            text_tex: HashMap::new(),
            label_cache: HashMap::new(),
            vec_data: HashMap::new(),
            vec_tex: HashMap::new(),
            panel_hits: Vec::new(),
            panel_speed: None,
            last_frame: Instant::now(),
            tool: UiTool::Pencil,
            color_idx: 0,
            sizes: [Tool::Pencil.default_size(), Tool::Pen.default_size(), Tool::Marker.default_size(), Tool::OilStick.default_size()],
            selection: None,
            drag: DragKind::None,
            drag_sx: 0.0,
            drag_sy: 0.0,
            pan_cx: 0.0,
            pan_cy: 0.0,
            move_kind: None,
            move_id: 0,
            move_ox: 0.0,
            move_oy: 0.0,
            move_dx: 0.0,
            move_dy: 0.0,
            active_stroke: None,
            space_down: false,
            text_edit: None,
            import_buf: None,
            show_help: false,
            toast: None,
            start: Instant::now(),
            last_click_at: None,
            last_click_item: None,
            win_w,
            win_h,
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast = Some((msg.to_string(), Instant::now()));
    }

    fn brush_size(&self) -> f32 {
        self.sizes[tool_index(self.tool)]
    }
    fn set_brush_size(&mut self, s: f32) {
        let i = tool_index(self.tool);
        self.sizes[i] = s.clamp(1.0, 96.0);
    }

    // ---- paint tiles ----
    fn rerasterize(&mut self) {
        let dirty = self.doc.take_dirty();
        if dirty.is_empty() {
            return;
        }
        let mut keys = HashSet::new();
        for r in &dirty {
            for k in tiles_in_rect(&r.expand(160.0)) {
                keys.insert(k);
            }
        }
        let tc = self.tc;
        let doc = &self.doc;
        let tips = &mut self.tips;
        let tiles = &mut self.tiles;
        let canvas = &mut self.canvas;
        for key in keys {
            let tile = tiles.entry(key).or_insert_with(|| {
                let mut t = tc
                    .create_texture_target(PixelFormatEnum::RGBA8888, TILE_PX as u32, TILE_PX as u32)
                    .expect("tile texture");
                t.set_blend_mode(BlendMode::Blend);
                t
            });
            let origin = (key.0 as f32 * TILE_PX, key.1 as f32 * TILE_PX);
            let trect = Rect::new(origin.0, origin.1, origin.0 + TILE_PX, origin.1 + TILE_PX);
            canvas
                .with_texture_canvas(tile, |c| {
                    c.set_draw_color(Color::RGBA(0, 0, 0, 0));
                    c.clear();
                    for s in &doc.data.strokes {
                        if !s.bbox().intersects(&trect) {
                            continue;
                        }
                        rasterize_stroke(c, tc, tips, s, origin);
                    }
                })
                .expect("rasterize");
        }
    }
}

fn rasterize_stroke<'a>(
    c: &mut Canvas<Window>,
    tc: &'a TextureCreator<WindowContext>,
    tips: &mut TipCache<'a>,
    s: &Stroke,
    origin: (f32, f32),
) {
    if s.points.is_empty() {
        return;
    }
    let tool = Tool::from(s.tool);
    let d = s.size.max(2.0) as u32;
    let tip = tips.get(tc, tool, d).expect("tip");
    let col = PALETTE[s.color % PALETTE_COUNT].color;
    tip.set_color_mod(col.r, col.g, col.b);
    let stamp = |c: &mut Canvas<Window>, x: f32, y: f32, dd: f32| {
        blit_stamp(c, tip, x - origin.0, y - origin.1, dd).expect("stamp");
    };
    if s.points.len() == 1 {
        let (x, y) = s.points[0];
        stamp(c, x, y, s.size);
        return;
    }
    for w in s.points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        walk_segment(x0, y0, x1, y1, s.size, tool.spacing(), |x, y, dd| {
            stamp(c, x, y, dd)
        });
    }
}

// ================= events =================
impl<'tc, 'ttf> App<'tc, 'ttf> {
    fn ui_hit(&self, sx: i32, sy: i32) -> bool {
        if sy < TOP_H || sx < TOOL_W || sy > self.win_h as i32 - BOT_H {
            return true;
        }
        // the vector panel floats above the bottom bar, in canvas territory —
        // its clicks must reach ui_click, not the canvas hit-test
        if let Some(r) = self.vector_panel_rect() {
            if r.contains_point((sx, sy)) {
                return true;
            }
        }
        false
    }

    /// Screen rect of the vector animation panel, when a vector is selected.
    fn vector_panel_rect(&self) -> Option<SdlRect> {
        match self.selection {
            Some((SelKind::Vector, _)) => {
                let (w, h) = (self.win_w as i32, self.win_h as i32);
                let pw = 700i32;
                Some(SdlRect::new((w - pw) / 2, h - BOT_H - 50, pw as u32, 42))
            }
            _ => None,
        }
    }

    fn tool_rect(&self, i: usize) -> SdlRect {
        SdlRect::new(6, TOP_H + 12 + i as i32 * 54, 48, 48)
    }
    fn swatch_rect(&self, i: usize) -> SdlRect {
        SdlRect::new(
            TOOL_W + 12 + i as i32 * 46,
            self.win_h as i32 - 62,
            40,
            40,
        )
    }
    fn slider_rect(&self) -> SdlRect {
        SdlRect::new(self.win_w as i32 - 300, self.win_h as i32 - 56, 220, 28)
    }

    /// Hit test topmost-first (text > paint > images, matching draw order top-down).
    fn hit_test(&self, wx: f32, wy: f32) -> Option<(SelKind, u64)> {
        for t in self.doc.data.texts.iter().rev() {
            let (dx, dy) = self.move_delta_for(SelKind::Text, t.id);
            if Rect::new(t.x + dx, t.y + dy, t.x + dx + t.w, t.y + dy + t.h).contains(wx, wy) {
                return Some((SelKind::Text, t.id));
            }
        }
        for v in self.doc.data.vectors.iter().rev() {
            let (dx, dy) = self.move_delta_for(SelKind::Vector, v.id);
            if !Rect::new(v.x + dx, v.y + dy, v.x + dx + v.sw, v.y + dy + v.sh).contains(wx, wy) {
                continue;
            }
            let lx = (wx - v.x - dx) / v.sw * v.w;
            let ly = (wy - v.y - dy) / v.sh * v.h;
            let hit = match self.vec_data.get(&v.id) {
                Some(polys) => polys.iter().any(|p| point_in_poly(lx, ly, &p.pts)),
                None => true,
            };
            if hit {
                return Some((SelKind::Vector, v.id));
            }
        }
        let thresh = 8.0 / self.cam.zoom;
        let mut best: Option<(f32, u64)> = None;
        for s in &self.doc.data.strokes {
            let d = s.dist_to(wx, wy);
            let tol = (s.size * 0.5 + thresh).max(thresh + 2.0);
            if d < tol && best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, s.id));
            }
        }
        if let Some((_, id)) = best {
            return Some((SelKind::Stroke, id));
        }
        for m in self.doc.data.images.iter().rev() {
            let (dx, dy) = self.move_delta_for(SelKind::Image, m.id);
            if Rect::new(m.x + dx, m.y + dy, m.x + dx + m.w, m.y + dy + m.h).contains(wx, wy) {
                return Some((SelKind::Image, m.id));
            }
        }
        None
    }

    fn move_delta_for(&self, kind: SelKind, id: u64) -> (f32, f32) {
        if self.drag == DragKind::MoveItem && self.move_kind == Some(kind) && self.move_id == id {
            (self.move_dx, self.move_dy)
        } else {
            (0.0, 0.0)
        }
    }

    fn on_down(&mut self, sx: i32, sy: i32, btn: MouseButton, video: &sdl2::VideoSubsystem) {
        // UI chrome first
        if self.ui_hit(sx, sy) {
            self.ui_click(sx, sy);
            return;
        }
        if self.text_edit.is_some() || self.import_buf.is_some() {
            return; // clicks while typing go nowhere (esc cancels)
        }
        let (wx, wy) = self.cam.screen_to_world(sx as f32, sy as f32, self.win_w as f32, self.win_h as f32);
        let panning = btn == MouseButton::Middle || self.tool == UiTool::Pan || self.space_down;
        if btn == MouseButton::Left && panning {
            self.drag = DragKind::Pan;
            self.drag_sx = sx as f32;
            self.drag_sy = sy as f32;
            self.pan_cx = self.cam.cx;
            self.pan_cy = self.cam.cy;
            return;
        }
        if btn != MouseButton::Left {
            return;
        }
        match self.tool {
            UiTool::Select => {
                if let Some((kind, id)) = self.hit_test(wx, wy) {
                    // double-click on text -> edit it
                    let now = Instant::now();
                    let dbl = self.last_click_at.map(|t| now.duration_since(t).as_millis() < 450).unwrap_or(false)
                        && self.last_click_item == Some((kind, id));
                    self.last_click_at = Some(now);
                    self.last_click_item = Some((kind, id));
                    if dbl && kind == SelKind::Text {
                        if let Some(t) = self.doc.data.texts.iter().find(|t| t.id == id) {
                            self.text_edit = Some(TextEdit {
                                x: t.x,
                                y: t.y,
                                buf: t.text.clone(),
                                existing: Some(id),
                            });
                            video.text_input().start();
                            return;
                        }
                    }
                    self.selection = Some((kind, id));
                    if kind != SelKind::Stroke {
                        self.drag = DragKind::MoveItem;
                        self.drag_sx = sx as f32;
                        self.drag_sy = sy as f32;
                        self.move_kind = Some(kind);
                        self.move_id = id;
                        let (ox, oy) = self.item_pos(kind, id);
                        self.move_ox = ox;
                        self.move_oy = oy;
                        self.move_dx = 0.0;
                        self.move_dy = 0.0;
                    }
                } else {
                    self.selection = None;
                    self.last_click_at = None;
                    self.last_click_item = None;
                }
            }
            UiTool::Pan => {}
            UiTool::Text => {
                // click empty space -> new text; double-click existing -> edit (via select path above is for V)
                self.text_edit = Some(TextEdit { x: wx, y: wy, buf: String::new(), existing: None });
                video.text_input().start();
            }
            t if t.draw_tool().is_some() => {
                let tool = t.draw_tool().unwrap();
                self.drag = DragKind::Stroke;
                self.active_stroke = Some(ActiveStroke {
                    tool,
                    color: self.color_idx,
                    size: self.brush_size(),
                    points: vec![(wx, wy)],
                    last_sx: sx as f32,
                    last_sy: sy as f32,
                });
            }
            _ => {}
        }
    }

    fn item_pos(&self, kind: SelKind, id: u64) -> (f32, f32) {
        match kind {
            SelKind::Image => self.doc.data.images.iter().find(|m| m.id == id).map(|m| (m.x, m.y)).unwrap_or((0.0, 0.0)),
            SelKind::Text => self.doc.data.texts.iter().find(|t| t.id == id).map(|t| (t.x, t.y)).unwrap_or((0.0, 0.0)),
            SelKind::Vector => self.doc.data.vectors.iter().find(|v| v.id == id).map(|v| (v.x, v.y)).unwrap_or((0.0, 0.0)),
            SelKind::Stroke => (0.0, 0.0),
        }
    }

    fn on_motion(&mut self, sx: i32, sy: i32) {
        match self.drag {
            DragKind::Pan => {
                let dx = (sx as f32 - self.drag_sx) / self.cam.zoom;
                let dy = (sy as f32 - self.drag_sy) / self.cam.zoom;
                self.cam.cx = self.pan_cx - dx;
                self.cam.cy = self.pan_cy - dy;
            }
            DragKind::MoveItem => {
                let (wx0, wy0) = self.cam.screen_to_world(self.drag_sx, self.drag_sy, self.win_w as f32, self.win_h as f32);
                let (wx1, wy1) = self.cam.screen_to_world(sx as f32, sy as f32, self.win_w as f32, self.win_h as f32);
                self.move_dx = wx1 - wx0;
                self.move_dy = wy1 - wy0;
            }
            DragKind::Stroke => {
                if let Some(st) = &mut self.active_stroke {
                    let dx = sx as f32 - st.last_sx;
                    let dy = sy as f32 - st.last_sy;
                    if dx * dx + dy * dy > 9.0 {
                        let (wx, wy) = self.cam.screen_to_world(sx as f32, sy as f32, self.win_w as f32, self.win_h as f32);
                        st.points.push((wx, wy));
                        st.last_sx = sx as f32;
                        st.last_sy = sy as f32;
                    }
                }
            }
            DragKind::Slider => {
                let r = self.slider_rect();
                let t = ((sx - r.x()) as f32 / r.width() as f32).clamp(0.0, 1.0);
                self.set_brush_size(1.0 + t * 95.0);
            }
            DragKind::Speed => {
                if let Some(r) = self.panel_speed {
                    let t = ((sx - r.x()) as f32 / r.width() as f32).clamp(0.0, 1.0);
                    if let Some((SelKind::Vector, id)) = self.selection {
                        if let Some(vm) = self.doc.data.vectors.iter_mut().find(|v| v.id == id) {
                            vm.speed = 0.25 + t * 2.75;
                        }
                    }
                }
            }
            DragKind::None => {}
        }
    }

    fn on_up(&mut self, _btn: MouseButton, video: &sdl2::VideoSubsystem) {
        let _ = video;
        match self.drag {
            DragKind::Stroke => {
                if let Some(st) = self.active_stroke.take() {
                    if !st.points.is_empty() {
                        let id = self.doc.alloc_id();
                        self.doc.add_stroke(Stroke {
                            id,
                            tool: st.tool.into(),
                            color: st.color,
                            size: st.size,
                            points: st.points,
                        });
                    }
                }
            }
            DragKind::MoveItem => {
                if let Some(kind) = self.move_kind {
                    let (dx, dy) = (self.move_dx, self.move_dy);
                    match kind {
                        SelKind::Image => self.doc.move_image(self.move_id, dx, dy),
                        SelKind::Text => self.doc.move_text(self.move_id, dx, dy),
                        SelKind::Vector => self.doc.move_vector(self.move_id, dx, dy),
                        SelKind::Stroke => {}
                    }
                }
                self.move_kind = None;
            }
            _ => {}
        }
        self.drag = DragKind::None;
    }

    fn on_wheel(&mut self, y: i32, mx: i32, my: i32) {
        if self.ui_hit(mx, my) {
            return;
        }
        let f = if y > 0 { 1.12 } else { 1.0 / 1.12 };
        self.cam.zoom_at(f, mx as f32, my as f32, self.win_w as f32, self.win_h as f32);
    }

    /// Returns false when the app should quit.
    fn on_key(&mut self, keycode: Option<Keycode>, keymod: Mod, video: &sdl2::VideoSubsystem) -> bool {
        let ctrl = keymod.contains(Mod::LCTRLMOD) || keymod.contains(Mod::RCTRLMOD);
        let shift = keymod.contains(Mod::LSHIFTMOD) || keymod.contains(Mod::RSHIFTMOD);

        // --- text entry modes take over the keyboard ---
        if self.import_buf.is_some() || self.text_edit.is_some() {
            match keycode {
                Some(Keycode::Return) | Some(Keycode::KpEnter) => self.commit_text_input(),
                Some(Keycode::Escape) => self.cancel_text_input(video),
                Some(Keycode::Backspace) => {
                    if let Some(b) = self.import_buf.as_mut() {
                        b.pop();
                    } else if let Some(e) = self.text_edit.as_mut() {
                        e.buf.pop();
                    }
                }
                _ => {}
            }
            return true;
        }

        match keycode {
            Some(Keycode::Escape) => {
                if self.show_help {
                    self.show_help = false;
                } else if self.drag != DragKind::None {
                    self.drag = DragKind::None;
                    self.active_stroke = None;
                    self.move_kind = None;
                } else {
                    self.selection = None;
                }
            }
            Some(Keycode::Space) => {}
            Some(Keycode::Delete) | Some(Keycode::Backspace) => self.delete_selection(),
            Some(Keycode::F1) => self.show_help = !self.show_help,
            Some(Keycode::V) if !ctrl => self.set_tool(UiTool::Select),
            Some(Keycode::H) if !ctrl => self.set_tool(UiTool::Pan),
            Some(Keycode::P) if !ctrl => self.set_tool(UiTool::Pencil),
            Some(Keycode::N) if !ctrl => self.set_tool(UiTool::Pen),
            Some(Keycode::M) if !ctrl => self.set_tool(UiTool::Marker),
            Some(Keycode::O) if !ctrl => self.set_tool(UiTool::OilStick),
            Some(Keycode::T) if !ctrl => self.set_tool(UiTool::Text),
            Some(Keycode::I) if !ctrl => {
                self.import_buf = Some(String::new());
                video.text_input().start();
            }
            Some(Keycode::X) if !ctrl => self.vectorize_selection(),
            Some(Keycode::LeftBracket) => self.set_brush_size(self.brush_size() / 1.25),
            Some(Keycode::RightBracket) => self.set_brush_size(self.brush_size() * 1.25),
            Some(Keycode::Z) if ctrl && !shift => self.do_undo(),
            Some(Keycode::Y) if ctrl => self.do_redo(),
            Some(Keycode::Z) if ctrl && shift => self.do_redo(),
            Some(Keycode::S) if ctrl => self.save_doc(),
            Some(Keycode::E) if ctrl => self.export_png(),
            Some(Keycode::O) if ctrl => self.load_doc(),
            Some(Keycode::Q) if ctrl => return false,
            Some(Keycode::Equals) if ctrl => self.zoom_step(1.25),
            Some(Keycode::Minus) if ctrl => self.zoom_step(0.8),
            Some(Keycode::Num0) if ctrl => {
                self.cam.cx = 0.0;
                self.cam.cy = 0.0;
                self.cam.zoom = 1.0;
            }
            _ => {}
        }
        true
    }

    fn on_text(&mut self, text: &str) {
        if let Some(b) = self.import_buf.as_mut() {
            b.push_str(text);
        } else if let Some(e) = self.text_edit.as_mut() {
            e.buf.push_str(text);
        }
    }

    fn commit_text_input(&mut self) {
        if let Some(buf) = self.import_buf.take() {
            let path = buf.trim().to_string();
            if !path.is_empty() {
                self.import_folder(&path);
            }
        } else if let Some(ed) = self.text_edit.take() {
            let text = ed.buf.trim().to_string();
            if !text.is_empty() {
                if let Some(id) = ed.existing {
                    let color = self.color_idx_of_text(id);
                    let size = self.doc.data.texts.iter().find(|t| t.id == id).map(|t| t.size).unwrap_or(32.0);
                    if let Ok((tex, w, h)) = self.make_text_texture(&text, color, size) {
                        self.doc.set_text(id, text, w, h);
                        self.text_tex.insert(id, tex);
                    }
                } else {
                    let color = self.color_idx;
                    if let Ok((tex, w, h)) = self.make_text_texture(&text, color, 32.0) {
                        let id = self.doc.alloc_id();
                        self.doc.add_text(TextMeta {
                            id,
                            text,
                            x: ed.x,
                            y: ed.y,
                            size: 32.0,
                            color,
                            w,
                            h,
                        });
                        self.text_tex.insert(id, tex);
                        self.selection = Some((SelKind::Text, id));
                    }
                }
            }
        }
    }

    fn cancel_text_input(&mut self, video: &sdl2::VideoSubsystem) {
        self.import_buf = None;
        self.text_edit = None;
        video.text_input().stop();
    }

    fn color_idx_of_text(&self, id: u64) -> usize {
        self.doc.data.texts.iter().find(|t| t.id == id).map(|t| t.color).unwrap_or(0)
    }

    fn set_tool(&mut self, t: UiTool) {
        self.tool = t;
        self.selection = None;
    }

    fn zoom_step(&mut self, f: f32) {
        let (cx, cy) = (self.win_w as f32 / 2.0, self.win_h as f32 / 2.0);
        self.cam.zoom_at(f, cx, cy, self.win_w as f32, self.win_h as f32);
    }

    fn delete_selection(&mut self) {
        if let Some((kind, id)) = self.selection.take() {
            match kind {
                SelKind::Stroke => {
                    self.doc.delete_stroke(id);
                }
                SelKind::Image => {
                    self.doc.delete_image(id);
                    self.img_tex.remove(&id);
                }
                SelKind::Text => {
                    self.doc.delete_text(id);
                    self.text_tex.remove(&id);
                }
                SelKind::Vector => {
                    self.doc.delete_vector(id);
                    self.vec_data.remove(&id);
                    self.vec_tex.remove(&id);
                }
            }
        }
    }

    fn do_undo(&mut self) {
        if self.doc.undo().is_some() {
            self.sync_media();
            self.selection = None;
        }
    }
    fn do_redo(&mut self) {
        if self.doc.redo().is_some() {
            self.sync_media();
            self.selection = None;
        }
    }

    /// Reconcile GPU textures with doc data (after undo/redo/load).
    fn sync_media(&mut self) {
        let live_img: std::collections::HashSet<u64> =
            self.doc.data.images.iter().map(|m| m.id).collect();
        self.img_tex.retain(|id, _| live_img.contains(id));
        for m in &self.doc.data.images.clone() {
            if !self.img_tex.contains_key(&m.id) {
                if let Ok(tex) = self.load_image_texture(&m.path) {
                    self.img_tex.insert(m.id, tex);
                }
            }
        }
        let live_tx: std::collections::HashSet<u64> =
            self.doc.data.texts.iter().map(|t| t.id).collect();
        self.text_tex.retain(|id, _| live_tx.contains(id));
        let live_v: std::collections::HashSet<u64> =
            self.doc.data.vectors.iter().map(|v| v.id).collect();
        self.vec_tex.retain(|id, _| live_v.contains(id));
        self.vec_data.retain(|id, _| live_v.contains(id));
        for v in &self.doc.data.vectors.clone() {
            if !self.vec_data.contains_key(&v.id) {
                self.vec_data.insert(v.id, rebuild_polys(v));
            }
        }
        for t in &self.doc.data.texts.clone() {
            if !self.text_tex.contains_key(&t.id) {
                if let Ok((tex, _, _)) = self.make_text_texture(&t.text, t.color, t.size) {
                    self.text_tex.insert(t.id, tex);
                }
            }
        }
    }

    fn on_drop(&mut self, filename: &str) {
        let p = Path::new(filename);
        if p.is_dir() {
            self.import_folder(filename);
        } else if is_image_path(p) {
            let (wx, wy) = (self.cam.cx, self.cam.cy);
            self.import_image_file(p, wx, wy);
        }
    }
}

fn is_image_path(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("bmp") | Some("gif") | Some("webp") | Some("tif") | Some("tiff")
    )
}

// ================= rendering =================
impl<'tc, 'ttf> App<'tc, 'ttf> {
    fn frame(&mut self) -> Result<(), String> {
        self.rerasterize();
        // advance vector animations
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.1);
        self.last_frame = Instant::now();
        for v in self.doc.data.vectors.iter_mut() {
            if v.playing && v.anim.animated() {
                v.t += dt;
            }
        }
        self.canvas.set_draw_color(PAPER);
        self.canvas.clear();
        self.draw_grid();
        self.draw_tiles()?;
        self.draw_images()?;
        self.draw_vectors()?;
        self.draw_texts()?;
        self.draw_active_stroke()?;
        self.draw_selection();
        self.draw_text_edit()?;
        self.draw_chrome()?;
        self.draw_vector_panel()?;
        if self.show_help {
            self.draw_help()?;
        }
        self.draw_import_bar()?;
        self.draw_toast()?;
        self.canvas.present();
        Ok(())
    }

    // ---------- vectors ----------
    fn vectorize_selection(&mut self) {
        let img_id = match self.selection {
            Some((SelKind::Image, id)) => id,
            _ => {
                self.toast("select an image first (V), then X");
                return;
            }
        };
        let meta = match self.doc.data.images.iter().find(|m| m.id == img_id) {
            Some(m) => m.clone(),
            None => return,
        };
        let rgba = match image::open(&meta.path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.toast(&format!("cannot open image: {}", e));
                return;
            }
        };
        let t0 = Instant::now();
        match vectorize_rgba(rgba.as_raw(), rgba.width(), rgba.height()) {
            Ok((polys, tw, th)) => {
                let n = polys.len();
                let id = self.doc.alloc_id();
                let vm = VectorMeta {
                    id,
                    polys: polys.iter().map(|p| p.pts.clone()).collect(),
                    w: tw as f32,
                    h: th as f32,
                    x: meta.x,
                    y: meta.y,
                    sw: meta.w,
                    sh: meta.h,
                    color: self.color_idx,
                    filled: true,
                    anim: AnimKind::Draw,
                    speed: 1.0,
                    playing: true,
                    t: 0.0,
                };
                self.vec_data.insert(id, polys);
                self.doc.add_vector(vm);
                self.vec_tex.remove(&id);
                self.selection = Some((SelKind::Vector, id));
                self.toast(&format!("vectorized: {} shapes in {:.1}s", n, t0.elapsed().as_secs_f32()));
            }
            Err(e) => self.toast(&format!("vectorize: {}", e)),
        }
    }

    fn vector_frame_idx(&self, id: u64) -> u64 {
        match self.doc.data.vectors.iter().find(|v| v.id == id) {
            Some(m) if m.anim.animated() && m.playing => (m.t * 24.0) as u64,
            _ => u64::MAX, // static: render once
        }
    }

    fn ensure_vector_texture(&mut self, id: u64) -> bool {
        let idx = self.vector_frame_idx(id);
        if let Some((_, last)) = self.vec_tex.get(&id) {
            if *last == idx {
                return true;
            }
        }
        let (meta, polys) = match (
            self.doc.data.vectors.iter().find(|v| v.id == id),
            self.vec_data.get(&id),
        ) {
            (Some(m), Some(p)) => (m.clone(), p.clone()),
            _ => return false,
        };
        let col = PALETTE[meta.color % PALETTE_COUNT].color;
        let (buf, bw, bh) = render_frame(&meta, &polys, (col.r, col.g, col.b));
        let mut owned = buf;
        let surf = match sdl2::surface::Surface::from_data(&mut owned, bw, bh, bw * 4, PixelFormatEnum::RGBA32) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let tex = match self.tc.create_texture_from_surface(&surf) {
            Ok(mut t) => {
                t.set_blend_mode(BlendMode::Blend);
                t
            }
            Err(_) => return false,
        };
        self.vec_tex.insert(id, (tex, idx));
        true
    }

    fn draw_vectors(&mut self) -> Result<(), String> {
        let ids: Vec<u64> = self.doc.data.vectors.iter().map(|v| v.id).collect();
        for id in &ids {
            self.ensure_vector_texture(*id);
        }
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let items: Vec<(u64, f32, f32, f32, f32)> = self
            .doc
            .data
            .vectors
            .iter()
            .map(|v| {
                let (dx, dy) = self.move_delta_for(SelKind::Vector, v.id);
                (v.id, v.x + dx, v.y + dy, v.sw, v.sh)
            })
            .collect();
        for (id, x, y, sw, sh) in items {
            if let Some((tex, _)) = self.vec_tex.get(&id) {
                let (sx, sy) = cam.world_to_screen(x, y, w, h);
                let dw = (sw * cam.zoom).max(1.0) as u32;
                let dh = (sh * cam.zoom).max(1.0) as u32;
                self.canvas.copy(tex, None, SdlRect::new(sx as i32, sy as i32, dw, dh))?;
            }
        }
        Ok(())
    }

    fn panel_action(&mut self, act: PanelAction) {
        let id = match self.selection {
            Some((SelKind::Vector, id)) => id,
            _ => return,
        };
        let vm = match self.doc.data.vectors.iter_mut().find(|v| v.id == id) {
            Some(v) => v,
            None => return,
        };
        match act {
            PanelAction::Anim(k) => {
                vm.anim = k;
                if k.animated() {
                    vm.playing = true;
                }
                self.vec_tex.remove(&id);
            }
            PanelAction::PlayPause => {
                vm.playing = !vm.playing;
                self.vec_tex.remove(&id);
            }
            PanelAction::Fill => {
                vm.filled = !vm.filled;
                self.vec_tex.remove(&id);
            }
        }
    }

    fn draw_vector_panel(&mut self) -> Result<(), String> {
        self.panel_hits.clear();
        self.panel_speed = None;
        let (anim, playing, filled, speed) = match self.selection {
            Some((SelKind::Vector, id)) => match self.doc.data.vectors.iter().find(|v| v.id == id) {
                Some(v) => (v.anim, v.playing, v.filled, v.speed),
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let pr = match self.vector_panel_rect() {
            Some(r) => r,
            None => return Ok(()),
        };
        let (px, py) = (pr.x(), pr.y());
        let pw = pr.width() as i32;
        self.canvas.set_draw_color(CHROME2);
        self.canvas.fill_rect(SdlRect::new(px, py, pw as u32, 42))?;
        self.canvas.set_draw_color(ACCENT);
        self.canvas.draw_rect(SdlRect::new(px, py, pw as u32, 42))?;
        let mut x = px + 8;
        for k in AnimKind::all() {
            let r = SdlRect::new(x, py + 7, 64, 28);
            self.button(r, k.label(), anim == k)?;
            self.panel_hits.push((r, PanelAction::Anim(k)));
            x += 70;
        }
        let r = SdlRect::new(x, py + 7, 44, 28);
        self.button(r, if playing { "II" } else { ">" }, false)?;
        self.panel_hits.push((r, PanelAction::PlayPause));
        x += 52;
        let r = SdlRect::new(x, py + 7, 52, 28);
        self.button(r, "Fill", filled)?;
        self.panel_hits.push((r, PanelAction::Fill));
        x += 60;
        self.draw_label("speed", DIM, x, py + 12)?;
        x += 52;
        let sr = SdlRect::new(x, py + 7, 110, 28);
        self.canvas.set_draw_color(CHROME);
        self.canvas.fill_rect(sr)?;
        let t = ((speed - 0.25) / 2.75).clamp(0.0, 1.0);
        let fx = sr.x() + (t * sr.width() as f32) as i32;
        self.canvas.set_draw_color(ACCENT);
        self.canvas.fill_rect(SdlRect::new(fx - 3, sr.y(), 6, sr.height()))?;
        self.panel_speed = Some(sr);
        Ok(())
    }

    fn draw_grid(&mut self) {
        let zoom = self.cam.zoom;
        if zoom < 0.18 {
            return;
        }
        let mut spacing = 64.0;
        while spacing * zoom < 26.0 {
            spacing *= 2.0;
        }
        let view = self.cam.view_rect(self.win_w as f32, self.win_h as f32);
        let x0 = (view.x0 / spacing).floor() * spacing;
        let y0 = (view.y0 / spacing).floor() * spacing;
        let mut pts = vec![];
        let mut x = x0;
        while x <= view.x1 {
            let mut y = y0;
            while y <= view.y1 {
                let (sx, sy) = self.cam.world_to_screen(x, y, self.win_w as f32, self.win_h as f32);
                pts.push(sdl2::rect::Point::new(sx as i32, sy as i32));
                y += spacing;
            }
            x += spacing;
        }
        self.canvas.set_draw_color(PAPER_DOT);
        let _ = self.canvas.draw_points(pts.as_slice());
    }

    fn draw_tiles(&mut self) -> Result<(), String> {
        let view = self.cam.view_rect(self.win_w as f32, self.win_h as f32);
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let canvas = &mut self.canvas;
        for ((tx, ty), tile) in &self.tiles {
            let wx = *tx as f32 * TILE_PX;
            let wy = *ty as f32 * TILE_PX;
            if !view.intersects(&Rect::new(wx, wy, wx + TILE_PX, wy + TILE_PX)) {
                continue;
            }
            let (sx, sy) = cam.world_to_screen(wx, wy, w, h);
            let d = TILE_PX * cam.zoom;
            canvas.copy(tile, None, SdlRect::new(sx as i32, sy as i32, d.ceil() as u32, d.ceil() as u32))?;
        }
        Ok(())
    }

    fn draw_images(&mut self) -> Result<(), String> {
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let view = self.cam.view_rect(w, h);
        // collect to end borrows before &mut canvas
        let items: Vec<(u64, f32, f32, f32, f32)> = self
            .doc
            .data
            .images
            .iter()
            .map(|m| {
                let (dx, dy) = self.move_delta_for(SelKind::Image, m.id);
                (m.id, m.x + dx, m.y + dy, m.w, m.h)
            })
            .collect();
        for (id, x, y, iw, ih) in items {
            if !view.intersects(&Rect::new(x, y, x + iw, y + ih)) {
                continue;
            }
            let (sx, sy) = cam.world_to_screen(x, y, w, h);
            let dw = (iw * cam.zoom).max(1.0) as u32;
            let dh = (ih * cam.zoom).max(1.0) as u32;
            let dst = SdlRect::new(sx as i32, sy as i32, dw, dh);
            if let Some(tex) = self.img_tex.get(&id) {
                self.canvas.copy(tex, None, dst)?;
            } else {
                // missing file placeholder
                self.canvas.set_draw_color(Color::RGB(0xaa, 0x55, 0x55));
                self.canvas.draw_rect(dst)?;
                self.draw_label("?", INK, dst.x() + 4, dst.y() + 4)?;
            }
        }
        Ok(())
    }

    fn draw_texts(&mut self) -> Result<(), String> {
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let items: Vec<(u64, f32, f32, f32, f32)> = self
            .doc
            .data
            .texts
            .iter()
            .map(|t| {
                let (dx, dy) = self.move_delta_for(SelKind::Text, t.id);
                (t.id, t.x + dx, t.y + dy, t.w, t.h)
            })
            .collect();
        for (id, x, y, iw, ih) in items {
            let (sx, sy) = cam.world_to_screen(x, y, w, h);
            let dw = (iw * cam.zoom).max(1.0) as u32;
            let dh = (ih * cam.zoom).max(1.0) as u32;
            if let Some(tex) = self.text_tex.get(&id) {
                self.canvas.copy(tex, None, SdlRect::new(sx as i32, sy as i32, dw, dh))?;
            }
        }
        Ok(())
    }

    fn draw_active_stroke(&mut self) -> Result<(), String> {
        let st = match &self.active_stroke {
            Some(s) => s,
            None => return Ok(()),
        };
        if st.points.len() < 2 {
            return Ok(());
        }
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let dpx = (st.size * cam.zoom).max(2.0) as u32;
        // borrow juggling: get tip, then stamp
        let tc = self.tc;
        let tips = &mut self.tips;
        let canvas = &mut self.canvas;
        let tip = tips.get(tc, st.tool, dpx).map_err(|e| e)?;
        let col = PALETTE[st.color % PALETTE_COUNT].color;
        tip.set_color_mod(col.r, col.g, col.b);
        let scr: Vec<(f32, f32)> = st.points.iter().map(|&(x, y)| cam.world_to_screen(x, y, w, h)).collect();
        for pair in scr.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            walk_segment(x0, y0, x1, y1, st.size * cam.zoom, st.tool.spacing(), |x, y, dd| {
                let _ = blit_stamp(canvas, tip, x, y, dd);
            });
        }
        Ok(())
    }

    fn draw_selection(&mut self) {
        let (kind, id) = match self.selection {
            Some(s) => s,
            None => return,
        };
        let r = match kind {
            SelKind::Stroke => self.doc.data.strokes.iter().find(|s| s.id == id).map(|s| s.bbox()),
            SelKind::Image => self.doc.data.images.iter().find(|m| m.id == id).map(|m| {
                let (dx, dy) = self.move_delta_for(kind, id);
                Rect::new(m.x + dx, m.y + dy, m.x + dx + m.w, m.y + dy + m.h)
            }),
            SelKind::Text => self.doc.data.texts.iter().find(|t| t.id == id).map(|t| {
                let (dx, dy) = self.move_delta_for(kind, id);
                Rect::new(t.x + dx, t.y + dy, t.x + dx + t.w, t.y + dy + t.h)
            }),
            SelKind::Vector => self.doc.data.vectors.iter().find(|v| v.id == id).map(|v| {
                let (dx, dy) = self.move_delta_for(kind, id);
                Rect::new(v.x + dx, v.y + dy, v.x + dx + v.sw, v.y + dy + v.sh)
            }),
        };
        if let Some(r) = r {
            let (w, h) = (self.win_w as f32, self.win_h as f32);
            let (sx0, sy0) = self.cam.world_to_screen(r.x0, r.y0, w, h);
            let (sx1, sy1) = self.cam.world_to_screen(r.x1, r.y1, w, h);
            self.canvas.set_draw_color(SEL);
            let _ = self.canvas.draw_rect(SdlRect::new(
                sx0 as i32 - 4,
                sy0 as i32 - 4,
                (sx1 - sx0 + 8.0).max(4.0) as u32,
                (sy1 - sy0 + 8.0).max(4.0) as u32,
            ));
        }
    }

    fn draw_text_edit(&mut self) -> Result<(), String> {
        let (ex, ey, buf) = match &self.text_edit {
            Some(e) => (e.x, e.y, e.buf.clone()),
            None => return Ok(()),
        };
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let (sx, sy) = self.cam.world_to_screen(ex, ey, w, h);
        let label = if buf.is_empty() { "type here" } else { &buf };
        let dim = buf.is_empty();
        self.draw_label(label, if dim { DIM } else { PALETTE[self.color_idx].color }, sx as i32, sy as i32)?;
        // caret
        if self.start.elapsed().as_millis() % 1000 < 500 {
            let adv = self.font_ui.size_of(label).map(|(tw, _)| tw).unwrap_or(0);
            let cx = sx as i32 + adv as i32 + 3;
            self.canvas.set_draw_color(INK);
            let _ = self.canvas.fill_rect(SdlRect::new(cx, sy as i32, 2, 22));
        }
        Ok(())
    }

    // ---- chrome ----
    fn draw_label(&mut self, text: &str, color: Color, x: i32, y: i32) -> Result<(u32, u32), String> {
        if text.is_empty() {
            return Ok((0, 0));
        }
        let key = format!("{}:{}:{}:{}", text, color.r, color.g, color.b);
        if !self.label_cache.contains_key(&key) {
            let surf = self.font_ui.render(text).blended(color).map_err(|e| e.to_string())?;
            let tex = self.tc.create_texture_from_surface(&surf).map_err(|e| e.to_string())?;
            self.label_cache.insert(key.clone(), tex);
        }
        let tex = &self.label_cache[&key];
        let q = tex.query();
        let dst = SdlRect::new(x, y, q.width, q.height);
        self.canvas.copy(tex, None, dst).map_err(|e| e.to_string())?;
        Ok((q.width, q.height))
    }

    fn button(&mut self, r: SdlRect, label: &str, active: bool) -> Result<(), String> {
        let bg = if active { ACCENT } else { CHROME2 };
        let fg = if active { Color::RGB(0x20, 0x18, 0x08) } else { INK };
        self.canvas.set_draw_color(bg);
        self.canvas.fill_rect(r)?;
        let (tw, th) = self.font_ui.size_of(label).unwrap_or((0, 0));
        self.draw_label(
            label,
            fg,
            r.x() + (r.width() as i32 - tw as i32) / 2,
            r.y() + (r.height() as i32 - th as i32) / 2,
        )?;
        Ok(())
    }

    fn draw_chrome(&mut self) -> Result<(), String> {
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        // top bar
        self.canvas.set_draw_color(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, 0, w as u32, TOP_H as u32))?;
        self.draw_label("MURAL", ACCENT, 12, 12)?;
        let zoom_txt = format!("{}%", (self.cam.zoom * 100.0).round() as i32);
        self.draw_label(&zoom_txt, DIM, w - 320, 12)?;
        // top-right buttons
        self.button(SdlRect::new(w - 292, 8, 64, 28), "Save", false)?;
        self.button(SdlRect::new(w - 220, 8, 64, 28), "Vec", false)?;
        self.button(SdlRect::new(w - 148, 8, 64, 28), "PNG", false)?;
        self.button(SdlRect::new(w - 76, 8, 64, 28), "?", self.show_help)?;
        // left toolbar
        self.canvas.set_draw_color(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, TOP_H, TOOL_W as u32, (h - TOP_H - BOT_H) as u32))?;
        for (i, t) in TOOLS.iter().enumerate() {
            let r = self.tool_rect(i);
            let active = *t == self.tool;
            self.button(r, t.shortcut(), active)?;
            self.draw_label(t.label(), DIM, r.x() - 2, r.y() + r.height() as i32 + 1)?;
        }
        // bottom bar
        self.canvas.set_draw_color(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, h - BOT_H, w as u32, BOT_H as u32))?;
        for i in 0..PALETTE_COUNT {
            let r = self.swatch_rect(i);
            let c = PALETTE[i].color;
            self.canvas.set_draw_color(c);
            self.canvas.fill_rect(r)?;
            if i == self.color_idx {
                self.canvas.set_draw_color(INK);
                self.canvas.draw_rect(r)?;
                let r2 = SdlRect::new(r.x() - 2, r.y() - 2, r.width() + 4, r.height() + 4);
                self.canvas.draw_rect(r2)?;
            }
        }
        let cname = PALETTE[self.color_idx].name;
        self.draw_label(cname, DIM, TOOL_W + 12, h - 20)?;
        // brush slider
        let sr = self.slider_rect();
        self.canvas.set_draw_color(CHROME2);
        self.canvas.fill_rect(sr)?;
        let t = (self.brush_size() - 1.0) / 95.0;
        let fx = sr.x() + (t * sr.width() as f32) as i32;
        self.canvas.set_draw_color(ACCENT);
        self.canvas.fill_rect(SdlRect::new(fx - 3, sr.y(), 6, sr.height()))?;
        self.draw_label(&format!("size {:.0}", self.brush_size()), DIM, sr.x(), sr.y() - 20)?;
        // undo/redo state
        let ur = format!("{}/{}", if self.doc.can_undo() { "undo" } else { "-" }, if self.doc.can_redo() { "redo" } else { "-" });
        self.draw_label(&ur, DIM, w - 300, h - 20)?;
        Ok(())
    }

    fn ui_click(&mut self, sx: i32, sy: i32) {
        // toolbar
        for (i, t) in TOOLS.iter().enumerate() {
            if self.tool_rect(i).contains_point((sx, sy)) {
                self.set_tool(*t);
                return;
            }
        }
        // palette
        for i in 0..PALETTE_COUNT {
            if self.swatch_rect(i).contains_point((sx, sy)) {
                self.color_idx = i;
                if let Some((SelKind::Vector, id)) = self.selection {
                    if let Some(vm) = self.doc.data.vectors.iter_mut().find(|v| v.id == id) {
                        vm.color = i;
                        self.vec_tex.remove(&id);
                    }
                }
                return;
            }
        }
        // vector panel hits
        for (r, act) in self.panel_hits.clone() {
            if r.contains_point((sx, sy)) {
                self.panel_action(act);
                return;
            }
        }
        if let Some(r) = self.panel_speed {
            if r.contains_point((sx, sy)) {
                self.drag = DragKind::Speed;
                return;
            }
        }
        // slider
        let sr = self.slider_rect();
        if sr.contains_point((sx, sy)) {
            self.drag = DragKind::Slider;
            let t = ((sx - sr.x()) as f32 / sr.width() as f32).clamp(0.0, 1.0);
            self.set_brush_size(1.0 + t * 95.0);
            return;
        }
        // top-right buttons
        let w = self.win_w as i32;
        if SdlRect::new(w - 292, 8, 64, 28).contains_point((sx, sy)) {
            self.save_doc();
            return;
        }
        if SdlRect::new(w - 220, 8, 64, 28).contains_point((sx, sy)) {
            self.vectorize_selection();
            return;
        }
        if SdlRect::new(w - 148, 8, 64, 28).contains_point((sx, sy)) {
            self.export_png();
            return;
        }
        if SdlRect::new(w - 76, 8, 64, 28).contains_point((sx, sy)) {
            self.show_help = !self.show_help;
        }
    }

    fn draw_help(&mut self) -> Result<(), String> {
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        let bw = 480;
        let bh = 470;
        let bx = (w - bw) / 2;
        let by = (h - bh) / 2;
        self.canvas.set_draw_color(Color::RGBA(10, 10, 14, 230));
        self.canvas.fill_rect(SdlRect::new(bx, by, bw as u32, bh as u32))?;
        self.canvas.set_draw_color(ACCENT);
        self.canvas.draw_rect(SdlRect::new(bx, by, bw as u32, bh as u32))?;
        let lines = [
            "MURAL — shortcuts",
            "",
            "V move/select   H pan (or space / mid-drag)",
            "P pencil  N pen  M marker  O oil stick  T text",
            "[ ] brush size   wheel zoom",
            "I import folder   drag & drop works too",
            "del remove selected   esc deselect",
            "ctrl+Z / ctrl+Y undo / redo",
            "ctrl+S save mural.json   ctrl+O open",
            "ctrl+E export PNG   ctrl+0 reset view",
            "double-click text to re-edit",
            "",
            "paint sits above images, text above paint",
            "",
            "F1 closes this",
        ];
        let mut y = by + 18;
        for (i, ln) in lines.iter().enumerate() {
            if !ln.is_empty() {
                self.draw_label(ln, if i == 0 { ACCENT } else { INK }, bx + 20, y)?;
            }
            y += 28;
        }
        Ok(())
    }

    fn draw_import_bar(&mut self) -> Result<(), String> {
        let buf = match &self.import_buf {
            Some(b) => b.clone(),
            None => return Ok(()),
        };
        let (w, _) = (self.win_w as i32, self.win_h as i32);
        let bw = 560;
        let bx = (w - bw) / 2;
        self.canvas.set_draw_color(CHROME2);
        self.canvas.fill_rect(SdlRect::new(bx, TOP_H + 8, bw as u32, 40))?;
        self.canvas.set_draw_color(ACCENT);
        self.canvas.draw_rect(SdlRect::new(bx, TOP_H + 8, bw as u32, 40))?;
        let prompt = format!("import folder: {}_", buf);
        self.draw_label(&prompt, INK, bx + 12, TOP_H + 18)?;
        Ok(())
    }

    fn draw_toast(&mut self) -> Result<(), String> {
        let msg = match &self.toast {
            Some((m, _)) => m.clone(),
            None => return Ok(()),
        };
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        let (tw, _) = self.font_ui.size_of(&msg).unwrap_or((200, 20));
        let bx = (w - tw as i32 - 24) / 2;
        self.canvas.set_draw_color(Color::RGBA(10, 10, 14, 220));
        self.canvas.fill_rect(SdlRect::new(bx, h - BOT_H - 44, (tw + 24) as u32, 32))?;
        self.draw_label(&msg, ACCENT, bx + 12, h - BOT_H - 36)?;
        Ok(())
    }

    // ---- text helpers ----
    fn make_text_texture(
        &mut self,
        text: &str,
        color_idx: usize,
        world_size: f32,
    ) -> Result<(Texture<'tc>, f32, f32), String> {
        let col = PALETTE[color_idx % PALETTE_COUNT].color;
        let surf = self.font_big.render(text).blended(col).map_err(|e| e.to_string())?;
        let (pw, ph) = surf.size();
        let tex = self.tc.create_texture_from_surface(&surf).map_err(|e| e.to_string())?;
        let k = world_size / 64.0;
        Ok((tex, pw as f32 * k, ph as f32 * k))
    }

    fn load_image_texture(&mut self, path: &str) -> Result<Texture<'tc>, String> {
        let img = image::open(path).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        if w == 0 || h == 0 {
            return Err("empty image".into());
        }
        let mut owned = rgba.into_raw();
        let surf = sdl2::surface::Surface::from_data(
            &mut owned,
            w,
            h,
            w * 4,
            PixelFormatEnum::RGBA32,
        )
        .map_err(|e| e.to_string())?;
        let mut tex = self.tc.create_texture_from_surface(&surf).map_err(|e| e.to_string())?;
        tex.set_blend_mode(BlendMode::Blend);
        Ok(tex)
    }
}

// ================= import / export / save =================
impl<'tc, 'ttf> App<'tc, 'ttf> {
    fn import_image_file(&mut self, p: &Path, wx: f32, wy: f32) -> bool {
        let path = p.to_string_lossy().to_string();
        let tex = match self.load_image_texture(&path) {
            Ok(t) => t,
            Err(e) => {
                self.toast(&format!("cannot load {}: {}", p.display(), e));
                return false;
            }
        };
        let q = tex.query();
        let scale = (512.0 / q.width.max(q.height) as f32).min(1.0);
        let (w, h) = (q.width as f32 * scale, q.height as f32 * scale);
        let id = self.doc.alloc_id();
        self.doc.add_image(ImageMeta { id, path, x: wx, y: wy, w, h });
        self.img_tex.insert(id, tex);
        true
    }

    fn import_folder(&mut self, path: &str) {
        let dir = match std::fs::read_dir(path) {
            Ok(d) => d,
            Err(e) => {
                self.toast(&format!("cannot open folder: {}", e));
                return;
            }
        };
        let mut files: Vec<PathBuf> = vec![];
        for e in dir.flatten() {
            let p = e.path();
            if p.is_file() && is_image_path(&p) {
                files.push(p);
            }
        }
        files.sort();
        if files.is_empty() {
            self.toast("no images in folder");
            return;
        }
        // lay out in a row centered on the current view
        let gap = 32.0;
        let total: f32 = files
            .iter()
            .filter_map(|f| image::image_dimensions(f).ok())
            .map(|(w, h)| {
                let s = (512.0 / w.max(h) as f32).min(1.0);
                w as f32 * s + gap
            })
            .sum();
        let mut x = self.cam.cx - total / 2.0;
        let mut n = 0;
        for f in &files {
            let (iw, ih) = image::image_dimensions(f).unwrap_or((256, 256));
            let s = (512.0 / iw.max(ih) as f32).min(1.0);
            let (w, h) = (iw as f32 * s, ih as f32 * s);
            if self.import_image_file(f, x, self.cam.cy - h / 2.0) {
                n += 1;
                x += w + gap;
            }
        }
        self.toast(&format!("imported {} image{}", n, if n == 1 { "" } else { "s" }));
    }

    fn save_doc(&mut self) {
        match self.doc.to_json() {
            Ok(j) => match std::fs::write("mural.json", j) {
                Ok(_) => self.toast("saved mural.json"),
                Err(e) => self.toast(&format!("save failed: {}", e)),
            },
            Err(e) => self.toast(&format!("save failed: {}", e)),
        }
    }

    fn load_doc(&mut self) {
        let s = match std::fs::read_to_string("mural.json") {
            Ok(s) => s,
            Err(e) => {
                self.toast(&format!("open failed: {}", e));
                return;
            }
        };
        match Doc::from_json(&s) {
            Ok(data) => {
                let n_img = data.images.len();
                self.doc.load_data(data);
                self.tiles.clear();
                self.img_tex.clear();
                self.text_tex.clear();
                self.sync_media();
                let missing = n_img - self.img_tex.len();
                self.selection = None;
                self.toast(&format!(
                    "loaded mural.json{}{}",
                    "",
                    if missing > 0 { format!(" ({} images missing)", missing) } else { String::new() }
                ));
            }
            Err(e) => self.toast(&format!("open failed: {}", e)),
        }
    }

    fn export_png(&mut self) {
        let ids: Vec<u64> = self.doc.data.vectors.iter().map(|v| v.id).collect();
        for id in ids {
            self.ensure_vector_texture(id);
        }
        match self.render_export("mural.png") {
            Ok(_) => self.toast("exported mural.png"),
            Err(e) => self.toast(&format!("export failed: {}", e)),
        }
    }

    fn render_export(&mut self, path: &str) -> Result<(), String> {
        let bbox = self.doc.content_bbox().ok_or("nothing to export")?;
        let m = 64.0;
        let w = ((bbox.w() + 2.0 * m).ceil() as u32).clamp(1, 8192);
        let h = ((bbox.h() + 2.0 * m).ceil() as u32).clamp(1, 8192);
        let (ox, oy) = (bbox.x0 - m, bbox.y0 - m);
        let mut target = self
            .tc
            .create_texture_target(PixelFormatEnum::RGBA8888, w, h)
            .map_err(|e| e.to_string())?;
        let mut pixels: Vec<u8> = vec![];
        {
            let tiles = &self.tiles;
            let doc = &self.doc;
            let img_tex = &self.img_tex;
            let text_tex = &self.text_tex;
            let vec_tex = &self.vec_tex;
            let canvas = &mut self.canvas;
            canvas
                .with_texture_canvas(&mut target, |c| {
                    c.set_draw_color(PAPER);
                    c.clear();
                    for ((tx, ty), tile) in tiles {
                        let dx = *tx as f32 * TILE_PX - ox;
                        let dy = *ty as f32 * TILE_PX - oy;
                        if dx + TILE_PX < 0.0 || dy + TILE_PX < 0.0 || dx > w as f32 || dy > h as f32 {
                            continue;
                        }
                        let _ = c.copy(tile, None, SdlRect::new(dx as i32, dy as i32, TILE_PX as u32, TILE_PX as u32));
                    }
                    for m in &doc.data.images {
                        if let Some(tex) = img_tex.get(&m.id) {
                            let _ = c.copy(
                                tex,
                                None,
                                SdlRect::new((m.x - ox) as i32, (m.y - oy) as i32, m.w as u32, m.h as u32),
                            );
                        }
                    }
                    for v in &doc.data.vectors {
                        if let Some((tex, _)) = vec_tex.get(&v.id) {
                            let _ = c.copy(
                                tex,
                                None,
                                SdlRect::new((v.x - ox) as i32, (v.y - oy) as i32, v.sw as u32, v.sh as u32),
                            );
                        }
                    }
                    for t in &doc.data.texts {
                        if let Some(tex) = text_tex.get(&t.id) {
                            let _ = c.copy(
                                tex,
                                None,
                                SdlRect::new((t.x - ox) as i32, (t.y - oy) as i32, t.w as u32, t.h as u32),
                            );
                        }
                    }
                    pixels = c
                        .read_pixels(SdlRect::new(0, 0, w, h), PixelFormatEnum::RGB24)
                        .unwrap_or_default();
                })
                .map_err(|e| e.to_string())?;
        }
        if pixels.len() != (w * h * 3) as usize {
            return Err("pixel read failed".into());
        }
        image::RgbImage::from_raw(w, h, pixels)
            .ok_or("pixel buffer mismatch")?
            .save(path)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ================= run =================
    fn run(&mut self, sdl: &Sdl, video: &sdl2::VideoSubsystem) -> Result<(), String> {
        let mut pump = sdl.event_pump().map_err(|e| e.to_string())?;
        'main: loop {
            for ev in pump.poll_iter() {
                match ev {
                    Event::Quit { .. } => break 'main,
                    Event::Window { win_event: sdl2::event::WindowEvent::Resized(ww, wh), .. } => {
                        self.win_w = ww as u32;
                        self.win_h = wh as u32;
                    }
                    Event::MouseButtonDown { x, y, mouse_btn, .. } => self.on_down(x, y, mouse_btn, video),
                    Event::MouseMotion { x, y, .. } => self.on_motion(x, y),
                    Event::MouseButtonUp { mouse_btn, .. } => self.on_up(mouse_btn, video),
                    Event::MouseWheel { y, mouse_x, mouse_y, .. } => self.on_wheel(y, mouse_x, mouse_y),
                    Event::KeyDown { keycode, keymod, .. } => {
                        if keycode == Some(Keycode::Space) {
                            self.space_down = true;
                        } else if !self.on_key(keycode, keymod, video) {
                            break 'main;
                        }
                    }
                    Event::KeyUp { keycode: Some(Keycode::Space), .. } => self.space_down = false,
                    Event::TextInput { text, .. } => self.on_text(&text),
                    Event::DropFile { filename, .. } => self.on_drop(&filename),
                    _ => {}
                }
            }
            if let Some((_, t)) = &self.toast {
                if t.elapsed().as_secs() > 3 {
                    self.toast = None;
                }
            }
            self.frame()?;
        }
        Ok(())
    }

    /// Headless self-test: synthetic strokes/text/image, then export.
    fn smoke(&mut self) -> Result<(), String> {
        for (i, tool) in [Tool::Pencil, Tool::Pen, Tool::Marker, Tool::OilStick].iter().enumerate() {
            let id = self.doc.alloc_id();
            let y = i as f32 * 110.0 - 165.0;
            let pts: Vec<(f32, f32)> = (0..48)
                .map(|k| (k as f32 * 11.0 - 260.0, y + (k as f32 * 0.4).sin() * 34.0))
                .collect();
            self.doc.add_stroke(Stroke {
                id,
                tool: (*tool).into(),
                color: (i * 3 + 1) % PALETTE_COUNT,
                size: tool.default_size() * 1.6,
                points: pts,
            });
        }
        // undo/redo round-trip on strokes (before image/text are added)
        let n0 = self.doc.data.strokes.len();
        self.doc.undo();
        assert_eq!(self.doc.data.strokes.len(), n0 - 1);
        self.doc.redo();
        assert_eq!(self.doc.data.strokes.len(), n0);
        // text
        let (tex, w, h) = self.make_text_texture("mural smoke 123", 0, 44.0)?;
        let id = self.doc.alloc_id();
        self.doc.add_text(TextMeta {
            id,
            text: "mural smoke 123".into(),
            x: -260.0,
            y: 230.0,
            size: 44.0,
            color: 0,
            w,
            h,
        });
        self.text_tex.insert(id, tex);
        // generated test image: high-contrast B/W checker (vectorize likes contrast)
        let img = image::RgbaImage::from_fn(160, 160, |x, y| {
            if (x / 20 + y / 20) % 2 == 0 {
                image::Rgba([15, 15, 15, 255])
            } else {
                image::Rgba([245, 245, 245, 255])
            }
        });
        img.save("/tmp/mural_test_img.png").map_err(|e| e.to_string())?;
        self.import_image_file(Path::new("/tmp/mural_test_img.png"), 60.0, 200.0);
        // vectorize the checker straight from pixels
        let rgba = image::open("/tmp/mural_test_img.png").map_err(|e| e.to_string())?.to_rgba8();
        let (vpolys, tw, th) = vectorize_rgba(rgba.as_raw(), rgba.width(), rgba.height())?;
        assert!(vpolys.len() >= 16, "checker should trace to many polys, got {}", vpolys.len());
        let vid = self.doc.alloc_id();
        let vm = VectorMeta {
            id: vid,
            polys: vpolys.iter().map(|p| p.pts.clone()).collect(),
            w: tw as f32,
            h: th as f32,
            x: -260.0,
            y: 330.0,
            sw: 220.0,
            sh: 220.0,
            color: 4,
            filled: true,
            anim: AnimKind::Wave,
            speed: 1.0,
            playing: true,
            t: 0.0,
        };
        self.vec_data.insert(vid, vpolys);
        self.doc.add_vector(vm);
        // render several animation frames; frames must differ while playing
        let mut prev: Vec<u8> = vec![];
        for tt in [0.0f32, 0.4, 0.9] {
            if let Some(m) = self.doc.data.vectors.iter_mut().find(|v| v.id == vid) {
                m.t = tt;
            }
            self.vec_tex.remove(&vid);
            assert!(self.ensure_vector_texture(vid));
            let (tex, _) = &self.vec_tex[&vid];
            let _ = tex;
            let meta = self.doc.data.vectors.iter().find(|v| v.id == vid).unwrap().clone();
            let polys = self.vec_data[&vid].clone();
            let (buf, _, _) = render_frame(&meta, &polys, (194, 47, 61));
            assert!(buf.iter().any(|&b| b > 0));
            if !prev.is_empty() {
                assert_ne!(buf, prev, "wave frames should differ");
            }
            prev = buf;
        }
        // draw-on frame renders a partial reveal
        if let Some(m) = self.doc.data.vectors.iter_mut().find(|v| v.id == vid) {
            m.anim = AnimKind::Draw;
            m.t = 0.1;
        }
        self.vec_tex.remove(&vid);
        assert!(self.ensure_vector_texture(vid));
        // undo the vector, redo it
        let nv0 = self.doc.data.vectors.len();
        self.doc.undo();
        assert_eq!(self.doc.data.vectors.len(), nv0 - 1);
        self.doc.redo();
        assert_eq!(self.doc.data.vectors.len(), nv0);
        // save/load roundtrip with the vector present (exercises load_data +
        // sync_media; the following rerasterize() would previously try to
        // allocate ~122TB of tile keys and crash)
        let json = self.doc.to_json().expect("to_json");
        let data = crate::doc::Doc::from_json(&json).expect("from_json");
        let nv = data.vectors.len();
        let ni = data.images.len();
        let ns = data.strokes.len();
        self.doc.load_data(crate::doc::DocData::default());
        self.sync_media();
        assert!(self.doc.data.vectors.is_empty());
        self.doc.load_data(data);
        self.sync_media();
        assert_eq!(self.doc.data.vectors.len(), nv, "vectors survive roundtrip");
        assert_eq!(self.doc.data.images.len(), ni, "images survive roundtrip");
        assert_eq!(self.doc.data.strokes.len(), ns, "strokes survive roundtrip");
        // vector panel clicks must reach ui_click, not the canvas hit-test.
        // Regression: the panel floats above the bottom bar (canvas territory),
        // so clicks fell through, hit empty canvas and deselected the vector.
        let vid = self.doc.data.vectors[0].id;
        self.selection = Some((SelKind::Vector, vid));
        self.frame()?;
        let pr = self.vector_panel_rect().expect("panel rect");
        assert!(self.ui_hit(pr.x() + 10, pr.y() + 10), "panel must count as UI chrome");
        let wave = self
            .panel_hits
            .iter()
            .find(|(_, a)| *a == PanelAction::Anim(AnimKind::Wave))
            .map(|(r, _)| *r)
            .expect("wave button");
        self.ui_click(wave.x() + wave.width() as i32 / 2, wave.y() + wave.height() as i32 / 2);
        assert_eq!(
            self.doc.data.vectors.iter().find(|v| v.id == vid).unwrap().anim,
            AnimKind::Wave,
            "wave click must set the anim mode"
        );
        assert_eq!(self.selection, Some((SelKind::Vector, vid)), "panel click must not deselect");
        // still frame for the export visual
        if let Some(m) = self.doc.data.vectors.iter_mut().find(|v| v.id == vid) {
            m.anim = AnimKind::Still;
            m.playing = false;
            m.t = 0.0;
        }
        self.vec_tex.remove(&vid);
        self.sync_media();
        self.rerasterize();
        let vids: Vec<u64> = self.doc.data.vectors.iter().map(|v| v.id).collect();
        for id in vids {
            assert!(self.ensure_vector_texture(id));
        }
        self.render_export("/tmp/mural_smoke.png")?;
        println!("smoke: exported /tmp/mural_smoke.png");
        // exercise the full interactive frame path (chrome, help, toast)
        self.show_help = true;
        self.toast("smoke toast");
        self.import_buf = Some("/tmp/".to_string());
        for _ in 0..3 {
            self.frame()?;
        }
        self.show_help = false;
        self.import_buf = None;
        println!("smoke: frames ok");
        Ok(())
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let smoke = args.iter().any(|a| a == "--smoke");
    if smoke {
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        std::env::set_var("SDL_AUDIODRIVER", "dummy");
    }
    let sdl = sdl2::init().map_err(|e| e.to_string())?;
    let video = sdl.video().map_err(|e| e.to_string())?;
    let ttf = sdl2::ttf::init().map_err(|e| e.to_string())?;
    let font_path = find_font().ok_or(
        "no TTF font found (tried DejaVu/Liberation/Noto; set MURAL_FONT=/path/to.ttf)",
    )?;
    let window = video
        .window("mural", 1280, 800)
        .position_centered()
        .resizable()
        .build()
        .map_err(|e| e.to_string())?;
    let canvas = window.into_canvas().present_vsync().build().map_err(|e| e.to_string())?;
    let (ww, wh) = canvas.output_size().unwrap_or((1280, 800));
    let tc = canvas.texture_creator();
    let font_big = ttf.load_font(&font_path, 64).map_err(|e| e.to_string())?;
    let font_ui = ttf.load_font(&font_path, 20).map_err(|e| e.to_string())?;
    let mut app = App::new(canvas, &tc, font_big, font_ui, ww, wh);
    if smoke {
        app.smoke()?;
        return Ok(());
    }
    println!("mural — F1 for help");
    app.run(&sdl, &video)
}
