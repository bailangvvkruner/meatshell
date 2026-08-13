# Terminal rendering performance

This document records the current terminal-rendering architecture and the
acceptance gates for performance work. It distinguishes implementation limits
from measurements so historical results are not mistaken for validation of a
new build.

## Current architecture

### Bounded terminal updates

Terminal output is parsed into the authoritative `vt100` state, then coalesced
per tab before the Slint model is updated. Sustained output is scheduled no more
often than once every 33 ms. A tab can have one queued or active UI flush; output
that arrives during a flush requests at most one follow-up. Hidden tabs settle
their tickets without building a display snapshot.

The interval does not change when the window loses focus. This avoids leaving a
slow background frame queued when focus changes and keeps the Debug API's
reported `terminal_render_interval_ms` deterministic.

Normal shells use keyed, in-place `TermSpan` synchronization. Span identity is
the terminal row and column, while selection and search models update only rows
whose values changed. This keeps readline input, erase operations, cursor
movement, scrollback, emoji, selection, and search authoritative without
replacing an entire Slint model on every frame.

### Cached row images

On Windows, row images are selected only when the configured renderer is GPU
backed and the frame is a dense or alternate screen. Frames containing emoji
stay on the text path so Twemoji remains correct. Developers can force the text
path with `MEATSHELL_TERMINAL_RENDERER=spans` or request row images with
`MEATSHELL_TERMINAL_RENDERER=rows` for diagnostics.

The background Cosmic Text worker:

1. hashes each row with its text, style, font, and cell geometry;
2. shapes and rasterizes only rows whose signatures changed;
3. retains at most one waiting frame per tab, replacing it with newer work;
4. classifies completions by both generation and epoch, so a result invalidated
   by clear, close, resize, font/DPI change, or mode transition cannot reappear;
5. caps reusable pixels at 4 MiB and 256 buffers, glyph images at 2,048 entries,
   each row at 32 MiB, and periodically trims the shaping cache.

Cursor, selection, search highlights, and pointer handling remain Slint
overlays. Returning to a sparse shell clears row images and the reusable pixel
pool. This is not zero-copy: ANSI parsing, shaping, changed-row rasterization,
and texture upload still consume CPU and memory. The design instead bounds
queued work and avoids repeated layout of hundreds of text spans.

### Windows renderer and memory recovery

Existing and missing Windows settings use the software renderer for
compatibility. **Settings > Rendering** offers Automatic, GPU, and Software;
changes apply after restart, and an explicit `SLINT_BACKEND` environment value
takes precedence. GPU selects Slint FemtoVG. The vendored `glutin-winit` patch
prefers the packaged ANGLE EGL/OpenGL ES runtime (D3D11) and retains WGL as an
initialization fallback.

Windows x64 ZIP and MSI packages contain `libEGL.dll`, `libGLESv2.dll`,
`ANGLE_LICENSE.txt`, and `THIRD_PARTY_NOTICES.md`. The authenticated Debug API
reports the selected renderer, hardware-acceleration flag, whether ANGLE's EGL
DLL is actually loaded, and memory-trim diagnostics.

The Windows memory worker performs one startup trim after about two seconds.
Later passes wait until all dense terminals are gone or sparse and ten seconds
have elapsed since the last activity. When the process working set is at least
24 MiB, a pass asks ANGLE's `IDXGIDevice3` to trim when available, optimizes the
process heap, and trims the working set. Repeated trimming during a changing TUI
is deliberately avoided because it would exchange resident memory for page
faults and visible stutter.

## Historical baseline

The following data came from the pre-v0.6.10 customization branch on Windows 10
22H2, an Intel i5-10400 with 12 logical processors, and Intel UHD 630. The
workload was a release build connected to Linux `btop --force-utf` in a stable
1440x900 window. It is retained as a comparison target, not proof that the
current commit meets the same numbers.

<!-- markdownlint-disable MD013 -->

| Path | Machine CPU | One-core CPU | GPU | Working set |
| --- | ---: | ---: | ---: | ---: |
| Original installed MeatShell, software | 6.3%-7.4% | 76%-88% | about 0% | not recorded |
| Keyed span synchronization, software | 6.05%-6.34% | 72.6%-76.1% | about 0% | 72-93 MiB |
| Cached row images with ANGLE/D3D11 | 1.169% median | 14.02% median | 25.023% median D3D11 | 55.5-60.9 MiB private active |

<!-- markdownlint-enable MD013 -->

The old row-image run sampled CPU for 30 seconds. Its private commit did not
grow over that sample, and idle private working set settled around 17 MiB after
trim. These figures must be remeasured after the v0.6.10 integration before
being used as a release claim.

## Measurement protocol

Use a release build and record the exact commit, Windows version, CPU/GPU,
display scale, window size, renderer setting, `angle_runtime_loaded` result,
remote latency, and TUI refresh interval. For a continuously changing TUI:

- sample process CPU for at least 30 seconds and report median and p95;
- convert machine CPU to one-logical-core percentage when comparing machines;
- record the active GPU engine instead of total adapter utilization;
- record total/private working set and private commit while active, after
  returning to a sparse shell, and after the ten-second idle window;
- repeat window screenshots enough times to expose an unbounded encode queue;
- run the same workload in software mode as a compatibility baseline.

Idle terminals should consume effectively no CPU. A performance change should
not introduce an unbounded queue, linear memory growth, or input-to-model delay
beyond one 33 ms scheduling interval under normal load.

## Acceptance gates

Renderer changes must pass focused unit tests plus the full Rust check/test
suite. Manual or automated integration coverage must include:

- normal shell input, paste, backspace, erase, cursor movement, and clear;
- `btop`/`htop`, `vim`/`nano`, `tmux`, CJK, emoji, and ANSI colors;
- selection, find, scrollback, split panes, close/reopen, and alternate screen;
- resize, DPI/font changes, renderer restart, and first-frame rendering;
- xterm press/release/motion, Shift local selection, and SSH double click;
- repeated Debug API screen, input, pointer, health, and PNG screenshot calls;
- stale-frame rejection after input, clear, close, resize, and leaving a TUI;
- packaged hardware mode with ANGLE loaded and software fallback without it.

Do not disable advanced shaping or CJK/emoji handling to improve a benchmark
without equivalent visual-regression coverage. A lower Task Manager number by
itself is not a pass if it comes from repeated working-set eviction while a TUI
is active.
