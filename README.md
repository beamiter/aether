# Tauri 2 自研终端垂直原型（本地实验）

**当前不是可用终端。没有 xterm.js，也没有 WebView ANSI 解释器。**

## v19：原生 Tauri 调度与真实 PTY 命令联测（2026-11-14 UTC）

14 个 Tauri 命令改为异步 handler + blocking pool，backend 锁、PTY 创建及解析不再占用窗口事件线程。新增 Tauri mock-window 命令路由/真实 PTY 集成测试；旧 v18 handler 在锁竞争回归中阻塞约 3 秒并失败，修复后通过。它不是真实 WebKit 窗口测试。

本次实际恢复并通过：Rust session 57、bridge 8、Tauri IPC 2、Node 60，共 **127 项**；三层严格 Clippy、原生 Linux debug 构建及真实 Vim PTY smoke 通过。核心仍固定 `2b37583`。Xvfb 可启动，但 Tauri 窗口被发行版 WebKit 的绝对子进程路径缺失阻断；GUI/像素/Wayland/平台 IME 仍未验收。详见 `NATIVE-IPC-CHECKPOINT.md`。本节是当前状态，下方旧记录保留为历史。

## v18：历史分页有界准入（2026-11-14 UTC）

历史按钮请求现在最多一个在途和一个合并后的最新 offset；快速连续翻页不会生成无限 IPC，也不会用过时页面干扰正在选择的快照。错误保留现有文本并丢弃待处理请求，只允许显式重试；跨 session 隔离保持。Node **60/60** 通过，突发 200 次测试已证明修复前失败。Rust/pin/权限未改；不是原生 GUI 或真实性能验收。详见 `HISTORY-ADMISSION-CHECKPOINT.md`。

## v17：恢复请求有界准入（2026-11-14 UTC）

连续点击恢复输出只允许同一 session epoch 一次恢复 IPC 在途，包含 ACK 和后续 poll。繁忙时忽略额外点击，不排队或自动重试；旧 session 卡住不妨碍新 session，旧回调不能解除新请求的保护。新增突发点击、显式重试和跨 session 回归；Node **56/56** 通过。Rust/pin/权限未改，尚未原生 GUI 重验。详见 `RECOVERY-ADMISSION-CHECKPOINT.md`。

## v16：异步写入安全锁（2026-11-14 UTC）

修复 Send line 等待写入时收到 shell 退出、writer 错误或 poll 失败后，被成功写入回调重新启用输入的竞态；部分写入后若健康锁已置位，不再继续发送后缀。三个新增回归在 v15 均失败，修复后完整 Node **53/53** 通过。Rust/pin/权限未改；这只是模拟 IPC 生命周期回归，不是原生 GUI、真实 shell 竞态或 IME 验收。详见 `INPUT-LOCK-CHECKPOINT.md`。

## v15：多行粘贴审阅（2026-11-14 UTC）

第五端现在对 LF/CR/CRLF clipboard paste 先展示只读预览，只有点击“Send reviewed paste”才向当前 shell 排队发送；取消、关闭、退出、写入/传输失败和 session epoch 变化都会丢弃旧审阅。4096 UTF-8 字节上限与不支持的控制字符在进入队列前检查。单行仍直接发送，Rust 仍独占粘贴协议编码；无新增 JS ANSI 状态机。审阅不承诺命令安全，也不保证接收端处于 bracketed-paste 模式。

本次 `npm test` **50/50** 通过，其中新增 9 项。没有修改 Rust、核心 pin、锁文件或 Tauri 权限。当前执行环境没有 Cargo，本次未重跑 Rust、原生构建、真实 PTY/Vim 或 GUI/IME；历史原生通过记录不代表此次 GUI 验收。审阅只覆盖实验 UI 的 clipboard 路径，并不是后端 IPC 授权机制，显式 Send line/键盘 Enter 保持原行为。详见 `PASTE-REVIEW-CHECKPOINT.md`。

## 最新验证补充：shell metadata 安全回退契约

新增 4 项第五端 OSC 7/133/私有元数据回退测试，覆盖分片、取消、超限和恢复；完整 session 57 项及严格 Clippy 通过。保持“不信任任意 PTY 元数据”的现有行为，没有新增 command blocks 或生产行为。详见 `SHELL-INTEGRATION-CONTRACT.md`。这不是五端协议或 GUI 验收。

## 最新验证补充：原生构建与真实 Vim（2026-11-13 UTC）

本节覆盖下方 v12 中“尚未原生构建/真实 Vim 重验”的旧状态。保持核心 pin 2b37583，Linux `cargo build --locked` 已成功；真实 Vim PTY 烟测已通过（含中文提交、resize、编辑/撤销、准确保存与主屏恢复）。临时/Cargo缓存改放有足够空间的工作区磁盘，未改系统安全配置。完整记录见 `NATIVE-BOUNDED-VALIDATION.md`。

**原生 GUI、像素、X11/Wayland 输入和平台 IME 仍未验收**：当前环境 AF_UNIX socket 在普通及受支持升级执行中均 EPERM。编译通过和 PTY 烟测不等于可日用终端。

## 最新 checkpoint：bounded core 接入（2026-11-13 UTC）

本节优先于下方历史 checkpoint。核心精确固定为 `2b37583ab1a833176e88145e67243146f99c309a`，采用 `Parser::with_bounded_profile` 和 `EmberV1`，pending/Kitty recovery 各 4096 字节。session、bridge、native 三份锁文件一致；未盲目跟进之后的核心提交。

实际通过：51 项 session 单测、2 项真实 PTY 集成、8 项 bridge 测试、41 项 Node 测试，共 102 项；session/bridge 严格 Clippy 通过。新增回归覆盖五类字符串超限、分片边界、CAN/SUB、OSC BEL、连续 1 MiB 超限恢复与后续新查询。超限后的嵌套控制不得执行。

**未完成：此 pin 的原生桌面编译/GUI、X11/Wayland 像素、真实 Vim 重验。** 本次 native 依赖解析在刷新锁后因 `/tmp` 空间耗尽终止；不是构建通过。下方旧版 native 编译/真实 Vim 记录不代表此版本通过。没有 Kitty 图片 assembler，不能将解析取消回归称为图片分块宿主验收；超限前 fresh ESC 仍遵循核心重启规则。

可恢复：保留 Library 原文件身份，前一版本 v11 为迁移前 checkpoint；不覆盖其历史，不建立远程仓库。详见 `BOUNDED-CORE-CHECKPOINT.md`。

## 历史初始实现（以下记录保留）

- `bridge`：真实 `jterm_engine@57ed89a` 的 `TerminalGrid` / `CellFrame` 生成演示帧，序列化 Rust→Web 格式。
- 每次只允许一帧在途，ACK 必须匹配 session / epoch / revision，重复或越权 ACK 被拒绝。
- `web`：Canvas 逐格绘制，原子校验帧身份/版本/边界/字符。不使用 innerHTML。
- bridge 3 项 + session 4 项 Rust 测试及严格 Clippy 通过；7 项 Node 合约/绘制/异步生命周期测试通过。
- `session`：Linux 固定 `/bin/sh -i` 真 PTY、共享 Rust parser/screen/semantics、非阻塞有界读写/回复、基础输入/resize/退出。
- 真实固定 `printf` → PTY → Rust `CellFrame` → JSON → JS 验证 → Canvas 绘制调用的联测通过（不是像素验收）。
- `web/demo-frame.json` 是 Rust 可执行程序生成的数据，不是手工伪造 engine 输出。

## 已准备但尚未原生验收

- `src-tauri`：Tauri 2 桌面壳、两个受控演示命令、CSP、本地前端资源，不开通 shell 插件或远程页面。
- 本云环境缺 GTK3 / WebKitGTK 4.1 / JavaScriptCoreGTK 4.1，未成功构建/启动原生窗口。
- 浏览器工具与本地 HTTP 服务连接被拒绝，故未宣称视觉 QA 通过。

## 本地验证

```sh
cargo test --locked --manifest-path bridge/Cargo.toml
cargo clippy --locked --manifest-path bridge/Cargo.toml --all-targets -- -D warnings
cargo run --locked --quiet --manifest-path bridge/Cargo.toml > web/demo-frame.json
npm test
# 纯浏览器仅可看固定 Rust 帧，不可启动 shell
python3 -m http.server 8765 --directory web
# 已安装当前平台 Tauri prerequisites 后
npm ci
npm run tauri -- dev
```

## 边界与缺口

固定演示帧与受限真 PTY 两种模式。PTY `TERM=dumb` 支持基础文本/CR/LF/BS/SGR/resize，尚无 Unicode 正确 Canvas 宽格/组合/字形、IME、选择、scrollback、鼠标、完整 Vim 兼容、图形和断线重同步。颜色已映射基本/索引/RGB，但未绘制全部 style。只回收直接 shell，尚无生产级进程树隔离，勿用于后台任务。ACK 只验证基本单在途演示，**不等于生产 IPC 字节预算或端到端背压已完成**。

`session` 已作为独立实验复用同一个 Rust parser / ScreenState / semantic dispatcher，接真实 PTY。它明确有限语义支持，不替代 Ember 完整行为，不是第二套 JS VT。完整日用前必须收敛现有 headless session 适配、权限、进程回收、resize、完整/full-delta resync、Unicode/IME 和真实 Vim 验收。

## 最短里程碑

1. **帧桥（当前）**：确认 Rust→Canvas 单向数据和 ACK 合约。
2. **受限交互（后端联测已过，原生 GUI 待验收）**：固定本地 shell，PTY读写/resize/关闭，Rust唯一解析状态；明确 unsupported。
3. **Vim-ready**：接齐现有 engine/host 语义，主副屏、终端查询、组合字符、鼠标、快捷键。
4. **日用 Alpha**：IME、选择/复制、滚动、性能预算、重连、权限、安全退出与打包。

不需要等待 Forge/Anvil 完全替换 VTE；但不可将帧骨架或受限 PTY demo 宣称为完整第五端。


## Real Vim backend smoke

`session/examples/real_vim.rs` drives an actual official Vim executable over the PTY (not Ex/batch mode): alternate screen, insertion, resize, multi-line scrolling, delete/undo, `:wq`, exact saved-file comparison, and primary-screen restoration. The test-only Vim command explicitly uses `TERM=xterm`; the product shell remains `TERM=dumb` and the UI now forwards focused logical key/paste events to Rust (native event behavior is not yet verified). Passing this smoke is neither full Vim compatibility nor desktop GUI acceptance.

```sh
cargo run --manifest-path session/Cargo.toml --example real_vim -- /absolute/path/to/official/vim
```


## Experimental keyboard path

Focus the Canvas to send logical key events. Rust owns DECCKM/modifyOtherKeys encoding and bracketed-paste markers; JavaScript never constructs terminal escape sequences. Host input queue admits at most 16 KiB, preserves partial-write suffixes and rejects overflow atomically. UI keeps at most 128 pending logical events and locks on failure rather than replaying uncertain input. Composition preedit and dead-key keydowns are withheld; a dedicated textarea forwards committed text once. Native WebKit IME/candidate positioning remains unverified. Clipboard control characters are rejected by Rust; unbracketed pasted newlines can execute shell commands.


## Composition checkpoint

The editable keyboard area receives desktop composition events. Preedit stays in the browser; only committed UTF-8 passes through Rust `encode_text`, with control-character rejection and a 4096-byte bound. Tests cover final input before/after compositionend, cancellation, stale epochs, duplicate prevention and a subsequent identical intentional key. Real Vim exact saved-file testing includes Chinese committed text. This validates the event state machine and backend, not an actual platform IME or candidate-window positioning; mobile event sequences and arbitrary WebKit ordering remain unverified. Candidate UI is anchored to the textarea, not the terminal cursor.

History/copy: Refresh history shows an immutable primary scrollback page, with
Older/Newer controls and manual selection/copy in a readonly textarea. It is not
Canvas drag-selection or scrollback reflow. Snapshot visible text copies the current
grid into that textarea using visual row separators. History is capped at 1024 rows;
new output does not mutate a snapshot you are selecting.

Recover output snapshot repairs lost delivery/ACK by requesting a fresh full frame
without restarting the shell or replaying input. It intentionally does not unlock
input after an uncertain/partial write. Native GUI recovery interaction is unverified.

Unicode/rendering: wire v2 preserves wide-cell spans and paints foreground only
after all backgrounds. Basic bold/italic/dim/underline/strike and RGB/256 colors
are included. Combining/ZWJ graphemes remain unsupported and are diagnosed; complex
underline styles fall back to single, blink is not animated. No GUI font/shaping
acceptance is claimed.

Cursor/size controls: use Columns/Rows and Apply size; rapid requests coalesce to
the latest target. Show cursor scrolls the live grid without moving the terminal
cursor. Block/beam/underline and wrap-pending come from Rust. No cursor animation
or native GUI acceptance is claimed; history snapshots remain separate from the
scrollable live canvas.
