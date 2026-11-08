# sommelier-rs-r 架构

本文描述程序的结构。功能完成度见
[`FEATURE_MATRIX.md`](FEATURE_MATRIX.md)，与主线 C++ `sommelier` 的逐项对照见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md)，安装与使用见
[`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md)。

## 1. 角色与边界

`Sommelier-rs-r` 是一个**双向 Wayland 消息代理**，同时充当 guest 的
"compositor" 与 host compositor 的 "client"：

```mermaid
flowchart LR
    G[guest Wayland / X11 应用] -->|Unix socket $XDG_RUNTIME_DIR/&lt;display&gt;| P[sommelier-rs-r]
    P -->|virtio_wl /dev/wl0| H[ChromeOS host compositor]
    P -->|virtio-gpu cross-domain| H
    P -->|本地 compositor socket（调试）| H
    P -.->|-wm XCB 连接| X[rootless Xwayland]
    X -->|WAYLAND_SOCKET 预连接| P
```

它不是 X11 字节流代理：X11 侧由真正的 Xwayland + 本进程内的 X11 WM 处理，
最终把 XID 对应到 host 侧的 `wl_surface` / `xdg_toplevel`。

## 2. 进程与线程模型

```
process
 ├─ tokio current_thread runtime                 (main.rs)
 │   ├─ listener task：accept 后 spawn_client
 │   ├─ one task per guest client                (proxy.rs::Client::run)
 │   │   └─ tokio::select! { client.recv, host.recv, wm_commands, 50ms tick }
 │   ├─ clipboard pipe pump / prefetch tasks     (tokio::spawn + blocking pool)
 │   └─ child-process reaper + sd_notify
 └─ dedicated OS thread                          (x11::wm::run)
     └─ X11 event loop (poll_for_event + flush, 2 ms)
```

- **必须用 `current_thread`**：`KeyboardHandler` 内的 `xkbcommon` 对象
  `!Send`，依赖 `unsafe impl Send`。改成多线程 runtime 会破坏该不变量。
- **X11 WM 独立 OS 线程**：`x11rb::RustConnection` 同步轮询，不占用 tokio
  runtime；通过 `X11Bridge` 的 `WmCommand` / `WmEvent` 无界通道与代理通信。
- 剪贴板管道读写用 `tokio::fs::File`（blocking pool），不会卡住事件循环。

主线程与 WM 线程之间**没有共享可变 Wayland 状态**：`Context` 属于单个
`Client` task；跨线程只走 `Arc<X11Bridge>`。剪贴板状态
`Context::selection` 是 `Arc<Mutex<SelectionBridge>>`，仅为让 handler 能同时
持有 `&mut Context`。

## 3. 三条 host 传输

`connection.rs::ConnectionTransport` 有三个变体，由
`config.rs::resolve_transport()` 选择（默认 `auto`：先 `/dev/wl0`，再
virtio-gpu render node，最后本地 socket）：

| 变体 | 设备 | 备注 |
|---|---|---|
| `VirtWayland` | `/dev/wl0` | Crostini 唯一可靠路径；`VirtWaylandChannel` 提供 `NEW_ALLOC` / `NEW_PIPE_READ` |
| `VirtGpu` | `/dev/dri/renderD*` | cross-domain blob + fence，`main` 分支能力 |
| `Unix` | `$WAYLAND_DISPLAY` | 调试 / 非 Crostini VMM |

`--force-drm-device` 同时作用于 `VirtGpuChannel::new_with_device()` 与
`Allocator::with_device()`。

## 4. 每个 guest client 一条独立 host 连接

```
Client {
  client_conn: WaylandConnection,   // guest socket
  host_conn:   WaylandConnection,   // 独立 host 连接
  ctx:         Context,             // ShadowTable + 各种 handler 状态
  handler:     SommelierHandler,    // 聚合 delegation
  wm_commands, x11, toplevels
}
```

`handle_msgs()` 逐条解析 `WireMessage`，按方向分发：

- `dispatch_request` → `protocols::all::dispatch_request()`；
- `dispatch_event` → `protocols::all::dispatch_event()`；
- `DispatchResult::UnknownOpcode` 只丢单条消息，其余 `ProtocolError` 断连；
- 两个反向队列 `host_to_client_queue` / `client_to_host_queue` 让 handler
  可以"回插"消息（合成事件、内部 bind、ack-key、剪贴板请求）；
- EOF 判定为 `bytes == 0 && read_fds.is_empty()`，避免 SCM_RIGHTS 落后于数据时误断连。

每轮循环末尾 `poll_selection()` 驱动剪贴板状态机并 flush 合成消息；
50 ms 定时器保证 X11 侧变化在 guest 空闲时也能生效。

额外有一条捷径：`Context::is_internal_selection_id()` 命中的 host→client
事件（来自内部创建的 `wl_data_source` / `wl_data_device` / manager）不进入
聚合分发，而是由 `handle_internal_selection_event()` 直接消费。这些对象只存在
于 host 命名空间，没有 guest id 可供重写 sender，常规转发会污染 guest。

## 5. 对象 ID 与 codegen

`ShadowTable` 维护四组映射：

| 映射 | 用途 |
|---|---|
| `guest_to_host` / `host_to_guest` | 普通对象 id 双向翻译 |
| `interfaces` / `host_interfaces` | 按对象 id 查 interface 名，供聚合分发用 |
| `next_host_id` | 代理在 host 侧创建的 new_id |
| `next_guest_server_id` (`0xff000000+`) | host 事件中 `new_id` 在 guest namespace 的新 id |

**关键正确性点**：host→client 事件里的 `new_id` 是 host namespace 的 id，
绝不能原样交给 guest。`generator.rs` 对所有事件 `new_id` 统一调用
`allocate_guest_server_id()`；`handler/data_device.rs` 保留 `data_offer` 的
手写路径作为互补。

构建流程：

```
third_party/protocols/*.xml (33)
   └─ sommelier/build.rs 扫描目录
        ├─ wayland_codegen::generate()          → <module>_protocol.rs
        ├─ wayland_codegen::aggregate::generate → aggregate_protocol.rs
        └─ all_includes.rs                      → include! 进 mod protocols
```

`protocols::all` 提供 `KNOWN_INTERFACES`、`is_known()`、
`dispatch_request()` / `dispatch_event()` 与 `impl_generated_delegates!`。
新增协议只需放入一个 XML，`proxy.rs` / `registry.rs` 无需改动。

16 个协议有手写 delegation（见 `proxy.rs` 的
`impl_sommelier_delegates!` / `ProtocolHandler` 挂载），其余由聚合宏生成纯转发；
`aggregate.rs::EXPLICIT_DELEGATES` 列出这 16 个模块以避免生成冲突实现。
10 个纯内部绑定模块在 `INTERNAL_ONLY` 中，其 host event 在聚合分发层直接丢弃。
唯一的例外是 `INTERNAL_EVENT_EXCEPTIONS = ["zaura_output"]`：它的
`scale` / `connection` / `device_scale_factor` 必须交给 `OutputHandler` 本地消费，
否则输出 DPI 与缩放修正无法生效。
`text_input_unstable_v1` / `text_input_extension_unstable_v1` **刻意不屏蔽**：
它们的 host event 由 `handler/text_input.rs` 转成 guest 的 text-input-v3 事件。

可观测性由 `stats.rs` 独立承担：全进程共享一个 `Stats`，`proxy::run()` 起单个
定时采样任务，`Client::run()` 结束时写入最终样本。`--timing-filename` /
`--stats-log` 输出 CSV、`--stats-summary` 输出实时快照、
`--trace-filename` 逐消息记录方向/接口/opcode/长度。连接建立时
`proxy::so_peercred_pid()` 用 `SO_PEERCRED` 取 client pid（显式 `--peer-pid`
优先），注入 `Context.client_pid`。

## 6. X11 子系统

```
x11/xwayland.rs   WAYLAND_SOCKET pair 由 main 提前创建 → socketpair(displayfd / wm)
                  → 独立 OS 线程 spawn Xwayland（见 §9 启动顺序）
x11/wm.rs         SubstructureRedirect + Composite RedirectManual
                  ICCCM/EWMH 属性、WM_S0、CLIPBOARD_MANAGER
                  WL_SURFACE_ID 配对 → WmCommand::CreateToplevel
x11/atoms.rs      48 个 atom
x11/clipboard.rs  X11 selection / TARGETS / INCR / 延迟 SelectionRequest
x11/selection.rs  Wayland 侧剪贴板状态机
x11/bridge.rs     跨线程共享状态与两条通道
```

`proxy.rs::create_toplevel()` 在 **Xwayland 的 host 连接**上合成
`xdg_surface` / `xdg_toplevel` / `zaura_surface` 并 `set_application_id`；
host 的 `configure` / `close` 事件再经 `WmEvent` 转成 X11
`ConfigureNotify` / `WM_DELETE_WINDOW`。

## 7. 剪贴板三向桥接

与主线 C++ `sommelier` 的逐行对照见 [`CLIPBOARD.md`](CLIPBOARD.md)。

```mermaid
sequenceDiagram
    participant X11 as X11 应用
    participant WM as wm.rs / clipboard.rs
    participant B as X11Bridge
    participant S as selection.rs (Xwayland 连接)
    participant H as ChromeOS host compositor
    participant G as guest Wayland 应用

    Note over X11,B: X11 → ChromeOS host / guest Wayland
    X11->>WM: XFixes SelectionNotify (owner 改变)
    WM->>WM: ConvertSelection(TARGETS)
    WM->>B: set_clipboard_targets(names)
    S->>B: poll() 发现 generation 变化
    S->>H: create_data_source / offer(mime)* / set_selection
    S->>G: data_offer + offer(mimes) + selection
    H->>S: wl_data_source.send(mime, fd)
    G->>S: wl_data_offer.receive(mime, fd)
    S->>B: WmEvent::RequestClipboardData
    B->>WM: ConvertSelection(atom(mime))
    WM->>B: set_clipboard_data(atom, bytes)
    S->>H: 异步写回 host fd
    S->>G: 异步写回 guest fd

    Note over G,X11: host / guest Wayland → X11
    G->>S: set_selection / offer(mime)
    H->>S: data_offer / offer(mime) / selection
    S->>B: WmEvent::ClipboardTargets / ClipboardData
    WM->>X11: set_selection_owner(CLIPBOARD) + SelectionNotify
```

设计要点：

- 只有 Xwayland 连接（`Context::is_xwayland_client`）创建内部 host
  `wl_data_device_manager` / `wl_data_device` / `wl_data_source`。host 的
  selection 是 per-seat 的，一个 exporter 足够服务整个 VM；内部 manager 由
  `registry.rs` 绑定，host seat id 从 guest 的 `get_data_device` 请求反查。
- `Context::internal_selection_ids` 让 `proxy.rs::handle_msgs()` 直接消费这些
  host 对象的事件（只处理 `wl_data_source.send`），绝不把 host 命名空间的 id
  转发给 guest；`on_host_selection()` 再过滤一次 selection 回声，避免 WM
  抢走刚复制的 X11 应用的 owner。退役的旧 source 仍留在集合中，迟到事件也会被吞掉。
- 多 client 各自持有 `SelectionBridge`，因此 TARGETS 用
  `(generation, snapshot)` 广播式读取、数据用按 target 的 cache，避免
  `take()` 造成只有一个连接拿到结果。owner 变化清空 cache 与 pending
  transfer；每次 `send` 前再对单个 target 做 `invalidate_clipboard_data()`。
- `wl_data_device.set_selection` 的 serial 由
  `proxy.rs::maybe_update_host_serial()` 从 host 输入事件首字段提取
  （主线用 `ctx->default_seat->seat->last_serial`）。

## 8. 缩放与输出

`transform.rs` 实现主线三坐标系（Physical / Virtual / Host Logical）、
`wl_fixed` 24.8 定点运算、DPI 桶与三种 scaling mode。`handler/output.rs`
拦截 `wl_output.geometry/mode/scale` 与
`wp_fractional_scale_v1.preferred_scale`，按虚拟坐标系重写，并把输出布局
同步回 `Transform` 与 `X11Bridge`（供 RandR 模拟）。

`create_output_extensions()` 在 `wl_output` bind 之后内部创建 `zaura_output`，
消费 `scale` / `connection` / `device_scale_factor`（线值 1/1000）；DSF 到达时
按主线 display-ready 语义重发一次完整 output state。`--direct-scale` 还会内部
bind `zxdg_output_manager_v1`，把 `logical_size` / `logical_position` 本地消费
后喂给 `Transform::axis_factors()`；两者都**不向 guest 公布**，
`aggregate.rs::INTERNAL_EVENT_EXCEPTIONS` 保证这些内部对象的 host 事件不被聚合层
提前丢弃。`device_scale_factor` 缺省 1.0（主线 `sommelier-output.cc:649`），
不再用整数 `wl_output.scale` 兜底。

`--no-support-damage-buffer` 把 `wl_compositor` 钳制到 v3，并把违规
`damage_buffer` 请求改写成缩放后的 `wl_surface.damage`。

## 9. 配置

`config.rs` 用 clap 统一主线 C++ `sommelier` 与两个上游分支的 CLI，并支持
`SOMMELIER_*` 环境变量（命令行优先）。与主线一致：`--socket` 是 guest 监听
socket 名（默认 `wayland-0`），`--display` / `SOMMELIER_DISPLAY` 是要连接的
host Wayland display，第一个位置参数起为 `[program] [args...]`。
`main.rs` 负责 X11 条件启动（失败降级为 Wayland-only）、
`runprog` 子进程与 `exit_with_child`、`sd_notify READY=1`、
`--print-enabled-features`。

主线把 `SOMMELIER_GLAMOR`、`SOMMELIER_CLIPBOARD_MANAGER`、
`SOMMELIER_SUPPORT_DAMAGE_BUFFER` 当**字符串**处理（例如只有 `"0"` 才禁用
glamor），真机 systemd 注入的是 `"1"`。这些变量一律由
`Config::from_args` 手动读取，**不绑定 clap 的 bool flag**，否则 clap 会以
`invalid value '1' ... [possible values: true, false]` 拒绝启动。

**X11 启动顺序**：Xwayland 必须先通过 `WAYLAND_SOCKET` 完成 Wayland 握手，
才会往 `-displayfd` 写 display 号。因此 `main` 提前建好 socketpair，把
`wl_parent` 立即交给 `proxy::run()`（开头即启动专用 Xwayland client），
再把 `wl_child` 连同 `start_x11()` 放到独立 OS 线程（Tokio 是
`current_thread`，不能阻塞 main task）。该线程在 `DISPLAY` / `XCURSOR_SIZE`
设置完成后才启动 `runprog`。现役主线用单个 event loop 达到同样效果。

## 10. 构建与验证

```bash
cargo check --target x86_64-unknown-linux-gnu --workspace
cargo check --tests --target x86_64-unknown-linux-gnu --workspace
cargo fmt --all -- --check
# Linux 主机 / CI：
cargo test --workspace
```

Debian 13 CI 覆盖 x86_64 / aarch64 构建、x86_64 `cargo test --workspace`、
全目标 `cargo check --tests` 与格式检查；此外已在 Crostini 真机跑通 Wayland
与 X11 端到端，并可由 `packaging/build-deb.sh` 构建 `.deb`（见
`CROSTINI_INSTALL.md`）。剩余限制见 `FEATURE_MATRIX.md` §6。
