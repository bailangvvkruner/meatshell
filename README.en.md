# meatshell

[简体中文](./README.md) | **English**

> [!IMPORTANT]
>
> ## Changes from upstream v0.6.10
>
> This fork includes upstream v0.6.10 and carries these additions on top of its
> current module layout:
>
> - Local (1-30 seconds) and remote SSH (1-60 seconds) resource intervals are independently configurable, apply live, and persist. Timed-out remote collectors reconnect automatically, and process parsing supports BusyBox `top`/`ps`.
> - SSH stdout, stderr, and post-ZMODEM output use incremental UTF-8 decoding, preserving CJK, box drawing, and emoji split across network packets.
> - xterm SGR, UTF-8, and legacy mouse reports support press, release, and motion. Shift keeps local selection available, while SSH pointer presses are paced for reliable TUI double clicks.
> - Rejected passwords prompt again for SSH terminals, SFTP, and jump hosts. Each bounded retry uses a fresh connection instead of reusing a failed transport.
> - Dense terminals use a bounded background row-raster cache in Windows GPU mode, while normal shells retain the lower-memory text path. Epoch checks prevent stale frames after close, clear, resize, or renderer changes.
> - An opt-in Debug API is restricted to `127.0.0.1` and requires a Bearer token; see the [API guide](docs/debug-api.md).
> - Windows x64 packages include an optional ANGLE/EGL D3D11 runtime and its license. Software remains the compatibility default; select GPU under **Settings > Rendering** and restart to use it.

A lightweight, low-memory SSH / terminal client inspired by FinalShell, but
written entirely in **Rust + [Slint](https://slint.dev)**. The goal is to keep
FinalShell's core experience (resource-monitor sidebar, session management,
tabbed terminals) while cutting memory use from the 400 MB+ of a JVM app down to
the tens-of-MB range of a native binary.

## Screenshots

<p align="center">
  <img src="docs/screenshots/01-welcome-en.png" alt="Welcome / session management" width="800"><br>
  <em>Welcome page: session management + local resource monitor sidebar</em>
</p>

<p align="center">
  <img src="docs/screenshots/02-terminal-htop.png" alt="Terminal + SFTP" width="800"><br>
  <em>Tabbed terminal (full-screen btop) + SFTP file browser + remote resource monitoring</em>
</p>

## Download & install

Every push to `main` builds a Windows x64 nightly ZIP and MSI and updates the
rolling [nightly Release](https://github.com/bailangvvkruner/meatshell/releases/tag/nightly).
Every `v*` tag still builds formal **Windows / Linux / macOS** artifacts on this
fork's [Releases](https://github.com/bailangvvkruner/meatshell/releases) page.

### Windows

Download `meatshell-*-windows-x86_64.zip`, unzip, and run `meatshell.exe`, or use
the `.msi` from the same Release. Keep `libEGL.dll`, `libGLESv2.dll`, and
`ANGLE_LICENSE.txt` beside the executable; they support the optional GPU mode.

### Linux

```bash
tar -xzf meatshell-*-linux-x86_64.tar.gz
cd meatshell-*-linux-x86_64
./meatshell                                  # run it directly
# Optional: install the app icon + launcher entry (shows the icon in the dock /
# app list — no argument needed, it finds the binary next to the script)
chmod +x install-linux.sh && ./install-linux.sh
```

> Requires glibc ≥ 2.35 (Ubuntu 22.04+ / Debian 12+). On Wayland you may need to
> log out/in once after installing the icon.

Building from source with `cargo run` on Linux Mint / Ubuntu / Debian requires
the Slint/winit/rfd system development packages:

```bash
sudo apt update
sudo apt install -y --no-install-recommends \
  build-essential pkg-config cmake \
  libfontconfig1-dev libfreetype6-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libgl1-mesa-dev libegl1-mesa-dev libgtk-3-dev \
  libudev-dev
```

### macOS

The download is a `.zip` containing the `meatshell.app` bundle:

```bash
# Unzip (aarch64 = Apple Silicon, x86_64 = Intel)
unzip meatshell-*-macos-*.zip
# Move it to Applications (optional — it also runs in place)
mv meatshell.app /Applications/
# Clear the quarantine flag, otherwise macOS says "meatshell is damaged and can't be opened"
xattr -dr com.apple.quarantine /Applications/meatshell.app
# Open it (or double-click in Finder)
open /Applications/meatshell.app
```

> If you didn't move it to `/Applications`, point both paths above at wherever the `.app` actually is (e.g. `~/Downloads/meatshell.app`).

> To build from source, see [Running](#running) below.

## Features

### Done

- [x] FinalShell-style UI with dark / light / follow-system themes
- [x] Local + remote resource monitoring (CPU / memory / swap / network / disk) with separate intervals and automatic remote-monitor recovery
- [x] Remote process monitor (CPU-sorted table with PID copy and permission-aware termination), including GNU and BusyBox output
- [x] Full VT/ANSI terminal emulation (btop / htop / vim render correctly) with incremental UTF-8 and xterm mouse reporting
- [x] Background row-raster cache for dense terminals plus a text fast path for normal shells ([performance and acceptance notes](docs/terminal-rendering-performance.md))
- [x] Color emoji, including skin tones, flags, and ZWJ sequences
- [x] Tabs (welcome page + multiple sessions)
- [x] Session management: create / edit / delete / groups, local JSON, export / import
  - Config location: `%APPDATA%/meatshell/sessions.json` (Windows)
    / `~/.config/meatshell/sessions.json` (Linux)
    / `~/Library/Application Support/meatshell/sessions.json` (macOS)
- [x] SSH (`russh`, pure Rust): password / private key / encrypted key (passphrase), with credential re-entry after rejection
- [x] SFTP browser + upload / download (drag-and-drop) + in-terminal ZMODEM (`sz`) receive
- [x] SSH port forwarding / tunnels: local -L / remote -R / dynamic -D (SOCKS5)
- [x] Quick commands + command box (broadcast to all sessions) + command history
- [x] Serial / Telnet sessions
- [x] Outbound proxy (SOCKS5 / HTTP)
- [x] Import `~/.ssh/config`
- [x] Session passwords encrypted at rest (ChaCha20-Poly1305)
- [x] Known-hosts (`known_hosts`) verification + first-connect confirmation
- [x] Split panes for tabbed terminals
- [x] Opt-in loopback Debug API (Bearer auth, terminal screen/input/pointer, and window screenshots)

Color emoji graphics are provided by [Twemoji](https://github.com/jdecked/twemoji)
under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the full attribution.

### Planned

- [ ] Store session passwords in the OS keychain

## Tech stack

| Module        | Choice                                                            |
| ------------- | ----------------------------------------------------------------- |
| UI            | [Slint](https://slint.dev) (compiled pure Rust, no GC)            |
| Async runtime | [`tokio`](https://tokio.rs)                                       |
| SSH protocol  | [`russh`](https://crates.io/crates/russh) (no libssh dependency)  |
| System metrics| [`sysinfo`](https://crates.io/crates/sysinfo)                     |
| Row rasterizer| [`cosmic-text`](https://crates.io/crates/cosmic-text)             |
| Debug API     | [`axum`](https://crates.io/crates/axum)                           |
| Serialization | `serde` + `serde_json`                                            |
| Logging       | `tracing` + `tracing-subscriber`                                  |

## Running

```bash
cargo run --release
```

On first launch an empty session store is created at
`%APPDATA%/meatshell/sessions.json`. Click **"＋ New Session"** in the top-right
to add your first server.

## Project layout

```
meatshell/
├── Cargo.toml
├── build.rs                 # Slint compiler entry point
├── ui/
│   ├── app.slint            # top-level window
│   ├── theme.slint          # design tokens
│   ├── widgets.slint        # reusable buttons / inputs / sparkline
│   ├── sidebar.slint        # left-hand system monitor panel
│   ├── welcome.slint        # welcome page / quick connect
│   ├── session_dialog.slint # new / edit session dialog
│   └── terminal_view.slint  # text and row-image terminal view
└── src/
    ├── main.rs              # process entry and runtime
    ├── app.rs + app/        # UI/backend bridge and callback modules
    ├── config/              # sessions, settings, encrypted persistence
    ├── ssh/ + sftp/         # SSH, authentication, monitoring, file transfer
    ├── terminal/            # VT state, input, mouse, and row rasterization
    ├── debug_api.rs         # loopback HTTP Debug API
    └── memory_trim.rs       # Windows idle-memory reclamation
```

## Development notes

- Slint widgets use a strict layout DSL; after editing a `.slint` file,
  `cargo check` is the fastest feedback loop.
- The application event loop is single-threaded (required by Slint); all
  cross-thread UI updates go through `slint::invoke_from_event_loop` callbacks.
- SSH / SFTP share the `known_hosts` verification path: first contact asks for
  trust and remembers the host key, while later key changes prompt again.
- See [docs/debug-api.md](docs/debug-api.md) for the local API, request limits,
  and PowerShell examples.
- Renderer boundaries and regression gates are in
  [docs/terminal-rendering-performance.md](docs/terminal-rendering-performance.md);
  dependency-audit exceptions are documented in
  [docs/security-audit.md](docs/security-audit.md).

## Release

Do not bump `Cargo.toml` by hand and then create a tag. Use the release helper
so the tag points at a commit that already contains the matching Cargo version:

```powershell
.\scripts\release.ps1 v0.6.0 -Push
```

The script updates `Cargo.toml` / `Cargo.lock`, runs `cargo check --locked`,
verifies `meatshell --version`, commits `Release v0.6.0`, creates an annotated
tag, and pushes the current branch plus the tag. See
[docs/release.md](docs/release.md) for details.

## License

Dual-licensed under MIT OR Apache-2.0.
