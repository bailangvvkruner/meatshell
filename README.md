# meatshell

**简体中文** | [English](./README.en.md)

> [!IMPORTANT]
>
> ## 本分支相对上游 v0.6.10 的增强
>
> 本分支已合并上游 v0.6.10，并在新版模块结构上保留和补充以下改进：
>
> - 本机（1-30 秒）与远端 SSH（1-60 秒）资源刷新间隔可分别调整、即时生效并持久化；远端采样超时后会自动重建监控通道，进程解析兼容 BusyBox `top`/`ps`。
> - SSH 标准输出、标准错误和 ZMODEM 剩余输出使用增量 UTF-8 解码，跨网络分包的中文、框线和 emoji 不再被替换或扩宽。
> - 完整转发 xterm SGR、UTF-8 与传统鼠标报告，支持按下、释放和移动；按住 Shift 可保留本地选择，SSH 连续按下事件会适度分隔以兼容 TUI 双击。
> - 密码认证被拒绝后，SSH 终端、SFTP 和跳板机会重新提示凭据；每次重试使用新连接并限制次数，避免在失败连接上挂起。
> - 稠密终端在 Windows GPU 模式下使用有界的后台行栅格缓存，普通 shell 保留低内存文本路径；代次校验防止关闭、清屏或缩放后的旧帧回写。
> - 增加默认关闭、仅监听 `127.0.0.1` 且要求 Bearer Token 的本机 Debug API，详见 [调用说明](docs/debug-api.md)。
> - Windows x64 包含可选 ANGLE/EGL D3D11 运行时和许可证；默认仍使用兼容性更好的软件渲染，可在“设置 → 渲染”中选择 GPU 并重启应用。

一个轻量级、低内存占用的 SSH / 终端客户端，灵感来自 FinalShell，但完全由
**Rust + [Slint](https://slint.dev)** 实现。目标是保留 FinalShell 的核心体验
（资源监控侧栏、会话管理、多标签页终端）的同时，把内存占用从 400 MB+ 的
JVM 压到几十 MB 原生级别。

## 截图

<p align="center">
  <img src="docs/screenshots/01-welcome.png" alt="欢迎页 / 会话管理" width="800"><br>
  <em>欢迎页：会话管理 + 左侧本机资源监控</em>
</p>

<p align="center">
  <img src="docs/screenshots/02-terminal-htop.png" alt="终端 + SFTP" width="800"><br>
  <em>多标签页终端（htop 全屏渲染）+ 底部 SFTP 文件浏览 + 远端资源监控</em>
</p>

## 下载与安装

每次推送 `main` 都会构建 Windows x64 nightly ZIP 与 MSI，并更新滚动的
[nightly Release](https://github.com/bailangvvkruner/meatshell/releases/tag/nightly)。
每个 `v*` 标签仍会构建 **Windows / Linux / macOS** 正式产物，发布到本分支的
[Releases](https://github.com/bailangvvkruner/meatshell/releases) 页面。

### Windows

下载 `meatshell-*-windows-x86_64.zip`，解压后双击 `meatshell.exe`；也可以使用
同一 Release 中的 `.msi`。ZIP 内的 `libEGL.dll`、`libGLESv2.dll` 和
`ANGLE_LICENSE.txt` 是可选 GPU 模式所需文件，请与可执行文件放在一起。

### Linux

```bash
tar -xzf meatshell-*-linux-x86_64.tar.gz
cd meatshell-*-linux-x86_64
./meatshell                                  # 直接运行
# 可选：装应用图标 + 启动器入口（Dock / 应用列表里显示图标，无需传参）
chmod +x install-linux.sh && ./install-linux.sh
```

> 需要 glibc ≥ 2.35（Ubuntu 22.04+ / Debian 12+）。Wayland 下首次装完图标可能要注销重登一次。

从源码 `cargo run`（Linux Mint / Ubuntu / Debian）需要先安装 Slint/winit/rfd 等用到的系统开发包：

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

下载得到的是 `.zip`，里面是 `meatshell.app` 应用程序包：

```bash
# 解压(aarch64 = Apple 芯片，x86_64 = Intel)
unzip meatshell-*-macos-*.zip
# 移到「应用程序」(可选，留在原地也行)
mv meatshell.app /Applications/
# 去掉「未签名应用」的隔离属性，否则会提示「meatshell 已损坏，无法打开」
xattr -dr com.apple.quarantine /Applications/meatshell.app
# 打开(或在「访达」里双击)
open /Applications/meatshell.app
```

> 若未移到 `/Applications`，把上面两条路径换成 `.app` 实际所在位置(如 `~/Downloads/meatshell.app`)即可。

> 从源码构建见下方 [运行](#运行)。

## 功能

### 已实现

- [x] FinalShell 风格 UI，深色 / 浅色 / 跟随系统主题
- [x] 本机 + 远端资源监控（CPU / 内存 / 交换 / 网络 / 磁盘），支持独立刷新间隔与远端监控自动恢复
- [x] 远端进程监控（按 CPU 排序、PID 复制与权限确认后结束进程），兼容 GNU 与 BusyBox 输出
- [x] 完整 VT/ANSI 终端模拟（btop / htop / vim 全屏正常渲染），支持增量 UTF-8 与 xterm 鼠标协议
- [x] 稠密终端后台行栅格缓存与普通 shell 文本快路径（[性能与验收说明](docs/terminal-rendering-performance.md)）
- [x] 彩色 emoji（支持肤色、旗帜及 ZWJ 组合序列）
- [x] 多标签页（欢迎页 + 多个会话）
- [x] 会话管理：新建 / 编辑 / 删除 / 分组，本地 JSON 持久化，导出 / 导入
  - 配置位置：`%APPDATA%/meatshell/sessions.json`（Windows）
    / `~/.config/meatshell/sessions.json`（Linux）
    / `~/Library/Application Support/meatshell/sessions.json`（macOS）
- [x] SSH（`russh`，纯 Rust）：密码 / 私钥 / 加密私钥（密码短语），认证失败后可重新输入
- [x] SFTP 文件浏览 + 上传 / 下载（拖拽）+ 终端内 ZMODEM（`sz`）接收
- [x] SSH 端口转发 / 隧道：本地 -L / 远程 -R / 动态 -D（SOCKS5）
- [x] 快捷命令 + 命令输入框（可群发到所有会话）+ 命令历史
- [x] 串口 / Telnet 会话
- [x] 出站代理（SOCKS5 / HTTP）
- [x] 导入 `~/.ssh/config`
- [x] 会话密码加密存储（ChaCha20-Poly1305）
- [x] 已知主机（`known_hosts`）校验 + 首次连接确认
- [x] 多标签页终端分屏
- [x] 可选的回环 Debug API（Bearer 鉴权、终端读取/输入/鼠标与窗口截图）

彩色 emoji 图形来自 [Twemoji](https://github.com/jdecked/twemoji)，按
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) 使用；完整署名见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

### 计划中

- [ ] 会话密码改用 OS 钥匙串存储

## 技术栈

| 模块          | 选型                                                              |
| ------------- | ----------------------------------------------------------------- |
| UI            | [Slint](https://slint.dev)（纯 Rust 编译，无 GC）                 |
| 异步运行时    | [`tokio`](https://tokio.rs)                                       |
| SSH 协议      | [`russh`](https://crates.io/crates/russh)（无 libssh 依赖）       |
| 系统指标      | [`sysinfo`](https://crates.io/crates/sysinfo)                     |
| 行栅格        | [`cosmic-text`](https://crates.io/crates/cosmic-text)             |
| Debug API     | [`axum`](https://crates.io/crates/axum)                           |
| 序列化        | `serde` + `serde_json`                                            |
| 日志          | `tracing` + `tracing-subscriber`                                  |

## 运行

```bash
cargo run --release
```

首次启动会在 `%APPDATA%/meatshell/sessions.json` 建立空的会话库。点击右上
角 **“＋ 新建会话”** 添加第一台服务器。

## 项目布局

```
meatshell/
├── Cargo.toml
├── build.rs                 # Slint 编译器入口
├── ui/
│   ├── app.slint            # 顶层窗口
│   ├── theme.slint          # 设计 tokens
│   ├── widgets.slint        # 可复用按钮 / 输入框 / sparkline
│   ├── sidebar.slint        # 左侧系统监控面板
│   ├── welcome.slint        # 欢迎页 / 快速连接
│   ├── session_dialog.slint # 新建 / 编辑会话弹框
│   └── terminal_view.slint  # 文本与行图像终端视图
└── src/
    ├── main.rs              # 进程入口与运行时
    ├── app.rs + app/        # UI ↔ 后端桥接及回调模块
    ├── config/              # 会话、设置与加密持久化
    ├── ssh/ + sftp/         # SSH、认证、监控与文件传输
    ├── terminal/            # VT 状态、输入、鼠标与行栅格
    ├── debug_api.rs         # 回环 HTTP Debug API
    └── memory_trim.rs       # Windows 空闲内存回收
```

## 开发提示

- Slint 控件有非常严格的布局 DSL，改 `.slint` 后 `cargo check` 是最快的
  反馈方式。
- 应用事件循环是单线程（Slint 要求），所有跨线程 UI 更新通过
  `slint::invoke_from_event_loop` 回调。
- SSH / SFTP 共享 `known_hosts` 校验逻辑：首次连接会确认并记住主机密钥，
  后续密钥变化会再次提示。
- 本机 Debug API 的接口、限制和 PowerShell 示例见
  [docs/debug-api.md](docs/debug-api.md)。
- 终端行缓存的边界与回归测试要求见
  [docs/terminal-rendering-performance.md](docs/terminal-rendering-performance.md)，
  依赖审计例外见 [docs/security-audit.md](docs/security-audit.md)。

## 发版

不要直接手动修改 `Cargo.toml` 后再打标签。使用发布脚本，让 Git tag 指向的提交本身就已经包含正确版本号：

```powershell
.\scripts\release.ps1 v0.6.0 -Push
```

脚本会更新 `Cargo.toml` / `Cargo.lock`，运行 `cargo check --locked`，验证 `meatshell --version`，提交 `Release v0.6.0`，创建 annotated tag，并推送当前分支和 tag。更多细节见 [docs/release.md](docs/release.md)。

## License

MIT OR Apache-2.0（双许可）。
