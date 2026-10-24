# mural

Infinite canvas drawing app for desktop, in Rust + SDL2.

Draw with six procedural brushes, pull in folders of images as movable
reference, and drop text labels anywhere. Pan forever, zoom 0.1x–8x.

## Brushes

- **Pencil** — thin, graphite grain
- **Pen** (N) — clean solid line
- **Marker** (M) — thick, translucent alcohol-marker feel; overlaps darken
- **Oil stick** (O) — chunky impasto with speckled texture
- **Charcoal** (K) — vine-charcoal dust, paper-tooth grain
- **Smudge** (S) — soft finger-smudge blending

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

- `T` → click anywhere → type → Enter commits (point text)
- `T` → **drag** a box → type → text flows inside it (word wrap)
- Select a text box for the text panel: shape (none / rect / oval),
  alignment (left / center / right), font size
- Drag the box corners to resize; double-click a label with `V` to re-edit it
- Clicking a color swatch recolors the selected text

## Canvas & palette

- The **BG** swatch (top bar) shows the canvas color — click it, or
  right-click any color swatch, to open the color editor (RGB sliders)
- Custom colors and the background travel with the saved file
- The toolbar fades while you type or draw so it never blocks the canvas

## Shortcuts

| key | action |
|---|---|
| V / H | move · pan |
| P | pencil |
| space | play selected vector/clip (pause/resume in perform mode) |
| N M O K S | pen · marker · oil stick · charcoal · smudge |
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
| MIX / `:mixer` | 4-track mixer for voice clips |
| 1–4 | route selected clip to mixer track 1–4 |
| F1 | this help |

## Playback

- **Selecting a paint stroke** replays its drawing history with true timing
  — your real rhythm, pauses included. Strokes drawn before timing was
  recorded (older files) replay at a steady pace instead. **Space** on a
  selected stroke restarts the replay. The replay is view-only: Esc,
  deselect, starting a new stroke, or perform mode cancels it.
- **Space** — play the selected object: toggles a vector's animation
  (same as the panel play/pause button) or plays a voice clip's audio
  (same as double-click)
- **Shift+P** (or the ▶ button in the top bar) — **perform mode**: every
  animated vector starts playing and every voice clip starts sounding,
  all at once. A slim transport overlay appears with pause/resume and exit.
- **Space** in perform mode — pause / resume (audio restarts from the top)
- **Esc** — exit perform mode: stops all audio and restores each vector's
  pre-perform play/pause state
- While performing, canvas editing is suspended — only transport keys
  (and the overlay buttons) respond

## Mixer

Four-track mixer for voice clips, in a Teenage Engineering / Braun
hardware idiom: one warm off-white unit, four color-coded strips plus a
master, vertical faders, M (mute) and S (solo) buttons, live meters.

- **MIX** button in the top bar (or `:mixer`) — show / hide the mixer.
  Entering perform mode with clips on the canvas brings it up automatically.
- Every clip is routed to one of **4 tracks**: `1`–`4` with a clip selected
  routes it; new recordings land on the emptiest track. The clip marker
  carries a small badge in its track's color.
- Faders move live — no clip restart — because playback runs through one
  mixer device, not one queue per clip. Mute silences a track, solo
  silences everything else, master scales the whole mix. Muted tracks keep
  playing in sync, so unmuting resumes mid-clip.
- Fader, mute, solo, master, and clip routing all persist in `mural.json`.

## Command line

Press `:` for a vim-style command line (Enter runs, Esc cancels, ↑/↓ history):

| command | action |
|---|---|
| `:w [path]` | save `mural.json` (current file or given path) |
| `:q` / `:q!` | quit — refuses when dirty unless `:q!` |
| `:e <path>` | open a `mural.json` file |
| `:tool <name>` | switch brush — `pencil pen marker oil charcoal smudge` (unique prefixes ok) |
| `:set <prop>[=v]` | get/set tool props: `size`, `spacing` (per-tool), `color` (global); `+=` / `-=` work |
| `:color <n>` | set palette color index |
| `:export <path.png>` | PNG export of content bounds |
| `:undo` / `:redo` | undo / redo |
| `:help` | this help |

Commands abbreviate vim-style (`:w :q :e :se :col :exp`).

Layer order is fixed: images below paint below text.

## Icon & assets

`assets/` holds the app icon (gold impasto stroke on deep indigo):

- `icon-master.webp` — 1600px master
- `icon-{16,32,48,64,128,256}.png` — sized exports
- `icon.bmp` — 64px BMP loaded at startup as the SDL window icon
  (`assets/icon.bmp` must sit next to the working directory; missing file
  is silently ignored)

`mural.desktop` installs the app into desktop menus — adjust the `Exec`
and `Icon` paths to your install location.

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
