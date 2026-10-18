//! mural — infinite canvas drawing app.
//! marker / pencil / pen / oil stick, 16 colors, image import, text fields.

mod audio;
mod brush;
mod canvas;
mod cmd;
mod doc;
mod palette;
mod vector;

use audio::{AudioClip, CLIP_RADIUS};
use brush::{blit_stamp, box_blur_rgba, clamp_brush_size, clamp_spacing, light_oil_region, smudge_stroke, stamp_height, walk_segment, TipCache, Tool};
use canvas::{union_rects, AnimKind, Camera, ImageMeta, Rect, SelKind, Stroke, TextMeta, ToolSer, VectorMeta};
use cmd::{Command, ParseError, Prop, SetOp, SetVal, ToolName};
use doc::Doc;
use vector::{point_in_poly, rebuild_polys, render_frame, vectorize_rgba, TracedPoly};
use palette::{COUNT as PALETTE_COUNT, COLOR_SETS, PAPER_DOT};
use sdl2::audio::{AudioCallback, AudioDevice, AudioQueue, AudioSpecDesired};
use sdl2::AudioSubsystem;
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
use std::sync::{Arc, Mutex};
use std::time::Instant;

const TILE_PX: f32 = 512.0;
/// Oil-stick grease blur radius (kernel = 2r+1 = 5px), applied only to
/// freshly rasterized oil-stroke dirty rects — never whole tiles.
const OIL_BLUR_R: usize = 2;
const TOP_H: i32 = 44;
const TOOL_W: i32 = 60;
const BOT_H: i32 = 76;

const CHROME: Color = Color::RGB(0x1e, 0x1e, 0x24);
const CHROME2: Color = Color::RGB(0x2a, 0x2a, 0x32);
const INK: Color = Color::RGB(0xe8, 0xe8, 0xee);
const ERR: Color = Color::RGB(0xff, 0x70, 0x60);
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
    Charcoal,
    Smudge,
    Text,
}
impl UiTool {
    fn draw_tool(self) -> Option<Tool> {
        match self {
            UiTool::Pencil => Some(Tool::Pencil),
            UiTool::Pen => Some(Tool::Pen),
            UiTool::Marker => Some(Tool::Marker),
            UiTool::OilStick => Some(Tool::OilStick),
            UiTool::Charcoal => Some(Tool::Charcoal),
            UiTool::Smudge => Some(Tool::Smudge),
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
            UiTool::Charcoal => "K",
            UiTool::Smudge => "S",
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
            UiTool::Charcoal => "Charcoal",
            UiTool::Smudge => "Smudge",
            UiTool::Text => "Text",
        }
    }
}
const TOOLS: [UiTool; 9] = [
    UiTool::Select,
    UiTool::Pan,
    UiTool::Pencil,
    UiTool::Pen,
    UiTool::Marker,
    UiTool::OilStick,
    UiTool::Charcoal,
    UiTool::Smudge,
    UiTool::Text,
];

fn tool_index(t: UiTool) -> usize {
    match t {
        UiTool::Pencil => 0,
        UiTool::Pen => 1,
        UiTool::Marker => 2,
        UiTool::OilStick => 3,
        UiTool::Charcoal => 4,
        UiTool::Smudge => 5,
        _ => 0,
    }
}

/// Compact float formatting for the command line (`24`, `0.35`).
fn fmt_num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
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
    bw: f32, // box w/h, 0 = point text
    bh: f32,
    buf: String,
    existing: Option<u64>, // text id being re-edited
}

/// Vim-style `:` command-line state. History lives on App so it survives
/// across invocations; `hist` is the in-progress navigation cursor.
struct CmdLine {
    buf: String,
    hist: Option<usize>,
}

/// SDL2 capture callback: appends 12 kHz S16 mono samples to a shared buffer.
/// try_lock: never block the audio thread; dropping a chunk beats a stall.
struct CapBuf {
    buf: Arc<Mutex<Vec<i16>>>,
    freq: i32,
}
impl AudioCallback for CapBuf {
    type Channel = i16;
    fn callback(&mut self, input: &mut [i16]) {
        if let Ok(mut b) = self.buf.try_lock() {
            b.extend_from_slice(input);
            let max = (self.freq as usize) * audio::CLIP_MAX_SECS;
            if b.len() > max {
                let drop = b.len() - max;
                b.drain(..drop);
            }
        }
    }
}

/// An in-progress recording. The device is held for its Drop effect:
/// dropping it stops the capture.
struct RecState {
    _dev: AudioDevice<CapBuf>,
    buf: Arc<Mutex<Vec<i16>>>,
    start: Instant,
    freq: i32,
}

/// An in-progress playback. The queue is held alive for its Drop effect.
struct PlayState {
    _q: AudioQueue<i16>,
    clip_id: u64,
    start: Instant,
    seconds: f32,
}

/// `m:ss` for a float second count.
fn fmt_dur(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Filled circle on any window canvas (markers, rec dot).
fn fill_circle(c: &mut Canvas<Window>, cx: i32, cy: i32, r: i32) -> Result<(), String> {
    let r = r.max(1);
    for dy in -r..=r {
        let dx = ((r * r - dy * dy) as f32).sqrt() as i32;
        c.draw_line((cx - dx, cy + dy), (cx + dx, cy + dy)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Circle outline on any window canvas.
fn draw_circle(c: &mut Canvas<Window>, cx: i32, cy: i32, r: i32) -> Result<(), String> {
    let r = r.max(1);
    let mut prev: Option<(i32, i32)> = None;
    for i in 0..=48 {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        let p = (cx + (r as f32 * a.cos()) as i32, cy + (r as f32 * a.sin()) as i32);
        if let Some(q) = prev {
            c.draw_line(q, p).map_err(|e| e.to_string())?;
        }
        prev = Some(p);
    }
    Ok(())
}

/// Procedural mic glyph inside a marker of radius r, in the current draw color.
fn draw_mic_glyph(c: &mut Canvas<Window>, cx: i32, cy: i32, r: i32) -> Result<(), String> {
    let rf = r as f32;
    // mic head: capsule
    let hw = (rf * 0.13) as i32;
    let hy0 = cy - (rf * 0.30) as i32;
    let hy1 = cy + (rf * 0.06) as i32;
    c.fill_rect(SdlRect::new(cx - hw, hy0, (hw * 2) as u32, (hy1 - hy0).max(1) as u32))
        .map_err(|e| e.to_string())?;
    fill_circle(c, cx, hy0, hw)?;
    fill_circle(c, cx, hy1, hw)?;
    // stem + base
    c.draw_line((cx, hy1 + hw), (cx, cy + (rf * 0.26) as i32)).map_err(|e| e.to_string())?;
    c.draw_line(
        (cx - (rf * 0.18) as i32, cy + (rf * 0.26) as i32),
        (cx + (rf * 0.18) as i32, cy + (rf * 0.26) as i32),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorTarget {
    Swatch(usize),
    Bg,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    None,
    Pan,
    MoveItem,
    Stroke,
    Slider,
    Speed,
    PalSlider(u8),
    TextBox,
    ResizeText(u64, u8),
}

#[derive(Clone, Copy, PartialEq)]
enum TextPanelAction {
    Shape(u8),
    Align(u8),
    Size(f32),
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
    sizes: [f32; 6],
    spacings: [f32; 6],
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
    cmdline: Option<CmdLine>,
    cmd_history: Vec<String>,
    cmd_msg: Option<(String, bool, Instant)>, // (text, is_error, shown_at)
    cmd_swallow_colon: bool,
    saved_seq: u64, // doc.seq() at last save/load; `:q` refuses when they differ
    toast: Option<(String, Instant)>,
    start: Instant,
    last_click_at: Option<Instant>,
    last_click_item: Option<(SelKind, u64)>,
    win_w: u32,
    win_h: u32,
    current_path: Option<std::path::PathBuf>,
    color_edit: Option<ColorTarget>,
    ui_fade: f32,
    label_fade: f32,
    text_panel_hits: Vec<(SdlRect, TextPanelAction)>,
    tbox: Option<(f32, f32, f32, f32)>,
    // voice clips
    audio_ss: Option<AudioSubsystem>,
    rec: Option<RecState>,
    playing: Option<PlayState>,
    mouse_sx: i32,
    mouse_sy: i32,
}

fn tiles_in_rect(r: &Rect) -> Vec<(i32, i32)> {
    // Clamp the tile range: a huge rect (e.g. from a programming slip) must
    // never turn into a terabytes-large Vec and take the app down.
    // (i64 diff: an i32 subtraction could itself overflow on absurd ranges.)
    const MAX_TILES_PER_AXIS: i64 = 4000; // 4000^2 tiles x 512px = plenty
    let x0 = (r.x0 / TILE_PX).floor() as i32;
    let y0 = (r.y0 / TILE_PX).floor() as i32;
    let x1 = (r.x1 / TILE_PX).floor() as i32;
    let y1 = (r.y1 / TILE_PX).floor() as i32;
    if x1 as i64 - x0 as i64 > MAX_TILES_PER_AXIS || y1 as i64 - y0 as i64 > MAX_TILES_PER_AXIS {
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
            sizes: [Tool::Pencil.default_size(), Tool::Pen.default_size(), Tool::Marker.default_size(), Tool::OilStick.default_size(), Tool::Charcoal.default_size(), Tool::Smudge.default_size()],
            spacings: [Tool::Pencil.spacing(), Tool::Pen.spacing(), Tool::Marker.spacing(), Tool::OilStick.spacing(), Tool::Charcoal.spacing(), Tool::Smudge.spacing()],
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
            cmdline: None,
            cmd_history: Vec::new(),
            cmd_msg: None,
            cmd_swallow_colon: false,
            saved_seq: 0,
            toast: None,
            current_path: None,
            color_edit: None,
            ui_fade: 1.0,
            label_fade: 1.0,
            text_panel_hits: vec![],
            tbox: None,
            audio_ss: None,
            rec: None,
            playing: None,
            mouse_sx: win_w as i32 / 2,
            mouse_sy: win_h as i32 / 2,
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
        self.sizes[i] = clamp_brush_size(s);
    }
    /// Per-tool stamp spacing, honoring the `:set spacing=` override.
    fn brush_spacing(spacings: &[f32; 6], tool: Tool) -> f32 {
        spacings[tool.index()]
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
        let spacings = self.spacings;
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
            // Paint-thickness heightmap for oil relief lighting. Rebuilt
            // deterministically from the strokes on every pass (like the
            // color tiles), so layering/undo/load can't drift it. Only
            // allocated when oil strokes touch this tile.
            let has_oil = doc
                .data
                .strokes
                .iter()
                .any(|s| s.tool == ToolSer::OilStick && s.bbox().intersects(&trect));
            let mut height: Option<Vec<u8>> = if has_oil {
                Some(vec![0u8; TILE_PX as usize * TILE_PX as usize])
            } else {
                None
            };
            // (tile-local rect, raw RGBA8888 pixels) to blur+light after rasterizing
            let mut blur_jobs: Vec<(SdlRect, Vec<u8>)> = Vec::new();
            // clear first: a tile holding only a smudge stroke still redraws
            canvas
                .with_texture_canvas(tile, |c| {
                    c.set_draw_color(Color::RGBA(0, 0, 0, 0));
                    c.clear();
                })
                .expect("clear tile");
            let mut oil_rects: Vec<Rect> = Vec::new();
            // Strokes rasterize in order. Paint strokes go to the GPU in
            // runs; each smudge stroke is applied on the CPU between runs
            // (read the tile back, smear, write back), so it samples
            // everything rasterized beneath it in stroke order — including
            // its own earlier dabs — while strokes drawn after it still
            // paint over it. Same stroke list -> same pixels, always.
            let mut run: Vec<&Stroke> = Vec::new();
            for s in &doc.data.strokes {
                let b = s.bbox();
                if !b.intersects(&trect) {
                    continue;
                }
                if s.tool == ToolSer::Smudge {
                    rasterize_paint_run(
                        canvas,
                        tc,
                        tips,
                        &mut *tile,
                        &run,
                        origin,
                        &doc.data.palette,
                        &mut height,
                        &mut oil_rects,
                        spacings,
                    );
                    run.clear();
                    rasterize_smudge_stroke(canvas, &mut *tile, s, origin, spacings[Tool::Smudge.index()]);
                } else {
                    run.push(s);
                }
            }
            rasterize_paint_run(
                canvas,
                tc,
                tips,
                &mut *tile,
                &run,
                origin,
                &doc.data.palette,
                &mut height,
                &mut oil_rects,
                spacings,
            );
            // grease: melt oil-stroke edges into the canvas.
            // Deterministic — EVERY re-rasterize blurs ALL oil strokes
            // on the tile. (The old design blurred only newly-drawn
            // regions, but tiles redraw from scratch, so layering a new
            // stroke silently un-blurred older ones, leaving shifting
            // rectangular sharp/blurry patches.)
            canvas
                .with_texture_canvas(tile, |c| {
                    // grease: melt freshly rasterized oil edges into the canvas
                    for r in &union_rects(oil_rects) {
                        if !r.intersects(&trect) {
                            continue;
                        }
                        // intersect, expand for blur context, clamp to the tile
                        let ix0 = r.x0.max(trect.x0) - origin.0;
                        let iy0 = r.y0.max(trect.y0) - origin.1;
                        let ix1 = r.x1.min(trect.x1) - origin.0;
                        let iy1 = r.y1.min(trect.y1) - origin.1;
                        let x0 = (ix0 - OIL_BLUR_R as f32).max(0.0) as i32;
                        let y0 = (iy0 - OIL_BLUR_R as f32).max(0.0) as i32;
                        let x1 = (ix1 + OIL_BLUR_R as f32).min(TILE_PX) as i32;
                        let y1 = (iy1 + OIL_BLUR_R as f32).min(TILE_PX) as i32;
                        if x1 <= x0 || y1 <= y0 {
                            continue;
                        }
                        let xr = SdlRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
                        // readback failures are non-fatal: skip the blur
                        if let Ok(px) = c.read_pixels(xr, PixelFormatEnum::RGBA8888) {
                            if px.len() == (xr.width() * xr.height() * 4) as usize {
                                blur_jobs.push((xr, px));
                            }
                        }
                    }
                })
                .expect("oil readback");
            for (xr, px) in blur_jobs {
                let w = xr.width() as usize;
                let h = xr.height() as usize;
                let mut blurred = box_blur_rgba(&px, w, h, OIL_BLUR_R);
                // relief lighting from the paint-thickness heightmap:
                // ridges catch specular highlights, valleys shade, thin
                // washes stay matte
                if let Some(hgt) = height.as_ref() {
                    light_oil_region(
                        &mut blurred,
                        hgt,
                        TILE_PX as usize,
                        xr.x() as usize,
                        xr.y() as usize,
                        w,
                        h,
                    );
                }
                let _ = tile.update(xr, &blurred, w * 4);
            }
        }
    }
}

/// Rasterize one run of paint (non-smudge) strokes onto `tile`, in order.
/// Collects oil bboxes for the grease pass; oil dabs deposit height.
fn rasterize_paint_run<'tc>(
    canvas: &mut Canvas<Window>,
    tc: &'tc TextureCreator<WindowContext>,
    tips: &mut TipCache<'tc>,
    tile: &mut Texture<'tc>,
    run: &[&Stroke],
    origin: (f32, f32),
    palette: &[[u8; 3]; 16],
    height: &mut Option<Vec<u8>>,
    oil_rects: &mut Vec<Rect>,
    spacings: [f32; 6],
) {
    if run.is_empty() {
        return;
    }
    canvas
        .with_texture_canvas(tile, |c| {
            for s in run {
                rasterize_stroke(
                    c,
                    tc,
                    tips,
                    s,
                    origin,
                    palette,
                    height.as_mut().map(|v| &mut v[..]),
                    spacings[Tool::from(s.tool).index()],
                );
                if s.tool == ToolSer::OilStick {
                    oil_rects.push(s.bbox());
                }
            }
        })
        .expect("rasterize run");
}

/// Apply one smudge stroke on the CPU: read back the tile region it
/// touches, smear its dabs in stroke order over those pixels, write back.
/// Each dab samples the tile as left by the earlier dabs (a finger drags
/// its own smear). Readback failures are non-fatal — the stroke is skipped
/// for this pass (same policy as the oil blur). The region is clamped to
/// the tile, and the dab kernel bounds-checks every access, so off-tile
/// dabs can never read or write out of bounds.
fn rasterize_smudge_stroke(
    canvas: &mut Canvas<Window>,
    tile: &mut Texture,
    s: &Stroke,
    origin: (f32, f32),
    spacing: f32, // per-tool stamp spacing override (App::spacings)
) {
    let b = s.bbox();
    // bbox already expands by the full dab diameter; pad one more pixel
    // for float safety, then clamp to the tile
    let rx0 = (b.x0 - origin.0 - 1.0).max(0.0) as i32;
    let ry0 = (b.y0 - origin.1 - 1.0).max(0.0) as i32;
    let rx1 = ((b.x1 - origin.0 + 1.0).min(TILE_PX)).ceil() as i32;
    let ry1 = ((b.y1 - origin.1 + 1.0).min(TILE_PX)).ceil() as i32;
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }
    let xr = SdlRect::new(rx0, ry0, (rx1 - rx0) as u32, (ry1 - ry0) as u32);
    // read back the tile region; failures are non-fatal (same policy as
    // the oil blur) — the stroke is skipped for this pass
    let mut px: Option<Vec<u8>> = None;
    canvas
        .with_texture_canvas(tile, |c| {
            if let Ok(p) = c.read_pixels(xr, PixelFormatEnum::RGBA8888) {
                px = Some(p);
            }
        })
        .expect("smudge readback");
    let mut px = match px {
        Some(p) => p,
        None => return,
    };
    let (w, h) = (xr.width() as usize, xr.height() as usize);
    if px.len() != w * h * 4 {
        return;
    }
    // dabs in region-local coords; the buffer evolves dab by dab
    let pts: Vec<(f32, f32)> = s
        .points
        .iter()
        .map(|&(x, y)| (x - origin.0 - rx0 as f32, y - origin.1 - ry0 as f32))
        .collect();
    smudge_stroke(&mut px, w, h, &pts, s.size, spacing);
    let _ = tile.update(xr, &px, w * 4);
}

fn rasterize_stroke<'a>(
    c: &mut Canvas<Window>,
    tc: &'a TextureCreator<WindowContext>,
    tips: &mut TipCache<'a>,
    s: &Stroke,
    origin: (f32, f32),
    pal: &[[u8; 3]; 16],
    height: Option<&mut [u8]>, // tile-local paint-thickness map (TILE_PX^2); oil dabs deposit here
    spacing: f32, // per-tool stamp spacing override (App::spacings)
) {
    if s.points.is_empty() {
        return;
    }
    let tool = Tool::from(s.tool);
    let is_oil = matches!(tool, Tool::OilStick);
    let d = s.size.max(2.0) as u32;
    let tip = tips.get(tc, tool, d).expect("tip");
    let p = pal[s.color % 16];
    let col = Color::RGB(p[0], p[1], p[2]);
    tip.set_color_mod(col.r, col.g, col.b);
    let mut hbuf = height;
    let tw = TILE_PX as usize;
    let mut stamp = |c: &mut Canvas<Window>, x: f32, y: f32, dd: f32| {
        blit_stamp(c, tip, x - origin.0, y - origin.1, dd).expect("stamp");
        if is_oil {
            if let Some(h) = hbuf.as_mut() {
                stamp_height(h, tw, x - origin.0, y - origin.1, dd);
            }
        }
    };
    if s.points.len() == 1 {
        let (x, y) = s.points[0];
        stamp(c, x, y, s.size);
        return;
    }
    for w in s.points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        walk_segment(x0, y0, x1, y1, s.size, spacing, |x, y, dd| {
            stamp(c, x, y, dd)
        });
    }
}

// ================= events =================
impl<'tc, 'ttf> App<'tc, 'ttf> {
    fn pal(&self, i: usize) -> Color {
        let p = self.doc.data.palette[i % PALETTE_COUNT];
        Color::RGB(p[0], p[1], p[2])
    }
    fn bg_color(&self) -> Color {
        let b = self.doc.data.bg;
        Color::RGB(b[0], b[1], b[2])
    }
    fn fade_col(&self, c: Color) -> Color {
        Color::RGBA(c.r, c.g, c.b, (c.a as f32 * self.ui_fade) as u8)
    }
    fn cc(&mut self, c: Color) {
        let fc = self.fade_col(c);
        self.canvas.set_draw_color(fc);
    }
    fn edit_rgb(&self) -> [u8; 3] {
        match self.color_edit {
            Some(ColorTarget::Swatch(i)) => self.doc.data.palette[i % PALETTE_COUNT],
            Some(ColorTarget::Bg) => self.doc.data.bg,
            None => [0, 0, 0],
        }
    }
    fn set_edit_channel(&mut self, ch: u8, v: u8) {
        match self.color_edit {
            Some(ColorTarget::Swatch(i)) => {
                self.doc.data.palette[i % PALETTE_COUNT][ch as usize] = v;
            }
            Some(ColorTarget::Bg) => {
                self.doc.data.bg[ch as usize] = v;
            }
            None => {}
        }
    }
    fn bg_rect(&self) -> SdlRect {
        SdlRect::new(self.win_w as i32 - 368, 8, 60, 28)
    }
    fn color_popup_rect(&self) -> SdlRect {
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        SdlRect::new((w - 320) / 2, (h - 230) / 2, 320, 230)
    }
    fn pal_slider_rect(&self, ch: u8) -> SdlRect {
        let pr = self.color_popup_rect();
        SdlRect::new(pr.x() + 70, pr.y() + 78 + ch as i32 * 40, (pr.width() as i32 - 90).max(10) as u32, 16)
    }
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
        if let Some(r) = self.text_panel_rect() {
            if r.contains_point((sx, sy)) {
                return true;
            }
        }
        if self.color_edit.is_some() && self.color_popup_rect().contains_point((sx, sy)) {
            return true;
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

    /// Screen rect of the text panel, when a text is selected.
    fn text_panel_rect(&self) -> Option<SdlRect> {
        match self.selection {
            Some((SelKind::Text, _)) => {
                let (w, h) = (self.win_w as i32, self.win_h as i32);
                let pw = 560i32;
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
    /// Chip showing the active color set; click = next set, right-click = prev.
    fn set_chip_rect(&self) -> SdlRect {
        SdlRect::new(
            TOOL_W + 12 + 16 * 46 + 14,
            self.win_h as i32 - 62,
            132,
            40,
        )
    }
    fn set_name(&self) -> &'static str {
        COLOR_SETS[self.doc.data.color_set % COLOR_SETS.len()].name
    }
    fn swatch_name(&self, i: usize) -> &'static str {
        COLOR_SETS[self.doc.data.color_set % COLOR_SETS.len()].names[i % PALETTE_COUNT]
    }
    fn cycle_color_set(&mut self, dir: i32) {
        let n = COLOR_SETS.len() as i32;
        let cur = self.doc.data.color_set as i32;
        let next = ((cur + dir) % n + n) % n;
        self.doc.apply_color_set(next as usize);
        self.toast(&format!("color set: {}", self.set_name()));
    }
    fn slider_rect(&self) -> SdlRect {
        SdlRect::new(self.win_w as i32 - 300, self.win_h as i32 - 56, 220, 28)
    }

    /// Hit test topmost-first (text > paint > images, matching draw order top-down).
    fn hit_test(&self, wx: f32, wy: f32) -> Option<(SelKind, u64)> {
        for c in self.doc.data.clips.iter().rev() {
            let (dx, dy) = self.move_delta_for(SelKind::Clip, c.id);
            let ddx = wx - (c.x as f32 + dx);
            let ddy = wy - (c.y as f32 + dy);
            if (ddx * ddx + ddy * ddy).sqrt() < CLIP_RADIUS as f32 {
                return Some((SelKind::Clip, c.id));
            }
        }
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
        // right-click on a swatch / bg swatch opens the color editor;
        // right-click on the set chip cycles to the previous color set
        if btn == MouseButton::Right {
            for i in 0..PALETTE_COUNT {
                if self.swatch_rect(i).contains_point((sx, sy)) {
                    self.color_edit = Some(ColorTarget::Swatch(i));
                    return;
                }
            }
            if self.bg_rect().contains_point((sx, sy)) {
                self.color_edit = Some(ColorTarget::Bg);
                return;
            }
            if self.set_chip_rect().contains_point((sx, sy)) {
                self.cycle_color_set(-1);
                return;
            }
        }
        // UI chrome first
        if self.ui_hit(sx, sy) {
            self.ui_click(sx, sy);
            return;
        }
        if self.text_edit.is_some() || self.import_buf.is_some() || self.cmdline.is_some() {
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
                // resize handles on the selected bounded text come first
                if let Some((SelKind::Text, tid)) = self.selection {
                    let mut handled = false;
                    for corner in 0..4u8 {
                        if let Some((hx, hy)) = self.text_handle_pos(tid, corner) {
                            if (sx - hx).abs() < 12 && (sy - hy).abs() < 12 {
                                self.drag = DragKind::ResizeText(tid, corner);
                                handled = true;
                                break;
                            }
                        }
                    }
                    if handled {
                        return;
                    }
                }
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
                                bw: t.box_w,
                                bh: t.box_h,
                                buf: t.text.clone(),
                                existing: Some(id),
                            });
                            video.text_input().start();
                            return;
                        }
                    }
                    if dbl && kind == SelKind::Clip {
                        self.selection = Some((kind, id));
                        self.toggle_play(id);
                        return;
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
                // press -> potential box drag; release without dragging -> point text
                self.drag = DragKind::TextBox;
                self.tbox = Some((wx, wy, wx, wy));
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
            SelKind::Clip => self.doc.data.clips.iter().find(|c| c.id == id).map(|c| (c.x as f32, c.y as f32)).unwrap_or((0.0, 0.0)),
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
            DragKind::TextBox => {
                let (wx, wy) = self.cam.screen_to_world(sx as f32, sy as f32, self.win_w as f32, self.win_h as f32);
                if let Some(tb) = &mut self.tbox {
                    tb.2 = wx;
                    tb.3 = wy;
                }
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
            DragKind::PalSlider(ch) => {
                for ch2 in 0..3u8 {
                    let r = self.pal_slider_rect(ch2);
                    if ch2 == ch {
                        let t = ((sx - r.x()) as f32 / r.width() as f32).clamp(0.0, 1.0);
                        self.set_edit_channel(ch2, (t * 255.0).round() as u8);
                    }
                }
            }
            DragKind::ResizeText(id, corner) => {
                let (wx, wy) = self.cam.screen_to_world(sx as f32, sy as f32, self.win_w as f32, self.win_h as f32);
                let mut tm = match self.doc.data.texts.iter().find(|t| t.id == id).cloned() {
                    Some(t) => t,
                    None => return,
                };
                if tm.box_w <= 0.0 {
                    return;
                }
                let (x0, y0, x1, y1) = (tm.x, tm.y, tm.x + tm.box_w, tm.y + tm.box_h);
                match corner {
                    0 => {
                        let nx = wx.min(x1 - 40.0);
                        let ny = wy.min(y1 - 28.0);
                        tm.x = nx; tm.y = ny; tm.box_w = x1 - nx; tm.box_h = y1 - ny;
                    }
                    1 => {
                        let nx = wx.max(x0 + 40.0);
                        let ny = wy.min(y1 - 28.0);
                        tm.y = ny; tm.box_w = nx - x0; tm.box_h = y1 - ny;
                    }
                    2 => {
                        let nx = wx.min(x1 - 40.0);
                        let ny = wy.max(y0 + 28.0);
                        tm.x = nx; tm.box_w = x1 - nx; tm.box_h = ny - y0;
                    }
                    _ => {
                        let nx = wx.max(x0 + 40.0);
                        let ny = wy.max(y0 + 28.0);
                        tm.box_w = nx - x0; tm.box_h = ny - y0;
                    }
                }
                if let Ok((tex, w, h)) = self.render_text(&tm) {
                    tm.w = w;
                    tm.h = h;
                    if let Some(dst) = self.doc.data.texts.iter_mut().find(|t| t.id == id) {
                        *dst = tm;
                    }
                    self.text_tex.insert(id, tex);
                }
            }
            DragKind::None => {}
        }
    }

    fn on_up(&mut self, _btn: MouseButton, video: &sdl2::VideoSubsystem) {
        match self.drag {
            DragKind::TextBox => {
                if let Some((x0, y0, x1, y1)) = self.tbox.take() {
                    let (bw, bh) = ((x1 - x0).abs(), (y1 - y0).abs());
                    let (x, y) = (x0.min(x1), y0.min(y1));
                    if bw > 12.0 / self.cam.zoom && bh > 12.0 / self.cam.zoom {
                        self.text_edit = Some(TextEdit { x, y, bw, bh, buf: String::new(), existing: None });
                    } else {
                        self.text_edit = Some(TextEdit { x: x0, y: y0, bw: 0.0, bh: 0.0, buf: String::new(), existing: None });
                    }
                    video.text_input().start();
                }
            }
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
                        SelKind::Clip => self.doc.move_clip(self.move_id, dx, dy),
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

        // --- command-line mode takes over the keyboard ---
        if self.cmdline.is_some() {
            match keycode {
                Some(Keycode::Return) | Some(Keycode::KpEnter) => return self.exec_cmdline(video),
                Some(Keycode::Escape) => self.cancel_cmdline(video),
                Some(Keycode::Backspace) => {
                    if let Some(c) = self.cmdline.as_mut() {
                        c.buf.pop();
                    }
                }
                Some(Keycode::Up) => self.cmd_hist_step(-1),
                Some(Keycode::Down) => self.cmd_hist_step(1),
                _ => {}
            }
            return true;
        }

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
                if self.rec.is_some() {
                    self.cancel_record();
                } else if self.color_edit.is_some() {
                    self.color_edit = None;
                } else if self.show_help {
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
            Some(Keycode::K) if !ctrl => self.set_tool(UiTool::Charcoal),
            Some(Keycode::S) if !ctrl => self.set_tool(UiTool::Smudge),
            Some(Keycode::T) if !ctrl => self.set_tool(UiTool::Text),
            Some(Keycode::I) if !ctrl => {
                self.import_buf = Some(String::new());
                video.text_input().start();
            }
            Some(Keycode::X) if !ctrl => self.vectorize_selection(),
            Some(Keycode::A) if !ctrl => self.toggle_record(),
            Some(Keycode::C) if !ctrl => self.cycle_color_set(if shift { -1 } else { 1 }),
            Some(Keycode::LeftBracket) => self.set_brush_size(self.brush_size() / 1.25),
            Some(Keycode::RightBracket) => self.set_brush_size(self.brush_size() * 1.25),
            Some(Keycode::Z) if ctrl && !shift => self.do_undo(),
            Some(Keycode::Y) if ctrl => self.do_redo(),
            Some(Keycode::Z) if ctrl && shift => self.do_redo(),
            Some(Keycode::S) if ctrl => self.save_doc(),
            Some(Keycode::E) if ctrl => self.export_png(),
            Some(Keycode::O) if ctrl => self.load_doc(),
            Some(Keycode::Q) if ctrl => return false,
            Some(Keycode::Semicolon) if shift && !ctrl => self.open_cmdline(video),
            Some(Keycode::Colon) if !ctrl => self.open_cmdline(video),
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
        if self.cmdline.is_some() {
            // swallow the ":" that opened the line (it arrives as TextInput)
            if self.cmd_swallow_colon {
                self.cmd_swallow_colon = false;
                if text == ":" {
                    return;
                }
            }
            if let Some(c) = self.cmdline.as_mut() {
                c.buf.push_str(text);
            }
            return;
        }
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
                    if let Some(tm) = self.doc.data.texts.iter().find(|t| t.id == id).cloned() {
                        let mut tm2 = tm;
                        tm2.text = text;
                        // keep the existing box/align/shape; re-render
                        if let Ok((tex, w, h)) = self.render_text(&tm2) {
                            tm2.w = w;
                            tm2.h = h;
                            if let Some(dst) = self.doc.data.texts.iter_mut().find(|t| t.id == id) {
                                *dst = tm2;
                            }
                            self.text_tex.insert(id, tex);
                        }
                    }
                } else {
                    let tm = TextMeta {
                        id: self.doc.alloc_id(),
                        text,
                        x: ed.x,
                        y: ed.y,
                        size: 32.0,
                        color: self.color_idx,
                        w: ed.bw,
                        h: ed.bh,
                        box_w: ed.bw,
                        box_h: ed.bh,
                        align: 0,
                        shape: 0,
                    };
                    if let Ok((tex, w, h)) = self.render_text(&tm) {
                        let mut tm2 = tm;
                        tm2.w = w;
                        tm2.h = h;
                        let id = tm2.id;
                        self.doc.add_text(tm2);
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

    // ================= vim-style `:` command line =================

    fn open_cmdline(&mut self, video: &sdl2::VideoSubsystem) {
        self.cmdline = Some(CmdLine { buf: String::new(), hist: None });
        self.cmd_swallow_colon = true;
        self.cmd_msg = None;
        video.text_input().start();
    }

    fn cancel_cmdline(&mut self, video: &sdl2::VideoSubsystem) {
        self.cmdline = None;
        self.cmd_swallow_colon = false;
        video.text_input().stop();
    }

    fn cmd_message(&mut self, msg: String, is_error: bool) {
        if !msg.is_empty() {
            self.cmd_msg = Some((msg, is_error, Instant::now()));
        }
    }

    fn cmd_hist_step(&mut self, dir: i32) {
        if self.cmd_history.is_empty() {
            return;
        }
        let n = self.cmd_history.len();
        let c = match self.cmdline.as_mut() {
            Some(c) => c,
            None => return,
        };
        let next = match c.hist {
            None if dir < 0 => n - 1,
            None => return,
            Some(i) => {
                let j = i as i32 + dir;
                if j < 0 {
                    return;
                }
                if j >= n as i32 {
                    c.hist = None;
                    c.buf.clear();
                    return;
                }
                j as usize
            }
        };
        c.hist = Some(next);
        c.buf = self.cmd_history[next].clone();
    }

    /// Execute the command line. Returns false when the app should quit.
    fn exec_cmdline(&mut self, video: &sdl2::VideoSubsystem) -> bool {
        let line = self.cmdline.as_ref().map(|c| c.buf.clone()).unwrap_or_default();
        let line = line.trim().to_string();
        self.cmdline = None;
        self.cmd_swallow_colon = false;
        video.text_input().stop();
        if !line.is_empty() && self.cmd_history.last().map(|h| h != &line).unwrap_or(true) {
            self.cmd_history.push(line.clone());
        }
        let cmd = match cmd::parse(&line) {
            Ok(c) => c,
            Err(ParseError::Empty) => return true,
            Err(e) => {
                self.cmd_message(e.message(), true);
                return true;
            }
        };
        let (msg, is_error, quit) = self.run_command(cmd);
        self.cmd_message(msg, is_error);
        !quit
    }

    /// Run a parsed command. Returns (message, is_error, quit).
    fn run_command(&mut self, cmd: Command) -> (String, bool, bool) {
        match cmd {
            Command::Write(path) => {
                let p = path.map(PathBuf::from).unwrap_or_else(|| {
                    self.current_path
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("mural.json"))
                });
                match self.save_to(&p) {
                    Ok(m) => {
                        self.saved_seq = self.doc.seq();
                        (m, false, false)
                    }
                    Err(e) => (format!("E: {e}"), true, false),
                }
            }
            Command::Quit { force } => {
                if !force && self.doc.seq() != self.saved_seq {
                    ("E: unsaved changes (use :q!)".to_string(), true, false)
                } else {
                    (String::new(), false, true)
                }
            }
            Command::Edit(path) => match self.load_path(Path::new(&path)) {
                Ok(m) => (m, false, false), // load_path snapshots saved_seq
                Err(e) => (format!("E: {e}"), true, false),
            },
            Command::Tool(t) => {
                let ui = match t {
                    ToolName::Pencil => UiTool::Pencil,
                    ToolName::Pen => UiTool::Pen,
                    ToolName::Marker => UiTool::Marker,
                    ToolName::OilStick => UiTool::OilStick,
                    ToolName::Charcoal => UiTool::Charcoal,
                    ToolName::Smudge => UiTool::Smudge,
                };
                self.set_tool(ui);
                (format!("tool: {}", ui.label().to_lowercase()), false, false)
            }
            Command::Set { prop, op, value } => self.cmd_set(prop, op, value),
            Command::Color(i) => {
                self.color_idx = i.min(PALETTE_COUNT - 1);
                (format!("color={}", self.color_idx), false, false)
            }
            Command::Export(path) => match self.render_export(&path) {
                Ok(_) => (format!("exported {path}"), false, false),
                Err(e) => (format!("E: {e}"), true, false),
            },
            Command::Undo => match self.doc.undo() {
                Some(_) => {
                    self.sync_media();
                    self.selection = None;
                    ("undo".to_string(), false, false)
                }
                None => ("nothing to undo".to_string(), false, false),
            },
            Command::Redo => match self.doc.redo() {
                Some(_) => {
                    self.sync_media();
                    self.selection = None;
                    ("redo".to_string(), false, false)
                }
                None => ("nothing to redo".to_string(), false, false),
            },
            Command::Help => {
                self.show_help = true;
                (String::new(), false, false)
            }
        }
    }

    /// `:set` get/set/+=/-= on tool properties.
    fn cmd_set(&mut self, prop: Prop, op: SetOp, value: Option<SetVal>) -> (String, bool, bool) {
        let fnum = |v: Option<SetVal>| -> Result<Option<f32>, String> {
            match v {
                None => Ok(None),
                Some(SetVal::F(x)) => Ok(Some(x)),
                Some(_) => Err("E: expected a number".to_string()),
            }
        };
        let unum = |v: Option<SetVal>| -> Result<Option<usize>, String> {
            match v {
                None => Ok(None),
                Some(SetVal::U(x)) => Ok(Some(x)),
                Some(_) => Err("E: expected an integer".to_string()),
            }
        };
        match prop {
            Prop::Size => {
                let v = match fnum(value) {
                    Ok(v) => v,
                    Err(e) => return (e, true, false),
                };
                let i = tool_index(self.tool);
                let new = match (op, v) {
                    (SetOp::Get, _) => self.sizes[i],
                    (SetOp::Set, Some(x)) => x,
                    (SetOp::Add, Some(x)) => self.sizes[i] + x,
                    (SetOp::Sub, Some(x)) => self.sizes[i] - x,
                    _ => return ("E: missing value".to_string(), true, false),
                };
                self.sizes[i] = clamp_brush_size(new);
                if !matches!(op, SetOp::Get) {
                    self.doc.touch();
                }
                (format!("size={}", fmt_num(self.sizes[i])), false, false)
            }
            Prop::Spacing => {
                let v = match fnum(value) {
                    Ok(v) => v,
                    Err(e) => return (e, true, false),
                };
                let i = tool_index(self.tool);
                let new = match (op, v) {
                    (SetOp::Get, _) => self.spacings[i],
                    (SetOp::Set, Some(x)) => x,
                    (SetOp::Add, Some(x)) => self.spacings[i] + x,
                    (SetOp::Sub, Some(x)) => self.spacings[i] - x,
                    _ => return ("E: missing value".to_string(), true, false),
                };
                self.spacings[i] = clamp_spacing(new);
                if !matches!(op, SetOp::Get) {
                    self.doc.touch();
                }
                (format!("spacing={}", fmt_num(self.spacings[i])), false, false)
            }
            Prop::Color => {
                let v = match unum(value) {
                    Ok(v) => v,
                    Err(e) => return (e, true, false),
                };
                match (op, v) {
                    (SetOp::Get, _) => {}
                    (SetOp::Set, Some(x)) => self.color_idx = x.min(PALETTE_COUNT - 1),
                    (SetOp::Add, Some(x)) => {
                        self.color_idx = (self.color_idx + x).min(PALETTE_COUNT - 1)
                    }
                    (SetOp::Sub, Some(x)) => self.color_idx = self.color_idx.saturating_sub(x),
                    _ => return ("E: missing value".to_string(), true, false),
                }
                if !matches!(op, SetOp::Get) {
                    self.doc.touch();
                }
                (format!("color={}", self.color_idx), false, false)
            }
        }
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
                SelKind::Clip => {
                    self.doc.delete_clip(id);
                    if self.playing.as_ref().map(|p| p.clip_id == id).unwrap_or(false) {
                        self.playing = None;
                    }
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
                if let Ok((tex, _, _)) = self.render_text(t) {
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
        self.poll_audio();
        // fade the chrome while typing or drawing so it never blocks the canvas
        let target = if self.text_edit.is_some() || self.active_stroke.is_some() { 0.22 } else { 1.0 };
        let k = (dt * 9.0).min(1.0);
        self.ui_fade += (target - self.ui_fade) * k;
        for v in self.doc.data.vectors.iter_mut() {
            if v.playing && v.anim.animated() {
                v.t += dt;
            }
        }
        self.canvas.set_draw_color(self.bg_color());
        self.canvas.clear();
        self.draw_grid();
        self.draw_tiles()?;
        self.draw_images()?;
        self.draw_vectors()?;
        self.draw_texts()?;
        self.draw_clips()?;
        self.draw_active_stroke()?;
        self.draw_selection();
        self.draw_tbox_rubber()?;
        self.draw_text_edit()?;
        self.canvas.set_blend_mode(BlendMode::Blend);
        self.draw_chrome()?;
        self.draw_rec_indicator()?;
        self.draw_vector_panel()?;
        self.draw_text_panel()?;
        self.canvas.set_blend_mode(BlendMode::None);
        self.draw_color_popup()?;
        if self.show_help {
            self.draw_help()?;
        }
        self.draw_import_bar()?;
        self.draw_cmdline()?;
        self.draw_toast()?;
        self.canvas.present();
        Ok(())
    }

    // ================= voice clips =================
    // 12 kHz S16 mono capture, μ-law companded to 8-bit = 96 kbps exactly.

    fn clip_spec() -> AudioSpecDesired {
        AudioSpecDesired { freq: Some(audio::SAMPLE_RATE), channels: Some(1), samples: Some(1024) }
    }

    fn toggle_record(&mut self) {
        if self.rec.is_some() {
            self.stop_record();
        } else {
            self.start_record();
        }
    }

    fn start_record(&mut self) {
        if self.audio_ss.is_none() {
            self.toast("no audio subsystem");
            return;
        }
        let ss = self.audio_ss.clone().unwrap();
        let buf = Arc::new(Mutex::new(Vec::new()));
        let buf2 = buf.clone();
        match ss.open_capture(None::<&str>, &Self::clip_spec(), |ospec| CapBuf {
            buf: buf2,
            freq: ospec.freq,
        }) {
            Ok(dev) => {
                let freq = dev.spec().freq;
                dev.resume();
                self.rec = Some(RecState { _dev: dev, buf, start: Instant::now(), freq });
                // stop any playback while recording
                self.playing = None;
            }
            Err(e) => self.toast(&format!("no mic ({})", e)),
        }
    }

    fn stop_record(&mut self) {
        let rec = match self.rec.take() {
            Some(r) => r,
            None => return,
        };
        // dev drops here -> capture stops
        let samples: Vec<i16> = rec.buf.lock().map(|b| b.clone()).unwrap_or_default();
        let seconds = samples.len() as f32 / rec.freq.max(1) as f32;
        if samples.is_empty() || seconds < 0.3 {
            self.toast("clip too short");
            return;
        }
        let ulaw: Vec<u8> = samples.iter().map(|&s| audio::mulaw_encode(s)).collect();
        let id = self.doc.alloc_id();
        // drop at the mouse; screen center if the pointer is over UI chrome
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let (wx, wy) = if self.ui_hit(self.mouse_sx, self.mouse_sy) {
            self.cam.screen_to_world(w / 2.0, h / 2.0, w, h)
        } else {
            self.cam.screen_to_world(self.mouse_sx as f32, self.mouse_sy as f32, w, h)
        };
        let clip = AudioClip {
            id,
            x: wx as f64,
            y: wy as f64,
            seconds,
            data: audio::b64encode(&ulaw),
        };
        self.doc.add_clip(clip);
        self.selection = Some((SelKind::Clip, id));
        let s = seconds as u32;
        self.toast(&format!("clip {}:{:02}", s / 60, s % 60));
    }

    fn cancel_record(&mut self) {
        if self.rec.take().is_some() {
            self.toast("recording cancelled");
        }
    }

    /// Double-click toggles; Space plays (never stops, so key-repeat is safe).
    fn toggle_play(&mut self, id: u64) {
        if self.playing.as_ref().map(|p| p.clip_id == id).unwrap_or(false) {
            self.playing = None;
            return;
        }
        self.play_clip(id);
    }

    fn play_clip_if_idle(&mut self, id: u64) {
        if self.playing.as_ref().map(|p| p.clip_id == id).unwrap_or(false) {
            return;
        }
        self.play_clip(id);
    }

    fn play_clip(&mut self, id: u64) {
        let clip = match self.doc.data.clips.iter().find(|c| c.id == id) {
            Some(c) => c.clone(),
            None => return,
        };
        let pcm = match clip.pcm() {
            Ok(p) if !p.is_empty() => p,
            _ => {
                self.toast("clip data corrupt");
                return;
            }
        };
        let ss = match &self.audio_ss {
            Some(ss) => ss.clone(),
            None => {
                self.toast("no audio subsystem");
                return;
            }
        };
        match ss.open_queue::<i16, _>(None::<&str>, &Self::clip_spec()) {
            Ok(q) => {
                if q.queue_audio(&pcm).is_err() {
                    self.toast("playback failed");
                    return;
                }
                q.resume();
                self.playing = Some(PlayState { _q: q, clip_id: id, start: Instant::now(), seconds: clip.seconds.max(0.1) });
            }
            Err(e) => self.toast(&format!("no audio out ({})", e)),
        }
    }

    fn poll_audio(&mut self) {
        // finish playback
        if let Some(p) = &self.playing {
            if p.start.elapsed().as_secs_f32() >= p.seconds {
                self.playing = None;
            }
        }
        // auto-stop at the length cap
        if let Some(r) = &self.rec {
            if r.start.elapsed().as_secs() >= audio::CLIP_MAX_SECS as u64 {
                self.stop_record();
                self.toast("clip max length");
            }
        }
    }

    fn draw_clips(&mut self) -> Result<(), String> {
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let view = self.cam.view_rect(w, h);
        let items: Vec<AudioClip> = self
            .doc
            .data
            .clips
            .iter()
            .map(|c| {
                let (dx, dy) = self.move_delta_for(SelKind::Clip, c.id);
                let mut cc = c.clone();
                cc.x += dx as f64;
                cc.y += dy as f64;
                cc
            })
            .collect();
        for c in items {
            let r = Rect::new(
                c.x as f32 - CLIP_RADIUS as f32,
                c.y as f32 - CLIP_RADIUS as f32,
                c.x as f32 + CLIP_RADIUS as f32,
                c.y as f32 + CLIP_RADIUS as f32,
            );
            if !view.intersects(&r) {
                continue;
            }
            let (sx, sy) = cam.world_to_screen(c.x as f32, c.y as f32, w, h);
            let sr = (CLIP_RADIUS as f32 * cam.zoom).max(8.0) as i32;
            let (cx, cy) = (sx as i32, sy as i32);
            let selected = self.selection == Some((SelKind::Clip, c.id));
            let playing = self.playing.as_ref().map(|p| p.clip_id == c.id).unwrap_or(false);
            // badge
            let bg = self.bg_color();
            self.canvas.set_draw_color(Color::RGBA(bg.r, bg.g, bg.b, 220));
            fill_circle(&mut self.canvas, cx, cy, sr)?;
            self.canvas.set_draw_color(if selected { ACCENT } else { INK });
            draw_circle(&mut self.canvas, cx, cy, sr)?;
            draw_mic_glyph(&mut self.canvas, cx, cy, sr)?;
            // playing: progress ring + elapsed readout in the label
            if playing {
                if let Some(p) = &self.playing {
                    let frac = (p.start.elapsed().as_secs_f32() / p.seconds).min(1.0);
                    self.canvas.set_draw_color(ACCENT);
                    let mut prev: Option<(i32, i32)> = None;
                    let steps = (frac * 48.0) as i32;
                    for i in 0..=steps.max(1) {
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 48.0 * std::f32::consts::TAU;
                        let rr = sr + 4;
                        let pt = (
                            cx + (rr as f32 * a.cos()) as i32,
                            cy + (rr as f32 * a.sin()) as i32,
                        );
                        if let Some(q) = prev {
                            self.canvas.draw_line(q, pt)?;
                        }
                        prev = Some(pt);
                    }
                }
            }
            let label = if playing {
                if let Some(p) = &self.playing {
                    let e = p.start.elapsed().as_secs_f32().min(p.seconds);
                    format!("▶ {} / {}", fmt_dur(e), c.duration_label())
                } else {
                    c.duration_label()
                }
            } else {
                c.duration_label()
            };
            let save = self.label_fade;
            self.label_fade = 1.0;
            self.draw_label(&label, if selected { ACCENT } else { INK }, cx - sr, cy + sr + 4)?;
            self.label_fade = save;
        }
        Ok(())
    }

    /// Pulsing red dot + live timer in the top bar while recording.
    /// Drawn at full opacity even when the chrome is faded.
    fn draw_rec_indicator(&mut self) -> Result<(), String> {
        let rec = match &self.rec {
            Some(r) => r,
            None => return Ok(()),
        };
        let save = self.label_fade;
        self.label_fade = 1.0;
        let pulse = (self.start.elapsed().as_secs_f32() * 5.0).sin() > 0.0;
        let dot_col = if pulse { Color::RGB(0xe0, 0x20, 0x20) } else { Color::RGB(0x80, 0x20, 0x20) };
        self.canvas.set_draw_color(dot_col);
        fill_circle(&mut self.canvas, 100, 22, 8)?;
        let s = rec.start.elapsed().as_secs() as u32;
        self.draw_label(&format!("REC {}:{:02}", s / 60, s % 60), dot_col, 115, 12)?;
        self.label_fade = save;
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
        let col = self.pal(meta.color);
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
        self.label_fade = self.ui_fade;
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
        self.cc(CHROME2);
        self.canvas.fill_rect(SdlRect::new(px, py, pw as u32, 42))?;
        self.cc(ACCENT);
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
        self.cc(CHROME);
        self.canvas.fill_rect(sr)?;
        let t = ((speed - 0.25) / 2.75).clamp(0.0, 1.0);
        let fx = sr.x() + (t * sr.width() as f32) as i32;
        self.cc(ACCENT);
        self.canvas.fill_rect(SdlRect::new(fx - 3, sr.y(), 6, sr.height()))?;
        self.panel_speed = Some(sr);
        self.label_fade = 1.0;
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

    fn draw_text_shape(&mut self, t: &TextMeta, x: f32, y: f32) -> Result<(), String> {
        if t.shape == 0 || t.box_w <= 0.0 {
            return Ok(());
        }
        let cam = self.cam;
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let col = self.pal(t.color);
        self.canvas.set_draw_color(col);
        if t.shape == 1 {
            let (sx, sy) = cam.world_to_screen(x, y, w, h);
            let (ex, ey) = cam.world_to_screen(x + t.box_w, y + t.box_h, w, h);
            self.canvas.draw_rect(SdlRect::new(sx as i32, sy as i32, (ex - sx).max(1.0) as u32, (ey - sy).max(1.0) as u32))?;
        } else {
            // ellipse outline, 64 segments
            let (cx, cy) = (x + t.box_w / 2.0, y + t.box_h / 2.0);
            let (rx, ry) = (t.box_w / 2.0, t.box_h / 2.0);
            let mut prev: Option<(i32, i32)> = None;
            for i in 0..=64 {
                let a = i as f32 / 64.0 * std::f32::consts::TAU;
                let (sx, sy) = cam.world_to_screen(cx + rx * a.cos(), cy + ry * a.sin(), w, h);
                let p = (sx as i32, sy as i32);
                if let Some(q) = prev {
                    self.canvas.draw_line(q, p)?;
                }
                prev = Some(p);
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
        // shape outlines under the text
        for t in &self.doc.data.texts.clone() {
            let (dx, dy) = self.move_delta_for(SelKind::Text, t.id);
            self.draw_text_shape(t, t.x + dx, t.y + dy)?;
        }
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
        // smudge ignores the paint color (a finger, not a brush): preview it
        // as a neutral gray ghost so the stroke doesn't promise paint
        let col = if st.tool == Tool::Smudge {
            Color::RGB(200, 200, 200)
        } else {
            self.pal(st.color)
        };
        let tc = self.tc;
        let spacing = Self::brush_spacing(&self.spacings, st.tool);
        let tips = &mut self.tips;
        let canvas = &mut self.canvas;
        let tip = tips.get(tc, st.tool, dpx).map_err(|e| e)?;
        tip.set_color_mod(col.r, col.g, col.b);
        let scr: Vec<(f32, f32)> = st.points.iter().map(|&(x, y)| cam.world_to_screen(x, y, w, h)).collect();
        for pair in scr.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            walk_segment(x0, y0, x1, y1, st.size * cam.zoom, spacing, |x, y, dd| {
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
            SelKind::Clip => self.doc.data.clips.iter().find(|c| c.id == id).map(|c| {
                let (dx, dy) = self.move_delta_for(kind, id);
                let (x, y, r) = (c.x as f32, c.y as f32, CLIP_RADIUS as f32);
                Rect::new(x + dx - r, y + dy - r, x + dx + r, y + dy + r)
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
            // resize handles for bounded text
            if kind == SelKind::Text {
                for corner in 0..4u8 {
                    if let Some((hx, hy)) = self.text_handle_pos(id, corner) {
                        self.canvas.set_draw_color(ACCENT);
                        let _ = self.canvas.fill_rect(SdlRect::new(hx - 5, hy - 5, 10, 10));
                    }
                }
            }
        }
    }

    /// Screen-space corner handle positions for the selected bounded text.
    fn text_handle_pos(&self, id: u64, corner: u8) -> Option<(i32, i32)> {
        let t = self.doc.data.texts.iter().find(|t| t.id == id)?;
        if t.box_w <= 0.0 {
            return None;
        }
        let (wx, wy) = match corner {
            0 => (t.x, t.y),
            1 => (t.x + t.box_w, t.y),
            2 => (t.x, t.y + t.box_h),
            _ => (t.x + t.box_w, t.y + t.box_h),
        };
        let (sx, sy) = self.cam.world_to_screen(wx, wy, self.win_w as f32, self.win_h as f32);
        Some((sx as i32, sy as i32))
    }

    fn draw_tbox_rubber(&mut self) -> Result<(), String> {
        if self.drag != DragKind::TextBox {
            return Ok(());
        }
        if let Some((x0, y0, x1, y1)) = self.tbox {
            if (x1 - x0).abs() < 4.0 || (y1 - y0).abs() < 4.0 {
                return Ok(());
            }
            let cam = self.cam;
            let (w, h) = (self.win_w as f32, self.win_h as f32);
            let (sx0, sy0) = cam.world_to_screen(x0.min(x1), y0.min(y1), w, h);
            let (sx1, sy1) = cam.world_to_screen(x0.max(x1), y0.max(y1), w, h);
            self.canvas.set_draw_color(ACCENT);
            self.canvas.draw_rect(SdlRect::new(
                sx0 as i32, sy0 as i32,
                (sx1 - sx0).max(1.0) as u32, (sy1 - sy0).max(1.0) as u32,
            ))?;
        }
        Ok(())
    }

    fn draw_text_edit(&mut self) -> Result<(), String> {
        let (ex, ey, bw, bh, buf) = match &self.text_edit {
            Some(e) => (e.x, e.y, e.bw, e.bh, e.buf.clone()),
            None => return Ok(()),
        };
        let (w, h) = (self.win_w as f32, self.win_h as f32);
        let (sx, sy) = self.cam.world_to_screen(ex, ey, w, h);
        if bw > 0.0 && bh > 0.0 {
            let (ex2, ey2) = self.cam.world_to_screen(ex + bw, ey + bh, w, h);
            self.canvas.set_draw_color(ACCENT);
            let _ = self.canvas.draw_rect(SdlRect::new(
                sx as i32, sy as i32,
                (ex2 - sx).max(1.0) as u32, (ey2 - sy).max(1.0) as u32,
            ));
        }
        let label = if buf.is_empty() { "type here" } else { &buf };
        let dim = buf.is_empty();
        self.draw_label(label, if dim { DIM } else { self.pal(self.color_idx) }, sx as i32, sy as i32)?;
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
        let tex = self.label_cache.get_mut(&key).ok_or("label")?;
        tex.set_alpha_mod((255.0 * self.label_fade) as u8);
        let q = tex.query();
        let dst = SdlRect::new(x, y, q.width, q.height);
        self.canvas.copy(tex, None, dst).map_err(|e| e.to_string())?;
        Ok((q.width, q.height))
    }

    fn button(&mut self, r: SdlRect, label: &str, active: bool) -> Result<(), String> {
        let bg = if active { ACCENT } else { CHROME2 };
        let fg = if active { Color::RGB(0x20, 0x18, 0x08) } else { INK };
        self.cc(bg);
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

    fn draw_color_popup(&mut self) -> Result<(), String> {
        let target = match self.color_edit {
            Some(t) => t,
            None => return Ok(()),
        };
        let pr = self.color_popup_rect();
        self.canvas.set_draw_color(CHROME);
        self.canvas.fill_rect(pr)?;
        self.canvas.set_draw_color(INK);
        self.canvas.draw_rect(pr)?;
        let title = match target {
            ColorTarget::Swatch(i) => format!("swatch: {}", self.swatch_name(i)),
            ColorTarget::Bg => "canvas background".to_string(),
        };
        self.draw_label(&title, INK, pr.x() + 14, pr.y() + 10)?;
        // preview
        let rgb = self.edit_rgb();
        let pv = SdlRect::new(pr.x() + 14, pr.y() + 34, pr.width() - 28, 30);
        self.canvas.set_draw_color(Color::RGB(rgb[0], rgb[1], rgb[2]));
        self.canvas.fill_rect(pv)?;
        self.canvas.set_draw_color(INK);
        self.canvas.draw_rect(pv)?;
        // RGB sliders
        for (ch, name) in [(0u8, "R"), (1u8, "G"), (2u8, "B")] {
            let r = self.pal_slider_rect(ch);
            self.draw_label(name, DIM, pr.x() + 14, r.y())?;
            self.canvas.set_draw_color(CHROME2);
            self.canvas.fill_rect(r)?;
            let t = rgb[ch as usize] as f32 / 255.0;
            let fw = (t * r.width() as f32) as i32;
            let bar = match ch {
                0 => Color::RGB(200, 60, 60),
                1 => Color::RGB(60, 160, 80),
                _ => Color::RGB(60, 100, 200),
            };
            self.canvas.set_draw_color(bar);
            self.canvas.fill_rect(SdlRect::new(r.x(), r.y(), fw.max(2) as u32, r.height()))?;
            self.canvas.set_draw_color(INK);
            self.canvas.draw_rect(r)?;
            self.draw_label(&format!("{}", rgb[ch as usize]), DIM, r.x() + r.width() as i32 + 8, r.y())?;
        }
        self.draw_label("esc to close", DIM, pr.x() + 14, pr.y() + pr.height() as i32 - 24)?;
        Ok(())
    }

    fn text_panel_action(&mut self, act: TextPanelAction) {
        let id = match self.selection {
            Some((SelKind::Text, id)) => id,
            _ => return,
        };
        let mut tm = match self.doc.data.texts.iter().find(|t| t.id == id).cloned() {
            Some(t) => t,
            None => return,
        };
        match act {
            TextPanelAction::Shape(s2) => tm.shape = s2,
            TextPanelAction::Align(a) => tm.align = a,
            TextPanelAction::Size(d) => tm.size = (tm.size + d).clamp(8.0, 220.0),
        }
        if let Ok((tex, w, h)) = self.render_text(&tm) {
            tm.w = w;
            tm.h = h;
            if let Some(dst) = self.doc.data.texts.iter_mut().find(|t| t.id == id) {
                *dst = tm;
            }
            self.text_tex.insert(id, tex);
        }
    }

    fn draw_text_panel(&mut self) -> Result<(), String> {
        self.text_panel_hits.clear();
        let (align, shape, size) = match self.selection {
            Some((SelKind::Text, id)) => match self.doc.data.texts.iter().find(|t| t.id == id) {
                Some(t) => (t.align, t.shape, t.size),
                None => return Ok(()),
            },
            _ => return Ok(()),
        };
        let pr = match self.text_panel_rect() {
            Some(r) => r,
            None => return Ok(()),
        };
        self.label_fade = self.ui_fade;
        let (px, py) = (pr.x(), pr.y());
        let pw = pr.width() as i32;
        self.cc(CHROME2);
        self.canvas.fill_rect(SdlRect::new(px, py, pw as u32, 42))?;
        self.cc(ACCENT);
        self.canvas.draw_rect(SdlRect::new(px, py, pw as u32, 42))?;
        let mut x = px + 8;
        // shape: none / rect / oval
        for (si, label) in [(0u8, "No"), (1u8, "Rect"), (2u8, "Oval")] {
            let r = SdlRect::new(x, py + 7, 52, 28);
            self.button(r, label, shape == si)?;
            self.text_panel_hits.push((r, TextPanelAction::Shape(si)));
            x += 58;
        }
        x += 8;
        for (ai, label) in [(0u8, "Left"), (1u8, "Ctr"), (2u8, "Right")] {
            let r = SdlRect::new(x, py + 7, 52, 28);
            self.button(r, label, align == ai)?;
            self.text_panel_hits.push((r, TextPanelAction::Align(ai)));
            x += 58;
        }
        x += 8;
        for (d, label) in [(-6.0f32, "A-"), (6.0f32, "A+")] {
            let r = SdlRect::new(x, py + 7, 44, 28);
            self.button(r, label, false)?;
            self.text_panel_hits.push((r, TextPanelAction::Size(d)));
            x += 50;
        }
        self.draw_label(&format!("{:.0}", size), DIM, x + 4, py + 12)?;
        self.label_fade = 1.0;
        Ok(())
    }

    fn fill_tri(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), String> {
        // scanline-filled triangle
        let (mut xa, mut ya) = (x0, y0);
        let (mut xb, mut yb) = (x1, y1);
        let (mut xc, mut yc) = (x2, y2);
        if ya > yb { std::mem::swap(&mut xa, &mut xb); std::mem::swap(&mut ya, &mut yb); }
        if ya > yc { std::mem::swap(&mut xa, &mut xc); std::mem::swap(&mut ya, &mut yc); }
        if yb > yc { std::mem::swap(&mut xb, &mut xc); std::mem::swap(&mut yb, &mut yc); }
        let lerp_x = |x0: i32, y0: i32, x1: i32, y1: i32, y: i32| -> i32 {
            if y1 == y0 { return x0; }
            x0 + (x1 - x0) * (y - y0) / (y1 - y0)
        };
        for y in ya..=yc {
            let (x1, x2) = if y < yb {
                (lerp_x(xa, ya, xb, yb, y), lerp_x(xa, ya, xc, yc, y))
            } else {
                (lerp_x(xb, yb, xc, yc, y), lerp_x(xa, ya, xc, yc, y))
            };
            let (x1, x2) = (x1.min(x2), x1.max(x2));
            self.canvas.draw_line((x1, y), (x2, y))?;
        }
        Ok(())
    }
    fn fill_circ(&mut self, cx: i32, cy: i32, rad: i32) -> Result<(), String> {
        for y in -rad..=rad {
            let hw = ((rad * rad - y * y) as f32).sqrt() as i32;
            self.canvas.draw_line((cx - hw, cy + y), (cx + hw, cy + y))?;
        }
        Ok(())
    }
    fn thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, w: i32) -> Result<(), String> {
        for o in -w / 2..=w / 2 {
            self.canvas.draw_line((x0 + o, y0), (x1 + o, y1))?;
            self.canvas.draw_line((x0, y0 + o), (x1, y1 + o))?;
        }
        Ok(())
    }

    fn draw_tool_icon(&mut self, tool: UiTool, r: SdlRect, fg: Color, bg: Color) -> Result<(), String> {
        let cx = r.x() + r.width() as i32 / 2;
        let cy = r.y() + r.height() as i32 / 2;
        self.cc(fg);
        match tool {
            UiTool::Select => {
                // cursor arrow
                self.fill_tri(cx - 7, cy - 11, cx - 7, cy + 9, cx + 9, cy + 1)?;
                self.cc(bg);
                self.fill_tri(cx - 4, cy - 8, cx - 4, cy + 6, cx + 5, cy + 1)?;
            }
            UiTool::Pan => {
                // four-way move arrows
                for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
                    let (x1, y1) = (cx + dx * 11, cy + dy * 11);
                    self.canvas.draw_line((cx, cy), (x1, y1))?;
                    let (px, py) = (-dy, dx);
                    self.canvas.draw_line((x1, y1), (x1 - dx * 4 + px * 3, y1 - dy * 4 + py * 3))?;
                    self.canvas.draw_line((x1, y1), (x1 - dx * 4 - px * 3, y1 - dy * 4 - py * 3))?;
                }
                self.fill_circ(cx, cy, 2)?;
            }
            UiTool::Pencil => {
                self.thick_line(cx - 8, cy + 8, cx + 5, cy - 5, 5)?;
                self.fill_tri(cx + 5, cy - 5, cx + 11, cy - 11, cx + 2, cy - 8)?;
                self.canvas.fill_rect(SdlRect::new(cx - 11, cy + 5, 7, 7))?;
            }
            UiTool::Pen => {
                // fountain nib
                self.fill_tri(cx, cy - 11, cx + 7, cy + 1, cx - 7, cy + 1)?;
                self.fill_tri(cx, cy + 11, cx + 7, cy + 1, cx - 7, cy + 1)?;
                self.cc(bg);
                self.canvas.draw_line((cx, cy - 9), (cx, cy - 1))?;
                self.fill_circ(cx, cy + 4, 2)?;
            }
            UiTool::Marker => {
                // chisel tip parallelogram
                self.fill_tri(cx - 9, cy + 8, cx + 1, cy - 10, cx + 9, cy - 10)?;
                self.fill_tri(cx - 9, cy + 8, cx - 1, cy + 8, cx + 9, cy - 10)?;
                self.cc(bg);
                self.canvas.draw_line((cx - 9, cy + 8), (cx - 1, cy + 8))?;
            }
            UiTool::OilStick => {
                self.fill_circ(cx - 2, cy - 2, 9)?;
                self.fill_circ(cx + 8, cy + 7, 4)?;
                self.cc(bg);
                self.fill_circ(cx - 5, cy - 5, 3)?;
            }
            UiTool::Charcoal => {
                // charcoal stick: chunky diagonal chunk, crumbled tip, dust
                self.thick_line(cx - 9, cy + 9, cx + 5, cy - 5, 7)?;
                self.thick_line(cx + 5, cy - 5, cx + 10, cy - 10, 4)?;
                self.fill_circ(cx - 12, cy + 11, 1)?;
                self.fill_circ(cx + 11, cy + 10, 1)?;
                self.fill_circ(cx - 1, cy - 12, 1)?;
            }
            UiTool::Smudge => {
                // finger smear: soft blob with trailing streaks
                self.fill_circ(cx + 4, cy - 2, 7)?;
                self.fill_circ(cx - 4, cy + 2, 5)?;
                self.fill_circ(cx - 10, cy + 5, 3)?;
                self.canvas.draw_line((cx - 11, cy - 8), (cx - 2, cy - 8))?;
                self.canvas.draw_line((cx - 12, cy - 4), (cx - 5, cy - 4))?;
                self.canvas.draw_line((cx - 11, cy + 11), (cx - 4, cy + 11))?;
            }
            UiTool::Text => {
                self.draw_label("A", fg, cx - 8, cy - 11)?;
            }
        }
        Ok(())
    }

    fn draw_top_icon(&mut self, kind: u8, r: SdlRect, fg: Color) -> Result<(), String> {
        // kind: 0 save, 1 vectorize, 2 export png
        let cx = r.x() + r.width() as i32 / 2;
        let cy = r.y() + r.height() as i32 / 2;
        self.cc(fg);
        match kind {
            0 => {
                // floppy disk
                self.canvas.draw_rect(SdlRect::new(cx - 9, cy - 7, 18, 14))?;
                self.canvas.fill_rect(SdlRect::new(cx - 5, cy - 7, 10, 6))?;
                self.canvas.draw_line((cx - 5, cy + 4), (cx + 5, cy + 4))?;
            }
            1 => {
                // hexagon (vector nodes)
                let pts = [(9, 0), (4, 8), (-4, 8), (-9, 0), (-4, -8), (4, -8)];
                for i in 0..6 {
                    let (x0, y0) = pts[i];
                    let (x1, y1) = pts[(i + 1) % 6];
                    self.canvas.draw_line((cx + x0, cy + y0), (cx + x1, cy + y1))?;
                }
                self.fill_circ(cx, cy, 2)?;
            }
            _ => {
                // picture: mountains + sun
                self.canvas.draw_rect(SdlRect::new(cx - 10, cy - 7, 20, 14))?;
                self.fill_tri(cx - 10, cy + 7, cx - 3, cy - 3, cx + 2, cy + 7)?;
                self.fill_tri(cx - 2, cy + 7, cx + 5, cy - 1, cx + 10, cy + 7)?;
                self.fill_circ(cx + 5, cy - 3, 2)?;
            }
        }
        Ok(())
    }

    fn draw_chrome(&mut self) -> Result<(), String> {
        self.label_fade = self.ui_fade;
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        // top bar
        self.cc(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, 0, w as u32, TOP_H as u32))?;
        self.draw_label("MURAL", ACCENT, 12, 12)?;
        let zoom_txt = format!("{}%", (self.cam.zoom * 100.0).round() as i32);
        self.draw_label(&zoom_txt, DIM, w - 440, 12)?;
        // background color swatch (right-click edits)
        let br = self.bg_rect();
        self.cc(self.bg_color());
        self.canvas.fill_rect(br)?;
        self.cc(INK);
        self.canvas.draw_rect(br)?;
        self.draw_label("BG", DIM, br.x() + 20, br.y() + 6)?;
        // top-right buttons (icons)
        for (i, bx) in [w - 292, w - 220, w - 148].iter().enumerate() {
            let r = SdlRect::new(*bx, 8, 64, 28);
            self.cc(CHROME2);
            self.canvas.fill_rect(r)?;
            self.draw_top_icon(i as u8, r, INK)?;
        }
        self.button(SdlRect::new(w - 76, 8, 64, 28), "?", self.show_help)?;
        // left toolbar
        self.cc(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, TOP_H, TOOL_W as u32, (h - TOP_H - BOT_H) as u32))?;
        for (i, t) in TOOLS.iter().enumerate() {
            let r = self.tool_rect(i);
            let active = *t == self.tool;
            let bgc = if active { ACCENT } else { CHROME2 };
            let fgc = if active { Color::RGB(0x20, 0x18, 0x08) } else { INK };
            self.cc(bgc);
            self.canvas.fill_rect(r)?;
            self.draw_tool_icon(*t, r, fgc, bgc)?;
            self.draw_label(t.shortcut(), DIM, r.x() + r.width() as i32 - 15, r.y() + r.height() as i32 - 15)?;
            self.draw_label(t.label(), DIM, r.x() - 2, r.y() + r.height() as i32 + 1)?;
        }
        // bottom bar
        self.cc(CHROME);
        self.canvas.fill_rect(SdlRect::new(0, h - BOT_H, w as u32, BOT_H as u32))?;
        for i in 0..PALETTE_COUNT {
            let r = self.swatch_rect(i);
            let c = self.pal(i);
            self.cc(c);
            self.canvas.fill_rect(r)?;
            if i == self.color_idx {
                self.cc(INK);
                self.canvas.draw_rect(r)?;
                let r2 = SdlRect::new(r.x() - 2, r.y() - 2, r.width() + 4, r.height() + 4);
                self.canvas.draw_rect(r2)?;
            }
        }
        // color-set chip: click = next set, right-click / shift+C = previous
        let cr = self.set_chip_rect();
        self.cc(CHROME2);
        self.canvas.fill_rect(cr)?;
        self.cc(DIM);
        self.canvas.draw_rect(cr)?;
        self.draw_label(self.set_name(), INK, cr.x() + 10, cr.y() + 12)?;
        let cname = self.swatch_name(self.color_idx);
        self.draw_label(&format!("{} · {}", self.set_name(), cname), DIM, TOOL_W + 12, h - 20)?;
        // brush slider
        let sr = self.slider_rect();
        self.cc(CHROME2);
        self.canvas.fill_rect(sr)?;
        let t = (self.brush_size() - 1.0) / 95.0;
        let fx = sr.x() + (t * sr.width() as f32) as i32;
        self.cc(ACCENT);
        self.canvas.fill_rect(SdlRect::new(fx - 3, sr.y(), 6, sr.height()))?;
        self.draw_label(&format!("size {:.0}", self.brush_size()), DIM, sr.x(), sr.y() - 20)?;
        // undo/redo state
        let ur = format!("{}/{}", if self.doc.can_undo() { "undo" } else { "-" }, if self.doc.can_redo() { "redo" } else { "-" });
        self.draw_label(&ur, DIM, w - 300, h - 20)?;
        self.label_fade = 1.0;
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
        // color-set chip: click = next set
        if self.set_chip_rect().contains_point((sx, sy)) {
            self.cycle_color_set(1);
            return;
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
                if let Some((SelKind::Text, id)) = self.selection {
                    if let Some(tm) = self.doc.data.texts.iter().find(|t| t.id == id).cloned() {
                        let mut tm2 = tm;
                        tm2.color = i;
                        if let Ok((tex, _, _)) = self.render_text(&tm2) {
                            if let Some(dst) = self.doc.data.texts.iter_mut().find(|t| t.id == id) {
                                *dst = tm2;
                            }
                            self.text_tex.insert(id, tex);
                        }
                    }
                }
                return;
            }
        }
        // color editor popup: sliders start drags; clicking outside closes
        if self.color_edit.is_some() {
            for ch in 0..3u8 {
                let r = self.pal_slider_rect(ch);
                if r.contains_point((sx, sy)) {
                    self.drag = DragKind::PalSlider(ch);
                    let t = ((sx - r.x()) as f32 / r.width() as f32).clamp(0.0, 1.0);
                    self.set_edit_channel(ch, (t * 255.0).round() as u8);
                    return;
                }
            }
            if !self.color_popup_rect().contains_point((sx, sy)) {
                self.color_edit = None;
            }
            return;
        }
        // vector panel hits
        for (r, act) in self.panel_hits.clone() {
            if r.contains_point((sx, sy)) {
                self.panel_action(act);
                return;
            }
        }
        // text panel hits
        for (r, act) in self.text_panel_hits.clone() {
            if r.contains_point((sx, sy)) {
                self.text_panel_action(act);
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
        if self.bg_rect().contains_point((sx, sy)) {
            self.color_edit = Some(ColorTarget::Bg);
            return;
        }
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
        let bh = 740;
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
            "P pencil  N pen  M marker  O oil stick  K charcoal  S smudge",
            "T text: click = point text, drag = text box",
            "text box panel: shape (no/rect/oval), align, size",
            "drag box corners to resize; double-click to re-edit",
            "[ ] brush size   wheel zoom",
            "I import folder   drag & drop works too",
            "A record voice clip (12kHz μ-law 96kbps); A again = stop, esc = cancel",
            "V + click a clip to select; double-click or space = play; del removes",
            "right-click a color swatch to edit it; BG edits canvas",
            "C cycles color set (Studio/Dutch/Pop/Picasso/Basquiat); shift+C = back",
            "del remove selected   esc deselect",
            "ctrl+Z / ctrl+Y undo / redo",
            "ctrl+S save   ctrl+O open (file dialog)",
            "ctrl+E export PNG   ctrl+0 reset view",
            "ui fades while typing or drawing",
            "",
            "paint sits above images, text above paint",
            "",
            ": command line — :w :q :e :tool :set :color :export :undo :redo :help",
            ":set size|spacing|color[=v] — per-tool props; += / -= work",
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

    /// Vim-style `:` command line + status/message bar, just above the
    /// bottom chrome (same bar shows results and errors after execution).
    fn draw_cmdline(&mut self) -> Result<(), String> {
        let (w, h) = (self.win_w as i32, self.win_h as i32);
        let y = h - BOT_H - 40;
        if let Some(c) = &self.cmdline {
            let buf = c.buf.clone();
            self.canvas.set_draw_color(Color::RGBA(10, 10, 14, 235));
            self.canvas.fill_rect(SdlRect::new(0, y, w as u32, 36))?;
            self.canvas.set_draw_color(ACCENT);
            self.canvas.draw_rect(SdlRect::new(0, y, w as u32, 36))?;
            let prompt = format!(":{buf}");
            self.draw_label(&prompt, INK, 12, y + 9)?;
            // block cursor
            let (tw, _) = self.font_ui.size_of(&prompt).unwrap_or((0, 0));
            self.canvas.set_draw_color(ACCENT);
            self.canvas
                .fill_rect(SdlRect::new(12 + tw as i32 + 2, y + 7, 11, 22))?;
        } else if let Some((msg, is_error, _)) = &self.cmd_msg {
            let msg = msg.clone();
            let col = if *is_error { ERR } else { INK };
            self.canvas.set_draw_color(Color::RGBA(10, 10, 14, 235));
            self.canvas.fill_rect(SdlRect::new(0, y, w as u32, 36))?;
            self.draw_label(&msg, col, 12, y + 9)?;
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
    /// Word-wrap text into a box. Returns (line text, y_top, line_width, avail_width).
    fn layout_text(&self, text: &str, size: f32, box_w: f32, box_h: f32, shape: u8) -> Vec<(String, f32, f32, f32)> {
        let k = size / 64.0;
        let (_, lh_px) = self.font_big.size_of("Ag").unwrap_or((0, 80));
        let line_h = lh_px as f32 * k;
        let pad = size * 0.4;
        let measure = |t: &str| -> f32 {
            self.font_big.size_of(t).map(|(w, _)| w as f32 * k).unwrap_or(0.0)
        };
        // width available for a line whose vertical center is at y
        let width_at = |y: f32| -> f32 {
            if shape == 2 {
                let cy = box_h / 2.0;
                let rx = (box_w / 2.0 - pad).max(1.0);
                let ry = (box_h / 2.0 - pad).max(1.0);
                let d = (y - cy) / ry;
                if d.abs() >= 1.0 {
                    return 0.0;
                }
                (2.0 * rx * (1.0 - d * d).sqrt()).max(0.0)
            } else {
                (box_w - 2.0 * pad).max(0.0)
            }
        };
        let mut lines: Vec<(String, f32, f32, f32)> = vec![];
        let mut y = pad;
        let fits_vertically = |y: f32| -> bool { y <= box_h - pad };
        for para in text.split('\n') {
            let words: Vec<&str> = para.split_whitespace().collect();
            // empty paragraph -> blank line
            if words.is_empty() {
                y += line_h;
                continue;
            }
            let mut cur = String::new();
            let mut cur_w = 0.0f32;
            let space_w = measure(" ");
            for word in words {
                if !fits_vertically(y) {
                    cur.clear();
                    break;
                }
                let ww = measure(word);
                let avail = width_at(y + line_h / 2.0);
                let need = if cur.is_empty() { ww } else { cur_w + space_w + ww };
                if need <= avail || cur.is_empty() {
                    if !cur.is_empty() {
                        cur.push(' ');
                        cur_w += space_w;
                    }
                    cur.push_str(word);
                    cur_w += ww;
                } else {
                    let lw = measure(&cur).min(avail);
                    lines.push((std::mem::take(&mut cur), y, lw, avail));
                    y += line_h;
                    cur.push_str(word);
                    cur_w = ww;
                }
            }
            if !cur.is_empty() && fits_vertically(y) {
                let avail = width_at(y + line_h / 2.0);
                let lw = measure(&cur).min(avail);
                lines.push((cur, y, lw, avail));
            }
            y += line_h;
        }
        lines
    }

    /// Render a text box to a texture. Returns (texture, world w, world h).
    fn render_text_box(&mut self, tm: &TextMeta) -> Result<(Texture<'tc>, f32, f32), String> {
        let col = self.pal(tm.color);
        let s = 64.0 / tm.size.max(4.0);
        let tw = (tm.box_w * s).ceil().max(2.0) as u32;
        let th = (tm.box_h * s).ceil().max(2.0) as u32;
        let mut surf = sdl2::surface::Surface::new(tw, th, PixelFormatEnum::RGBA32)
            .map_err(|e| e.to_string())?;
        for (line, y_top, lw, wline) in self.layout_text(&tm.text, tm.size, tm.box_w, tm.box_h, tm.shape) {
            if line.is_empty() {
                continue;
            }
            let ls = self.font_big.render(&line).blended(col).map_err(|e| e.to_string())?;
            let (lw_px, lh_px) = (ls.width(), ls.height());
            let pad = tm.size * 0.4;
            let x = match tm.align {
                1 => pad + (wline - lw).max(0.0) / 2.0,
                2 => pad + (wline - lw).max(0.0),
                _ => pad,
            };
            ls.blit(None, &mut surf, Some(SdlRect::new((x * s) as i32, (y_top * s) as i32, lw_px, lh_px)))
                .map_err(|e| e.to_string())?;
        }
        let tex = self.tc.create_texture_from_surface(&surf).map_err(|e| e.to_string())?;
        Ok((tex, tm.box_w, tm.box_h))
    }

    /// Render any text (point or box) to a texture.
    fn render_text(&mut self, tm: &TextMeta) -> Result<(Texture<'tc>, f32, f32), String> {
        if tm.box_w > 0.0 && tm.box_h > 0.0 {
            self.render_text_box(tm)
        } else {
            self.make_text_texture(&tm.text, tm.color, tm.size)
        }
    }

    fn make_text_texture(
        &mut self,
        text: &str,
        color_idx: usize,
        world_size: f32,
    ) -> Result<(Texture<'tc>, f32, f32), String> {
        let col = self.pal(color_idx);
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

    /// Save to `path`, returning the status message (shared by Ctrl+S and `:w`).
    fn save_to(&mut self, path: &Path) -> Result<String, String> {
        // persist the live tool-property overrides into the file
        self.doc.data.tool_sizes = self.sizes;
        self.doc.data.tool_spacings = self.spacings;
        self.doc.data.tool_color = self.color_idx;
        let j = self
            .doc
            .to_json()
            .map_err(|e| format!("save failed: {e}"))?;
        std::fs::write(path, j).map_err(|e| format!("save failed: {e}"))?;
        self.current_path = Some(path.to_path_buf());
        Ok(format!("saved {}", path.display()))
    }

    fn save_doc(&mut self) {
        let path = self
            .current_path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("mural.json"));
        match self.save_to(&path) {
            Ok(m) => {
                self.saved_seq = self.doc.seq();
                self.toast(&m);
            }
            Err(e) => self.toast(&e),
        }
    }

    fn load_doc(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("mural", &["json"])
            .set_title("Open mural")
            .pick_file();
        let path = match path {
            Some(p) => p,
            None => return, // cancelled (or no dialog backend available)
        };
        match self.load_path(&path) {
            Ok(m) => self.toast(&m),
            Err(e) => self.toast(&e),
        }
    }

    /// Open `path`, returning the status message (shared by Ctrl+O and `:e`).
    fn load_path(&mut self, path: &std::path::Path) -> Result<String, String> {
        let s =
            std::fs::read_to_string(path).map_err(|e| format!("open failed: {e}"))?;
        let data = Doc::from_json(&s).map_err(|e| format!("open failed: {e}"))?;
        let n_img = data.images.len();
        self.doc.load_data(data);
        // restore the saved tool-property overrides (clamped, like :set)
        {
            let d = &self.doc.data;
            self.sizes = d.tool_sizes.map(clamp_brush_size);
            self.spacings = d.tool_spacings.map(clamp_spacing);
            self.color_idx = d.tool_color.min(PALETTE_COUNT - 1);
        }
        self.tiles.clear();
        self.img_tex.clear();
        self.text_tex.clear();
        self.sync_media();
        let missing = n_img - self.img_tex.len();
        self.selection = None;
        self.current_path = Some(path.to_path_buf());
        self.saved_seq = self.doc.seq();
        Ok(format!(
            "loaded {}{}",
            path.display(),
            if missing > 0 {
                format!(" ({} images missing)", missing)
            } else {
                String::new()
            }
        ))
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
                    let b = doc.data.bg;
                    c.set_draw_color(Color::RGB(b[0], b[1], b[2]));
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
                    // voice-clip markers (circle + mic glyph; export is 1:1, no zoom)
                    for cl in &doc.data.clips {
                        let cx = (cl.x - ox as f64) as i32;
                        let cy = (cl.y - oy as f64) as i32;
                        let r = CLIP_RADIUS as i32;
                        if cx + r < 0 || cy + r < 0 || cx - r > w as i32 || cy - r > h as i32 {
                            continue;
                        }
                        let b = doc.data.bg;
                        c.set_draw_color(Color::RGBA(b[0], b[1], b[2], 220));
                        let _ = fill_circle(&mut *c, cx, cy, r);
                        c.set_draw_color(Color::RGB(0x1c, 0x1a, 0x17));
                        let _ = draw_circle(&mut *c, cx, cy, r);
                        let _ = draw_mic_glyph(&mut *c, cx, cy, r);
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
        // Audio is optional: recording/playback degrade to a toast when missing.
        self.audio_ss = sdl.audio().ok();
        'main: loop {
            for ev in pump.poll_iter() {
                match ev {
                    Event::Quit { .. } => break 'main,
                    Event::Window { win_event: sdl2::event::WindowEvent::Resized(ww, wh), .. } => {
                        self.win_w = ww as u32;
                        self.win_h = wh as u32;
                    }
                    Event::MouseButtonDown { x, y, mouse_btn, .. } => self.on_down(x, y, mouse_btn, video),
                    Event::MouseMotion { x, y, .. } => {
                        self.mouse_sx = x;
                        self.mouse_sy = y;
                        self.on_motion(x, y);
                    }
                    Event::MouseButtonUp { mouse_btn, .. } => self.on_up(mouse_btn, video),
                    Event::MouseWheel { y, mouse_x, mouse_y, .. } => self.on_wheel(y, mouse_x, mouse_y),
                    Event::KeyDown { keycode, keymod, repeat, .. } => {
                        if keycode == Some(Keycode::Space) {
                            self.space_down = true;
                            // Space plays the selected clip (repeat-safe: never stops)
                            if !repeat {
                                if let Some((SelKind::Clip, id)) = self.selection {
                                    self.play_clip_if_idle(id);
                                }
                            }
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
            if let Some((_, is_error, t)) = &self.cmd_msg {
                let ttl = if *is_error { 6 } else { 3 };
                if t.elapsed().as_secs() > ttl {
                    self.cmd_msg = None;
                }
            }
            self.frame()?;
        }
        Ok(())
    }

    /// Headless self-test: synthetic strokes/text/image, then export.
    fn smoke(&mut self) -> Result<(), String> {
        for (i, tool) in [Tool::Pencil, Tool::Pen, Tool::Marker, Tool::OilStick, Tool::Charcoal, Tool::Smudge].iter().enumerate() {
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
            box_w: 0.0,
            box_h: 0.0,
            align: 0,
            shape: 0,
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
        // voice clip: doc-level add/undo/redo (no mic needed; codec is unit-tested).
        // 1 s of synthetic μ-law at 12 kHz.
        let ulaw: Vec<u8> = (0..12000).map(|i| audio::mulaw_encode(((i % 200) as i16 - 100) * 40)).collect();
        let clip_id = self.doc.alloc_id();
        self.doc.add_clip(AudioClip {
            id: clip_id,
            x: 100.0,
            y: -50.0,
            seconds: 1.0,
            data: audio::b64encode(&ulaw),
        });
        let nc0 = self.doc.data.clips.len();
        self.doc.undo();
        assert_eq!(self.doc.data.clips.len(), nc0 - 1, "clip undo");
        self.doc.redo();
        assert_eq!(self.doc.data.clips.len(), nc0, "clip redo");
        let c = self.doc.data.clips.iter().find(|c| c.id == clip_id).unwrap();
        assert_eq!(c.pcm().unwrap().len(), 12000, "clip pcm decodes");
        assert_eq!(c.duration_label(), "0:01");
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
        assert_eq!(self.doc.data.clips.len(), nc0, "clips survive roundtrip");
        assert_eq!(self.doc.data.clips[0].pcm().unwrap().len(), 12000, "clip audio survives roundtrip");
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
        // real file-based load_doc with hostile data: old-format JSON (no
        // vectors key), a text, and an image whose file is missing
        let hostile = r#"{"strokes":[{"id":1,"tool":"Pen","color":0,"size":4.0,"points":[[0.0,0.0],[100.0,50.0]]}],"images":[{"id":2,"path":"/tmp/does_not_exist.png","x":10.0,"y":10.0,"w":200.0,"h":100.0}],"texts":[{"id":3,"text":"hello","x":5.0,"y":5.0,"w":60.0,"h":20.0,"color":1,"size":24.0}]}"#;
        std::fs::write("mural.json", hostile).expect("write mural.json");
        self.load_path(std::path::Path::new("mural.json")).expect("hostile load");
        assert_eq!(self.doc.data.strokes.len(), 1, "strokes load");
        assert_eq!(self.doc.data.texts.len(), 1, "texts load");
        assert!(self.img_tex.is_empty(), "missing image must not load a texture");
        self.frame()?;
        std::fs::remove_file("mural.json").ok();
        // bounded text: layout wraps, panel actions apply, resize works
        let lines = self.layout_text("the quick brown fox jumps over the lazy dog", 32.0, 200.0, 200.0, 0);
        assert!(lines.len() > 1, "long text must wrap, got {} lines", lines.len());
        let elines = self.layout_text("the quick brown fox jumps over the lazy dog", 32.0, 200.0, 200.0, 2);
        assert!(!elines.is_empty(), "ellipse layout must produce lines");
        let btm = crate::canvas::TextMeta {
            id: self.doc.alloc_id(), text: "the quick brown fox jumps over the lazy dog".into(),
            x: 0.0, y: 0.0, size: 32.0, color: 4, w: 200.0, h: 120.0,
            box_w: 200.0, box_h: 120.0, align: 0, shape: 1,
        };
        let (btex, bw, bh) = self.render_text(&btm).expect("render box text");
        assert_eq!((bw, bh), (200.0, 120.0));
        let bq = btex.query();
        assert!(bq.width > 10 && bq.height > 10, "box texture has size");
        self.doc.add_text(btm.clone());
        self.text_tex.insert(btm.id, btex);
        self.selection = Some((SelKind::Text, btm.id));
        self.frame()?;
        assert!(self.text_panel_rect().is_some(), "text panel shows for text selection");
        assert!(!self.text_panel_hits.is_empty(), "text panel has buttons");
        self.text_panel_action(TextPanelAction::Align(1));
        assert_eq!(self.doc.data.texts.iter().find(|t| t.id == btm.id).unwrap().align, 1);
        self.text_panel_action(TextPanelAction::Shape(2));
        assert_eq!(self.doc.data.texts.iter().find(|t| t.id == btm.id).unwrap().shape, 2);
        self.text_panel_action(TextPanelAction::Size(8.0));
        assert!((self.doc.data.texts.iter().find(|t| t.id == btm.id).unwrap().size - 40.0).abs() < 0.01);
        // clicking a text panel button must not deselect
        let pr = self.text_panel_rect().unwrap();
        assert!(self.ui_hit(pr.x() + 10, pr.y() + 10), "text panel counts as chrome");
        // color editor channel set
        self.color_edit = Some(ColorTarget::Swatch(0));
        self.set_edit_channel(0, 11);
        self.set_edit_channel(1, 22);
        self.set_edit_channel(2, 33);
        assert_eq!(self.doc.data.palette[0], [11, 22, 33]);
        self.color_edit = Some(ColorTarget::Bg);
        self.set_edit_channel(0, 200);
        assert_eq!(self.doc.data.bg[0], 200);
        self.color_edit = None;
        self.frame()?;
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
