# Terminal rendering performance

This document records the Windows terminal-rendering baseline, the production
architecture, and the acceptance gates. It exists to keep performance changes
measurable and to avoid an unmaintainable renderer fork.

## Goals

For a continuously updating full-screen TUI such as `btop`:

- keep median CPU below 25% of one logical core;
- use the same low-latency terminal scheduling interval while focused and
  unfocused, so changing applications cannot leave a stale frame queued;
- use the GPU for composition when hardware acceleration is enabled;
- keep at most one pending raster job per tab and discard obsolete results;
- preserve CJK, ANSI colors, cursor, selection, search, scrollback, mouse
  reporting, themes, and alternate-screen behavior;
- release dense-terminal and graphics working sets after returning to an idle
  shell.

Idle terminals should consume effectively no CPU. Input-to-display latency
should remain below one scheduling interval: 16 ms on the Windows GPU path and
33 ms on the software fallback.

## Historical baseline

Measurements below were taken on 2026-07-15 on Windows 10 22H2 (build 19045),
an Intel i5-10400 (6 cores, 12 logical processors), and Intel UHD 630 graphics.
The workload was a Release build connected to a Linux host running `btop` in a
stable terminal size.

Windows reports process CPU as a percentage of total machine capacity. The
"one core" column multiplies that number by 12 so results can be compared with
profilers that report 100% for one saturated logical processor.

| Build/path | Machine CPU | One-core CPU | GPU | Working set |
| --- | ---: | ---: | ---: | ---: |
| Original installed MeatShell, software | 6.3%-7.4% | 76%-88% | ~0% | not recorded |
| Keyed span synchronization, software | 6.05%-6.34% | 72.6%-76.1% | ~0% | 72-93 MiB |
| Nested-only alt-screen invalidation, software | 5.61%-5.63% | 67.4%-67.6% | 0% | 75.8 MiB |
| Span model with `winit-femtovg` | 10.34% | 124.1% | 20.6% | 216 MiB |
| RustDesk displaying the changing screen | 0.13% | 1.6% | 33.7% | not recorded |

The RustDesk row is an architectural reference, not a direct terminal
comparison. RustDesk receives decoded image frames, whereas MeatShell must
parse ANSI state, shape text, and rasterize terminal cells.

Profiling the old path put roughly 94% of one logical core in UI text layout
and rendering while SSH ingest and vt100 parsing together used about 0.4%.
The primary bottleneck was hundreds of Slint text nodes per TUI frame, not SSH
throughput.

## Production architecture

### Bounded terminal updates

Terminal output is coalesced per tab. A render gate allows one queued or active
flush, remembers output that arrived during that flush, and schedules at most
one follow-up. GPU and software intervals are 16 ms and 33 ms respectively.
Focus does not change these values; an earlier 5 FPS background policy caused
visible stalls and stale frames when switching applications.

Normal shell screens keep the keyed `TermSpan` model. This is the lower-memory
path for sparse text and lets readline input, erase operations, cursor updates,
selection, and search remain immediately authoritative.

### Cached row images

Dense and alternate-screen frames use cached row images on the Windows GPU
path. The worker:

1. hashes each row together with its font and cell geometry;
2. shapes and rasterizes only rows whose signatures changed;
3. keeps only the newest pending frame for each tab;
4. rejects results from an old tab epoch or generation;
5. merges pending UI deltas by row before applying them;
6. bounds the reusable pixel pool to 4 MiB, the glyph image cache to 2,048
   entries, and periodically trims Cosmic Text's shape-run cache.

Cursor, selection, search highlights, and pointer handling stay as lightweight
Slint overlays. Returning from a TUI to a sparse shell destroys row images,
clears the pixel pool, and restores the native span path. The implementation is
cross-platform Rust and Cosmic Text; no private Slint renderer API is used.

This is not end-to-end zero-copy: ANSI parsing and glyph rasterization are CPU
work, and changed rows must be uploaded to the GPU. It does remove repeated
per-span UI layout and bounds the copies and queues that remain.

### Windows GPU path

Windows x86-64 defaults to Slint FemtoVG backed by the bundled ANGLE EGL
runtime and D3D11. The setting at **Settings > Interface > Hardware
acceleration** is enabled for new and existing configurations unless explicitly
disabled. Changing it saves the preference and immediately restarts MeatShell
without creating a console window. Software rendering remains available as a
recovery path and `SLINT_BACKEND` remains an expert override.

The authenticated Debug API reports the selected backend, whether it is
GPU-backed, whether the bundled ANGLE runtime is actually loaded, and the
terminal scheduling interval. Checking only the backend name is insufficient:
`angle_runtime_loaded` confirms the packaged DLL path is in use.

### Idle memory reclamation

Ten seconds after the last dense terminal becomes sparse or closes, the
Windows memory worker:

- clears reusable terminal pixel buffers;
- calls `IDXGIDevice3::Trim` for ANGLE's D3D11 device;
- asks the Windows heap to optimize unused resources;
- trims the process working set when it is at least 24 MiB.

This is intentionally idle-only. Trimming during a changing TUI would trade a
smaller Task Manager number for page faults and visible stutter.

## Current measurements

The production-path run below was taken on 2026-07-17 on the same 12-logical-
processor Windows machine. A Release build used bundled ANGLE/D3D11 and a
1440x900 window connected over SSH to Linux `btop --force-utf` at a 100 ms btop
update interval. MeatShell stayed unfocused and visible; Debug API sampling did
not take focus.

| Metric | Result |
| --- | ---: |
| 30 s machine CPU median | 1.169% |
| 30 s machine CPU p95 | 1.661% |
| 30 s one-core CPU median | 14.02% |
| D3D11 3D engine median, 10 samples | 25.023% |
| Active btop private working set | 55.5-60.9 MiB |
| Active btop private commit | 144.7-149.1 MiB |
| Private commit growth during CPU run | -0.71 MiB |
| Idle private working set after trim and diagnostics | about 17 MiB |
| Idle total working set after trim and diagnostics | about 25 MiB |

Immediately after the ten-second trim, total working set reached 8.6 MiB. It
then settled near 25 MiB because the test continued polling health and loading
diagnostic code pages. GPU mode reserves more virtual/private commit in ANGLE
and the display driver than software mode; that commit is not the same as
resident private working set.

The screenshot endpoint was also exercised for 80 full-window PNG captures
while btop kept redrawing. The first pass established the PNG worker and
allocator high-water mark; the second pass increased peak private commit by
only about 1.5 MiB rather than growing linearly. After btop exited, the normal
idle trim reclaimed the resident row and graphics working set.

## Industry references

The implementation follows the same damage-and-cache principles used by
leading terminals:

- [Alacritty](https://github.com/alacritty/alacritty/blob/852e971cddfabe222d2d5bcda466e130f53af207/alacritty_terminal/src/term/mod.rs#L137)
  tracks damage bounds per line.
- [foot](https://codeberg.org/dnkl/foot/src/commit/3c5b584b0eafa772eb4376fb6eaf6643399e190e/render.c#L1580)
  skips clean rows and groups dirty-row surface damage.
- [WezTerm](https://github.com/wezterm/wezterm/blob/d96ba57121761cbafda4c08179fe45f8e4cf8212/wezterm-gui/src/termwindow/render/pane.rs#L446)
  caches per-line GPU allocations and shaped data.
- [kitty](https://github.com/kovidgoyal/kitty/blob/f47590533d7177daf0b74963f9d1b7581467af20/kitty/fonts.c#L63)
  uses a glyph sprite map and GPU cell rendering.
- [Windows Terminal AtlasEngine](https://github.com/microsoft/terminal/blob/922beefb83764646331662d6d15d70107d556402/src/renderer/atlas/AtlasEngine.cpp#L89)
  tracks invalid rows and presents dirty regions.
- [RustDesk](https://github.com/rustdesk/rustdesk/blob/cf2b28faf934fedb498c33b9746cd289426d8645/src/client/io_loop.rs#L1171)
  bounds its latest-frame queue and adapts to queue pressure.

The transferable RustDesk ideas are a bounded latest-frame queue, dropping
obsolete work, pacing, and texture submission. Its video codec and capture
pipeline are not applicable to a local terminal grid.

## Future work

A glyph atlas with instanced cell quads and dirty-range uploads could reduce
CPU and upload bandwidth further. It should be considered only if cached row
images miss the acceptance target on additional hardware or high-DPI displays.
Any such backend must retain the current parser/damage boundary and software
fallback rather than spreading D3D-specific logic through terminal behavior.

## Acceptance gates

Renderer changes must pass the full Rust test and lint suite plus scripted
checks for shell input and erase, `btop`, `vim`/`nano`, `tmux`, CJK, selection,
find, scrollback, resize, split panes, mouse reporting, first-frame rendering,
and hardware/software restart. Performance runs report median CPU over at least
30 seconds, GPU engine activity, active and idle memory, and repeated screenshot
behavior. Advanced shaping must not be disabled without visual regression
coverage.
