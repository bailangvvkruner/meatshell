# 上游同步计划（v0.6.10 分叉 → v0.8.4 基线）

本文档记录把本项目从 `v0.6.10` 追到上游 `v0.8.4` 的过程、取舍与验收标准。
`main` 分支**不受任何影响**，所有工作都在 `sync/v0.8.4` 分支 / 独立工作树里完成。

## 1. 背景数据

| 项 | 值 |
| --- | --- |
| 分叉点 | 上游 `v0.6.10`（`557a9d2 Merge upstream v0.6.10`） |
| 我方 `main` | 26 个自有提交（33 commits 含 merge） |
| 上游 `main` | `v0.8.4`，448 个提交，1178 文件 / +264k −11k（排除 `vendor/` 后 267 文件 / +76k −11k） |
| 直接 merge 冲突 | 30 个文件（`ui/app.slint` 4415 行、`ui/sftp_panel.slint` 3258 行、`src/app.rs`、`src/terminal/*` …） |

结论：**不做 merge，改用「上游为新基线 + 重放自有功能」**。原因是我方 26 个提交里有相当一部分
上游已独立实现（终端渲染性能、鼠标跟踪转发、窄窗口换行 / PTY 网格、ANGLE GPU、字体回退），
保留旧实现只会在上游重构后的模块划分里制造语义冲突。

## 2. 分支与工作树

```text
D:/aaa/git/github/meatshell        main（保持不动）
D:/aaa/git/github/meatshell-sync   sync/v0.8.4（基于 upstream/main，跟踪 upstream/main）
```

- 基线提交：`0d74769 chore(release): v0.8.4 发布`
- 工作树通过 `git worktree add` 创建，与 `main` 共享同一个 `.git`，互不影响。
- `main` 只在该同步分支验收通过后，由维护者决定是 `--ff-only` 合并还是直接改推。

## 3. 保留清单（重放）

| 序号 | 功能 | 源提交（main） | 目标落点（v0.8.4 结构） | 状态 |
| --- | --- | --- | --- | --- |
| 1 | Windows x64 夜间构建 + 滚动 `nightly` 预发布 | `945ed26` | `.github/workflows/release.yml`（新增 `nightly-windows` job） | 已完成 |
| 2 | 资源监控刷新频率可配置 | `94fe429`、`551c660` | `src/config/*`、`src/ssh/impls/ssh.rs`、`src/app/open_window.rs`、`src/app/settings_callbacks.rs`、`ui/interface_panel.slint` | 已完成 |
| 3 | 剪贴板免密 SSH 快速连接 | `979f83c` | `src/app/clipboard_connect.rs`（新增）、`src/app/session_callbacks.rs`、`src/config/struct/session.rs`、`ui/app.slint` | 已完成 |
| 4 | Debug HTTP API（127.0.0.1:24817，Bearer token） | `ed462f0`（部分） | `src/debug_api.rs`（新增）、`src/main.rs`、`src/app.rs`、`src/app/open_window.rs`、`src/session/struct/prompts.rs`、配置项、`ui/*.slint`、`docs/debug-api.md` | 已完成 |

### 3.1 与 `main` 实现方式的差异（有意为之）

- **资源刷新频率**：`main` 用 `tokio::sync::watch` 贯穿 `ConnectCtx`；这里改用与上游
  `KEEPALIVE` 同款进程级 `AtomicU32`（`ssh::REMOTE_MONITOR_INTERVAL`），开监控通道时读取并
  烘焙进远端 shell 循环，因此不改 `ConnectCtx` 签名、也不会打断已运行的监控。代价：远程
  间隔只对新建连接生效（设置页已注明）。本地采样仍由 UI timer 按设置间隔取数。
- **Debug API**：`TerminalScreenSnapshot` 里依赖 `main` 专有渲染/btop 补偿逻辑的字段
  （`cell_width/height_logical_px`、`synchronized_output`、`legacy_btop_compat`、
  `sanitized_btop_control_bytes`）在本分支报告为常量，`docs/debug-api.md` 末尾已写明。
  状态改为窗口作用域（每个窗口一份），输入通道仍只在传输层 `Connected` 之后才发布。
- **剪贴板免密**：复用上游 `agent::Agent` + `passwordless_identity_paths`，运行时会话标
  `runtime_passwordless`（`serde(skip)`），不写入 `sessions.json` / 最近列表。

## 4. 丢弃清单（上游已有等价实现）

| 项 | 我方提交 | 上游对应 |
| --- | --- | --- |
| ANGLE / GPU 渲染 + vendored `glutin-winit`、`vendor/angle/*.dll` | `1ea46c7`、`88a3cc2` | 上游自带渲染器选择（CPU / FemtoVG / Skia），涉及 ANGLE 的 27 个文件 |
| 终端渲染性能、60Hz、原地更新、滚动缓冲内存 | `dfd2bde`、`e0845f6`、`887b708`、`d2dfac3`、`9291dee` | CHANGELOG：#499 及「Smoother rendering and lower memory」 |
| 鼠标跟踪转发 / DEC 线框字符 | `3bac35a` | #399、#400 |
| 窄窗口换行、PTY 列宽与初始网格同步 | `c2d6d62`、`909c903` | #433 |
| `rust-toolchain.toml` 固定 1.97.0 | `07a4bd0`、`ed462f0` | 上游 CI 用 `dtolnay/rust-toolchain@stable`，不锁版本 |
| `.cargo/audit.toml` 搬家 | `ed462f0` | 上游仍为根目录 `audit.toml` |
| AUR 发布守卫 | `9e65e97` | 上游已含 `github.repository` 判断，直接取上游 |

## 5. 验收标准

1. `sync/v0.8.4` 相对 `upstream/main` 的差异**仅**包含第 3 节四个功能；
2. `cargo build --locked --release` 在 CI 上三平台通过；
3. `cargo test` 不回归（上游已有测试全绿）；
4. 每个功能一个独立提交，提交信息带 `sync:` 前缀，便于日后追踪；
5. `main` 分支历史零改动。

## 6. 后续同步节奏

上游 2026-08-14 → 2026-11-22 三个月 448 个提交。建议：

```text
每 2 周：git fetch upstream && git switch sync/v0.8.4 && git merge upstream/main
```

因为自有代码被压缩到 4 个功能点，冲突面稳定在 5~8 个文件以内，可长期维护。

## 7. 回滚

```text
git switch main                       # 回到主分支，worktree 保留
git worktree remove --force D:/aaa/git/github/meatshell-sync
git branch -D sync/v0.8.4
```

## 8. Windows 本机构建验证（2026-11-23）

本机无管理员权限、无 VS Build Tools / Windows SDK，因此使用
`x86_64-pc-windows-gnu` 工具链；但 rustup 的 GNU 目标只自带自包含
import lib，缺 `dlltool.exe`（`windows-sys` / `getrandom` / `windows-result`
构建时会报 `error calling dlltool 'dlltool.exe': program not found`）。
解决方式：把完整 MinGW-w64 解压到 D 盘并前置进 PATH（不写 C 盘）：

```text
D:\rust\mingw64      # gcc 16.2.0 + binutils 2.47（niXman mingw-builds, msvcrt 运行时）
D:\rust\cargo        # CARGO_HOME
D:\rust\rustup       # RUSTUP_HOME
D:\rust\temp         # TEMP/TMP 与构建日志
```

`D:\rust\temp\build-sync.ps1` 一键设置上述变量并跑 `cargo build` + `cargo test --no-run`。

为让该分支在 GNU 工具链下真正编译、测试，这条分支相对上游 v0.8.4 额外补了 4 处
移植缺口（都是「自己的功能」在基线里的残留依赖）：

| 文件 | 改动 | 原因 |
| --- | --- | --- |
| `src/ssh/struct/command.rs` + `src/ssh/impls/ssh.rs` + `src/terminal/impls/{local,serial,telnet}.rs` | 新增 `SessionCommand::DebugInput { bytes, ack }`，四条传输层都在写入后回 ack（失败则回 `Err` 并关闭会话） | Debug API 的 `/input` 依赖 ack 语义；基线里只有 `RawInput` |
| `src/app/window.rs` + `src/app.rs` | 记录 Slint 实际请求的渲染器（`ACTIVE_RENDERER` / `OnceLock`），导出 `active_renderer_name()` / `active_renderer_uses_gpu()` | `open_window.rs` 的 Debug API 诊断需要这两个值 |
| `src/app/open_window.rs` | vt100 0.18 的 `Screen::size()` 返回 `NonZeroU16`，改用 `.get()`；`parse_errors` 固定为 0（该版本无解析错误计数） | 基线的快照代码按更高版本 vt100 写 |
| `tests/app/modal_layers.rs` | `wire_session_callbacks` 补齐第 27 个参数（`debug_api`） | 基线新增了 Debug API 控制器 |
| `tests/ui_session_editor.rs` | “Add trigger”点击坐标改为按渲染像素定位（蓝色按钮行扫描） | 上游该测试的固定坐标 (505, 456) 在 X11 转发区块加入后已失效，任何平台都会点到 DISPLAY 输入框 |

结果：`cargo build`（debug）通过；`cargo test --no-fail-fast` 全绿
（838 个单测 + 9 个 UI 集成测试 + 其余 UI 测试用例）。

## 9. CI 矩阵修复（2026-11-23）

`sync-check.yml` 在 ubuntu / windows / macos 上跑 `cargo check --locked --all-targets`
+ `cargo test --locked`。首轮：三个平台**编译全部通过**，windows 测试全绿，
ubuntu 4 个、macos 9 个测试失败——都是上游从未在这两个平台跑过的测试，
断言直接依赖宿主机路径或权限：

| 测试 | 失败现象 | 修法 |
| --- | --- | --- |
| `local_shells::custom_shell_starts_its_program_directly_with_split_args` | unix 上 `host` 得到整个 `C:\msys64\usr\bin\zsh`：`Path::file_stem` 只认宿主分隔符 | `program_stem` 先按 `/` 与 `\` 切出文件名，再取 stem（配置文件跨平台复用） |
| `sftp_complete::local_unix_paths_split_at_the_last_slash` | `~/Do` 的列表目录多一个尾斜杠（`Path::join("")` 保留分隔符） | `~` 之后为空时用 `home_display()`，与面板自身的 home 文本一致 |
| `update::inplace::{install_script_copy…, classifies_appimage…}` | GitHub 的 ubuntu runner 把 `/usr/local` chown 给构建用户，`/usr/local/bin` 可写，分类器正确地走了 `LinuxTarball` 而非 `SystemBinary`/`Manual` | 断言按 `dir_is_writable("/usr/local/bin")` 取期望值，不再假设 root 所有 |
| `modal_layers_tests::every_app_modal_cancels_after_keyboard_navigation`（仅 macos） | Slint 把 macOS Command 映射成逻辑 control 修饰键，测试却在 mac 上发送 `Key::Meta`，模态收不到取消 | 统一发送 `Key::Control`（真实 Cmd 到达时就是该逻辑修饰键） |
| `single_instance::*`（仅 macos） | `Error { kind: InvalidInput, message: "path must be shorter than SUN_LEN" }`：macOS 临时目录本身就很长，测试又把整个测试名塞进 socket 文件名 | 文件名改为测试名哈希（`ipc-<hash>.sock`），macOS 上约 84 字节 < 104 |

这些都是测试侧（含两处小的显示名/路径文本修正），不改变已发布行为。
