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
| 1 | Windows x64 夜间构建 + 滚动 `nightly` 预发布 | `945ed26` | `.github/workflows/release.yml`（新增 `nightly-windows` job） | 进行中 |
| 2 | 资源监控刷新频率可配置 | `94fe429`、`551c660` | `src/config/struct/config_file.rs`、`src/resource/*`、`src/app/resource_ui.rs`、`ui/interface_panel.slint` | 待做 |
| 3 | 剪贴板免密 SSH 快速连接 | `979f83c` | `src/app/clipboard_connect.rs`（新增）、`src/app/tab_callbacks.rs`、`src/ssh/impls/ssh.rs`、`ui/app.slint` | 待做 |
| 4 | Debug HTTP API（127.0.0.1:24817，Bearer token） | `ed462f0`（部分） | `src/debug_api.rs`（新增，1745 行）、`src/main.rs`、`src/app.rs`、配置项、`ui/app.slint` 设置页、`docs/debug-api.md` | 待做 |

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
