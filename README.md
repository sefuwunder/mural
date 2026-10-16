# mural

Infinite canvas drawing app for desktop, in Rust + SDL2.

Draw with four procedural brushes, pull in folders of images as movable
reference, and drop text labels anywhere. Pan forever, zoom 0.1x–8x.

## Brushes

- **Pencil** (P) — thin, graphite grain
- **Pen** (N) — clean solid line
- **Marker** (M) — thick, translucent alcohol-marker feel; overlaps darken
- **Oil stick** (O) — chunky impasto with speckled texture

16 curated artist colors, per-brush size memory, `[` `]` or the slider to resize.

## Canvas

- Infinite pan (space-drag, middle-drag, or H) and zoom-to-cursor (wheel)
- Paint is rasterized into 512px world-space tiles, so the canvas never fills up
- Warm paper background with an adaptive dot grid

## Images

- `I` → type a folder path → every image lands on the canvas in a row
- Or drag & drop files/folders straight from your file manager
- `V` tool: drag images freely, `del` removes

## Vectors
Select an image (V) and press **X** — or hit the **Vec** button — to trace it
into resolution-independent vector polygons (via vtracer, B&W polygon mode;
works best on high-contrast images). The vector lands exactly over the source
image and starts playing a draw-on animation.

With a vector selected, a panel appears above the bottom bar:

- **Still / Draw / Wave / Spin / Pulse / March** — animation mode
  (March = marching-ants dashed outline)
- **play/pause**, **speed** slider (0.25×–3×), **Fill** toggle
- clicking a palette swatch recolors the vector

Vectors sit between the paint and text layers, can be moved with V,
deleted, undone/redone, saved in `mural.json`, and exported to PNG.

## Text

- `T` → click anywhere → type → Enter commits
- Double-click a label with `V` to re-edit it

## Shortcuts

| key | action |
|---|---|
| V / H | move · pan |
| P N M O | pencil · pen · marker · oil stick |
| T | text |
| `[` `]` | brush size |
| I | import folder |
| X | vectorize selected image |
| del | delete selected |
| esc | deselect / cancel |
| ctrl+Z / ctrl+Y | undo / redo |
| ctrl+S / ctrl+O | save / open `mural.json` |
| ctrl+E | export PNG (content bounds) |
| ctrl+0 | reset view |
| F1 | this help |

Layer order is fixed: images below paint below text.

## Build

```bash
cargo run --release
```

Needs SDL2 + SDL2_ttf dev libraries and a TTF font
(probes DejaVu → Liberation → Noto, or set `MURAL_FONT=/path/to.ttf`).

Headless self-test (exercises brushes, tiles, text, image import,
undo/redo, PNG export, and the full UI frame path):

```bash
cargo run -- --smoke   # writes /tmp/mural_smoke.png
```
