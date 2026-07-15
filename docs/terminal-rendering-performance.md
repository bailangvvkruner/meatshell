# Terminal rendering performance

This document records the Windows terminal-rendering baseline, the current
bottleneck, and the migration path. It is intended to prevent small local
optimizations from turning into an unmaintainable renderer fork.

## Goal

For a continuously updating full-screen TUI such as `btop`:

- focused: stay below 25% of one logical CPU core at 30 FPS;
- unfocused: stay below 10% of one logical CPU core;
- do not build an unlimited frame queue;
- preserve CJK, ANSI colors, cursor, selection, search, scrollback, mouse
  reporting, and alternate-screen behavior;
- keep platform-specific rendering behind a small backend boundary.

Idle terminals should consume effectively no CPU. Latency from receiving output
to displaying it should remain below one focused-frame interval.

## Reproducible baseline

Measurements below were taken on 2026-07-15 on Windows 10 22H2 (build 19045),
an Intel i5-10400 (6 cores, 12 logical processors), and Intel UHD 630 graphics.
The workload was a Release build connected to `PVE-Alpine`, running `btop` at a
stable terminal size. The window stayed focused, UI Automation polling was
disabled, samples were taken only after the display had settled, and `q` was
sent after each run.

Windows reports process CPU as a percentage of total machine capacity. The
"one core" column multiplies that number by 12 so results can be compared with
profilers that report 100% for one saturated logical processor.

| Build/path | Machine CPU | One-core CPU | GPU | Working set |
| --- | ---: | ---: | ---: | ---: |
| Original installed Meatshell, software | 6.3%-7.4% | 76%-88% | ~0% | not recorded |
| Stable-model + keyed span synchronization, software | 6.05%-6.34% | 72.6%-76.1% | ~0% | 72-93 MiB |
| Keyed spans + nested-only alt-screen invalidation, software | 5.61%-5.63% | 67.4%-67.6% | 0% | 75.8 MiB |
| Same workload, `winit-femtovg` | 10.34% | 124.1% | 20.6% | 216 MiB |
| RustDesk displaying the changing screen | 0.13% | 1.6% | 33.7% | not recorded |

The RustDesk row is useful as an architectural reference, not as a direct
terminal benchmark. RustDesk receives already-rasterized video frames and
submits textures. Meatshell currently shapes and lays out hundreds of separate
text runs for every changed terminal frame.

A CPU profile of Meatshell showed a UI/render thread near 94% of one logical
core while the SSH ingest and vt100 parsing threads together remained around
0.4%. The primary problem is therefore Slint text-node layout/rasterization,
not SSH throughput or ANSI parsing.

## Current low-risk optimizations

The existing Slint renderer remains the supported backend. The following
changes reduce work without introducing another text engine:

1. Coalesce terminal output and cap focused rendering at about 30 FPS.
2. Back off to about 5 FPS while the window is unfocused.
3. Do not publish or redraw an identical terminal frame.
4. Keep the `VecModel<TermSpan>` identity stable.
5. Synchronize spans by `(row, column)`, so a split or merged ANSI run does not
   rewrite every later model element.
6. On alternate screens, update the nested span model without replacing the
   outer `TerminalState`; the viewport is already pinned to `y = 0`.

These changes help model churn, but they cannot reach the target by themselves.
Slint still creates one `Rectangle` and one `Text` for every colored run. A
typical `btop` frame can contain hundreds of such nodes.

## What leading implementations do

The common design is damage tracking plus reusable raster/GPU resources:

- [Alacritty](https://github.com/alacritty/alacritty/blob/852e971cddfabe222d2d5bcda466e130f53af207/alacritty_terminal/src/term/mod.rs#L137)
  tracks left/right damage bounds independently for every terminal line.
- [foot](https://codeberg.org/dnkl/foot/src/commit/3c5b584b0eafa772eb4376fb6eaf6643399e190e/render.c#L1580)
  skips clean rows, groups consecutive dirty rows, and submits corresponding
  Wayland surface damage.
- [WezTerm](https://github.com/wezterm/wezterm/blob/d96ba57121761cbafda4c08179fe45f8e4cf8212/wezterm-gui/src/termwindow/render/pane.rs#L446)
  caches complete per-line GPU quad allocations in addition to shaping and
  glyph caches.
- [kitty](https://github.com/kovidgoyal/kitty/blob/f47590533d7177daf0b74963f9d1b7581467af20/kitty/fonts.c#L63)
  rasterizes glyphs into a sprite map and renders terminal cells through GPU
  shaders.
- [Windows Terminal AtlasEngine](https://github.com/microsoft/terminal/blob/922beefb83764646331662d6d15d70107d556402/src/renderer/atlas/AtlasEngine.cpp#L89)
  tracks invalidated rows, caches shaped rows/glyphs, and passes dirty regions
  to `Present1`.
- [RustDesk](https://github.com/rustdesk/rustdesk/blob/cf2b28faf934fedb498c33b9746cd289426d8645/src/client/io_loop.rs#L1171)
  adapts FPS to decoder capacity and queue pressure, while its Windows Flutter
  path can submit decoded GPU textures directly.

The transferable RustDesk ideas are a bounded latest-frame queue, dropping
obsolete work, adaptive pacing, and texture submission. Its video codec and
capture pipeline are not useful for rendering a local terminal grid.

## Recommended architecture

### Phase 1: explicit row damage

Introduce a renderer-neutral snapshot boundary:

```rust
struct TerminalFrame {
    generation: u64,
    rows: Vec<TerminalRow>,
    dirty_rows: Vec<usize>,
    cursor: CursorState,
}

struct TerminalRow {
    hash: u64,
    cells: Vec<StyledCell>,
}
```

Compare row hashes after vt100 ingestion and rebuild only changed rows. Moving
the cursor damages both its old and new rows. Resize, font, DPI, palette, theme,
and renderer changes invalidate all rows. Selection and search remain separate
overlays and damage only intersecting rows.

Keep this model independent of Slint so parser, damage, and rendering tests can
run headlessly.

### Phase 2: cached row images

Replace hundreds of Slint `Text` instances with roughly one `Image` per visible
row. Raster dirty rows on a worker using Cosmic Text/Swash into reusable RGBA
buffers, then update only those row images on the UI thread.

The worker queue must have capacity one. A newer generation replaces an older
pending frame; completed rows whose session/generation no longer matches are
discarded. This prevents latency and memory growth during output bursts.

Keep cursor, selection, search highlights, and mouse handling as lightweight
Slint overlays. This preserves interaction behavior while removing text layout
from the hot UI path. A 960 x 20 RGBA row is about 75 KiB; 50 rows require about
3.7 MiB and buffers should be reused.

Cosmic Text/Swash is preferred over a Windows-only DirectWrite implementation
because it keeps shaping, fallback, CJK, and raster behavior shared across
platforms. Font discovery can remain platform-specific behind the backend.

### Phase 3: optional GPU cell renderer

Only start this phase if Phase 2 misses the focused CPU target or high-DPI/large
terminal targets. Use a glyph atlas plus instanced cell quads, update atlas
entries lazily, upload only dirty cell ranges, and retain the same
`TerminalFrame`/damage contract.

On Windows, Direct3D 11/12 plus DirectWrite and dirty-rectangle presentation is
the proven path. A cross-platform `wgpu` backend is possible but has a larger
maintenance surface. Do not depend on Slint private renderer APIs; either use a
documented custom-render hook or host the terminal surface behind an isolated
native component.

## Acceptance gates

Each phase must pass the existing test suite and scripted interactive checks for
shell output, `btop`, `vim`/`nano`, `tmux`, CJK, selection, find, scrollback,
resize, split panes, and mouse reporting. Performance runs use the protocol
above and report median CPU over at least 30 seconds plus memory and GPU use.

Do not replace the software renderer with femtovg by default: on the measured
Windows system it consumed more CPU, GPU, and memory. Do not disable advanced
text shaping globally without visual regression coverage; the measured gain was
modest and the text-quality risk is real.
