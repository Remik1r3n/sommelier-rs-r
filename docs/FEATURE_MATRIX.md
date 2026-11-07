# 功能矩阵与交付状态

本文对照主线 C++ `sommelier`（`vm_tools/sommelier/`）与 `sommelier-rs` 两个分支，
逐项记录 `sommelier-rs-r` 的实现状态。

状态含义：

- ✅ **已实现**：代码存在并接入运行时（编译通过，有对应测试或明确的调用点）
- ⚠️ **部分实现**：框架/解析/转发存在，但关键环节未接线或做了简化
- ❌ **未实现**：本次未覆盖

## 1. 传输与进程模型

| 能力 | 主线 C++ | `main` | `virtwl` | **本项目** | 备注 |
|---|---|---|---|---|---|
| virtio_wl `/dev/wl0` | ✅ | ❌ | ✅ | ✅ | Crostini 唯一可用路径 |
| virtio-gpu cross-domain | ✅ | ✅ | ❌ | ✅ | 从 `main` 恢复 `virtgpu_channel.rs` |
| 本地 compositor socket | ✅ | ✅ | ✅ | ✅ | 调试用 |
| 自动选择 / 运行时降级 | ✅ | ❌ | ❌ | ✅ | `--transport=auto` + `validate_transport()` |
| `--force-drm-device` | ✅ | ❌ | ❌ | ✅ | 同时作用于 virtgpu 与 GBM allocator |
| Parent / per-client worker | ✅ | ❌ | ❌ | ⚠️ | 用 per-client tokio task 代替 fork；`--parent` 仅为语义标记 |
| `--exit-with-child` / `runprog` | ✅ | ❌ | ❌ | ✅ | 子进程 reap + 可选随子进程退出 |
| sd_notify (`READY=1`) | ✅ | ❌ | ⚠️ | ✅ | 在 socket bind 之后发送 |
| seccomp / sandbox | ✅ | ❌ | ❌ | ❌ | 未实现 |

## 2. X11 / XWayland（本次新增主体）

| 能力 | 状态 | 实现位置 |
|---|---|---|
| 启动 rootless Xwayland（`-displayfd` / `-wm` / `-rootless` / `-nolisten tcp`） | ✅ | `x11/xwayland.rs::start_x11` |
| `WAYLAND_SOCKET` 预连接 + 专用 Xwayland client | ✅ | `xwayland.rs` + `proxy.rs::run` |
| `-shm` / `-auth` / `-fp` / `LIBGL_DRIVERS_PATH` / `SOMMELIER_VERSION` | ✅ | `x11/xwayland.rs` |
| X11 WM 连接（`x11rb::RustConnection`） | ✅ | `x11/wm.rs::run` |
| `SubstructureRedirect` / `Composite RedirectManual` / XFixes | ✅ | `wm.rs` 初始化 |
| ICCCM（`WM_NAME`, `WM_CLASS`, `WM_NORMAL_HINTS`, `WM_DELETE_WINDOW`, `_MOTIF_WM_HINTS`） | ✅ | `wm.rs` 属性解析 |
| EWMH（`_NET_WM_NAME/STATE/PID/WINDOW_TYPE`, `_NET_SUPPORTED`, `_NET_SUPPORTING_WM_CHECK`） | ✅ | `wm.rs` + `atoms.rs`（48 个 atom） |
| `WM_S0` / `CLIPBOARD_MANAGER` selection owner | ✅ | `wm.rs` |
| `WL_SURFACE_ID` → guest `wl_surface` → host `wl_surface` 配对 | ✅ | `wm.rs` + `bridge.rs` + `proxy.rs::publish_x11_surfaces` |
| 合成 host `xdg_surface` / `xdg_toplevel` / `zaura_surface` + app-id | ✅ | `proxy.rs::create_toplevel` |
| X11 窗口状态 → `set_title` / `set_min_size` / `set_max_size` / `fullscreen` / `maximized` / `minimized` / `draw_attention` | ✅ | `proxy.rs::update_toplevel` |
| host `xdg_toplevel.configure` / `close` → X11 ConfigureNotify / WM_DELETE_WINDOW | ✅ | `proxy.rs::forward_x11_toplevel_event` |
| X11 焦点、激活、move/resize、ShapeNotify | ✅ | `wm.rs` 事件分发 |
| X11 selection：TARGETS / TIMESTAMP / UTF8_STRING / STRING / INCR / XFixes | ⚠️ | `x11/clipboard.rs` 骨架完整，但与 Wayland data-device 的对象桥接未接（见 §6） |
| X11 窗口边框绘制（frame / dark frame color） | ❌ | 仅解析 `--frame-color` / `--dark-frame-color` |
| `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` | ⚠️ | 仅解析 `--allow-xwayland-emulate-screen-pos-size` |
| X11 ↔ Wayland 剪贴板互通 | ⚠️ | 两侧各自可用，桥接未接（见 §6） |

## 3. 缩放 / 坐标 / 输出

| 能力 | 状态 | 实现位置 |
|---|---|---|
| 三坐标系（Physical / Virtual / Host Logical）与 `--scale` | ✅ | `transform.rs` |
| `--direct-scale` / `--stable-scaling`（trunc / ceil / floor 差异） | ✅ | `transform.rs` |
| `--dpi` 桶与 physical mm 换算 | ✅ | `transform.rs::physical_size_for_dpi` |
| `wl_fixed` 24.8 定点变换（无 f32 精度损失） | ✅ | `transform.rs::fixed_mul_frac` |
| `wl_output.geometry/mode/scale/name/description/done` 改写 | ✅ | `handler/output.rs`（本次接入运行时） |
| `zxdg_output_v1.logical_position/logical_size` 改写 | ✅ | `handler/output.rs` |
| `wp_fractional_scale_v1.preferred_scale` 改写 | ✅ | `handler/output.rs` |
| 多输出布局、按位置推断输出、虚拟桌面尺寸 | ✅ | `transform.rs::set_outputs` / `virtual_dimensions` |
| per-surface `wp_viewport` 缩放（非可缩放窗口） | ⚠️ | 参数已解析、`viewporter` 协议转发，但未做 per-surface 缩放注入 |
| `wl_surface.damage_buffer` 支持开关 | ⚠️ | `--no-support-damage-buffer` 已解析，未对 `wl_compositor` 做 version clamp |
| `preferred_scale` 严格等价于主线 `round(wire_scale / scale)` | ⚠️ | 当前量化到 120 的整数倍；`transform.rs` 注释里给出一行替换方案 |

## 4. ChromeOS 专有能力（保留自 `virtwl`）

| 能力 | 状态 |
|---|---|
| `zaura_shell` / `zaura_surface` shelf app-id | ✅ 内部绑定，不向 guest 暴露 |
| `zcr_keyboard_extension_v1` / `zcr_extended_keyboard_v1` 系统快捷键 ack-key | ✅ |
| `SOMMELIER_ACCELERATORS` / `SOMMELIER_WINDOWED_ACCELERATORS` 过滤 | ✅ `accelerator.rs` |
| `zwp_text_input_v1` + `zcr_text_input_extension_v1` ↔ guest `zwp_text_input_v3` | ✅ `handler/text_input.rs` |
| `zcr_text_input_crostini_v1` / `zcr_text_input_x11_v1` | ⚠️ 协议 XML 与生成代码就绪，无手写 handler |
| gaming input / stylus / tablet 扩展 | ✅ 纯转发 |
| `zwp_linux_dmabuf_v1` + GBM / explicit fence | ⚠️ 协议与 GBM 分配可用，`--explicit-fence` 未接入 fence 逻辑 |

## 5. Wayland 代理核心 / codegen

| 能力 | 状态 | 说明 |
|---|---|---|
| 每 guest client 一条独立 host 连接 | ✅ | `proxy.rs::spawn_client` |
| `ShadowTable` guest↔host ID 映射、interface 跟踪、host-only 对象 | ✅ | `state.rs` |
| server-created object（`event new_id`）ID 重映射 | ✅ | `generator.rs` 通用修复（原先是恒等映射） |
| `wl_data_device.data_offer` 手写重映射 | ✅ | `handler/data_device.rs`，与通用路径互补 |
| `wl_data_offer.receive` 跨 VM 管道 + tokio pump | ✅ | `VIRTWL_IOCTL_NEW_PIPE_READ` |
| SHM 四级降级分配 | ✅ | `handler/shm.rs` + `allocator.rs`（sealed memfd 防 SIGBUS） |
| registry global 过滤（deny-list + 内部协议） | ✅ | `handler/registry.rs` + `protocols::all::is_known()` |
| 未知 opcode 只丢单条消息、不杀连接 | ✅ | `proxy.rs::handle_msgs` |
| EOF 判定 `bytes == 0 && read_fds.is_empty()` | ✅ | 从 `main` 回补 |
| 方向分离的 sender id | ✅ | `last_guest_sender_id` / `last_host_sender_id` |
| 协议 codegen 从 XML 自动枚举 | ✅ | `build.rs` 扫描目录，33 个 XML → 33 个模块 + 聚合分发 |
| 聚合 `dispatch_request` / `dispatch_event` / `impl_generated_delegates!` | ✅ | `wayland_codegen/src/aggregate.rs` |
| `&str` handler 参数（非 `&String`） | ✅ | 从 `main` 取回并统一全部手写 handler |
| quirks textproto-like 规则（steam_game_id / always / enable / disable / merge） | ✅ | `quirks.rs`；不引入 protobuf，多 condition 整条跳过 |
| 主线 CLI + `SOMMELIER_*` 环境变量 | ✅ | `config.rs`，7 个配置单测 |

### 协议清单（33 个 XML）

`aura-shell`, `content-type-v1`, `cursor-shape-v1`, `drm`,
`fractional-scale-v1`, `gaming-input-unstable-v2`, `gtk-shell`,
`idle-inhibit-unstable-v1`, `keyboard-extension-unstable-v1`,
`keyboard-shortcuts-inhibit-unstable-v1`, `linux-dmabuf-v1`,
`linux-drm-syncobj-v1`, `linux-explicit-synchronization-unstable-v1`,
`pointer-constraints-unstable-v1`, `presentation-time`,
`primary-selection-unstable-v1`, `relative-pointer-unstable-v1`,
`single-pixel-buffer-v1`, `stylus-unstable-v2`, `tablet-unstable-v2`,
`tearing-control-v1`, `text-input-crostini-unstable-v1`,
`text-input-extension-unstable-v1`, `text-input-unstable-v1`,
`text-input-unstable-v3`, `text-input-x11-unstable-v1`, `viewporter`,
`wayland`, `xdg-activation-v1`, `xdg-decoration-unstable-v1`,
`xdg-dialog-v1`, `xdg-output-unstable-v1`, `xdg-shell`。

其中 11 个协议有手写 delegation（见 `proxy.rs`），其余由聚合宏生成纯转发实现；
`aura_shell` 标记为 internal-only，其 host event 不会注入 guest。

## 6. 已知限制（按影响排序）

1. **X11 ↔ Wayland 剪贴板桥接未完成。**
   - WM 侧 `ClipboardBridge` 已能处理 X11 `SelectionRequest` / XFixes / INCR，
     并把结果通过 `X11Bridge::set_clipboard_targets/data` 发布。
   - proxy 侧尚未 `take_clipboard_targets/data()` 合成 guest `wl_data_offer`
     并响应 `wl_data_offer.receive`；反向的 `WmEvent::ClipboardTargets/Data`
     也未由 guest `wl_data_source` 事件触发。
   - 因此 Wayland 应用 ↔ host 剪贴板正常（原有 `data_device` 路径），
     X11 应用 ↔ host 剪贴板仍不完整。
2. **未在真实 Crostini 设备上做端到端验证。** 本环境为 Windows 主机 +
   `x86_64-unknown-linux-gnu` 交叉检查，可以编译和类型检查，但不能运行
   Linux 测试二进制。`cargo test` 需在 Linux guest 或 CI 上执行。
3. **X11 边框绘制、RandR 模拟、per-surface viewport 缩放、damage_buffer
   version clamp、explicit fence** 均为参数解析/协议转发级别，尚未完整实现。
4. **`--parent` 语义简化**：主线是 fork 出独立 worker 进程，本项目是同一进程内
   的 tokio task；隔离性弱于主线。
5. **内部绑定协议的 host event**：`registry.rs` 通过 `INTERNAL_GLOBALS` 阻止
   guest 绑定这些接口，但只有 `aura_shell` 在聚合分发层标记为 internal-only。
   实践中 host 不会对未绑定的对象发事件，风险较低。
6. **`current_thread` Tokio runtime 是硬约束**：`KeyboardHandler` 中的
   `xkbcommon` 对象 `!Send`，依赖 `unsafe impl Send`；任何改为多线程 runtime
   的尝试都会破坏该不变量。

## 7. 验证结果

| 检查项 | 结果 |
|---|---|
| `cargo check --target x86_64-unknown-linux-gnu --workspace` | ✅ 通过，0 error / 0 warning |
| `cargo check --tests --target x86_64-unknown-linux-gnu --workspace` | ✅ 通过，0 error / 0 warning |
| build script 生成 | ✅ 33 个协议模块 + 1 个聚合模块（34 个 include） |
| 单元测试 | 108 个 `#[test]` 已编译（含 keyboard 19、transform 17、quirks 14、config 8、wm 10、output 2 等） |
| CLI 参数解析 | ✅ 用独立 clap 形状验证实测：位置 display、`--display`、`-- <program>`、常见 flag 组合均正确解析，无歧义/panic |
| `cargo fmt --all -- --check` | ✅ 通过（已统一格式） |
| Rust 源码规模 | 15,975 行（`sommelier/src` + `wayland_codegen/src`） |
| 真实设备运行 | ❌ 未执行，见 §6.2 |

### 合并过程中修掉的既有缺陷

1. `event new_id` 恒等映射 → 通用 `allocate_guest_server_id()` 重映射；
2. 解析失败直接杀连接 → 区分 `UnknownOpcode`（丢单条）与其它协议错误；
3. `last_sender_id` 方向混用 → 增加方向专用字段，旧字段保持兼容；
4. `--virtgpu-channel` 是死参数 → 真正接入 `connect_host()`；
5. `VirtGpuChannel::new()` / `Allocator::new()` 不支持指定设备 → `new_with_device` / `with_device`；
6. `Cargo.lock` 中 `sommelier` 版本与 `Cargo.toml` 不一致 → 统一为 `0.3.0-r`；
7. `virtwl` 的 `&String` handler 参数 → 回退为 `&str`；
8. `WaylandConnection::new` 未设 `O_NONBLOCK`（virtwl 修复，已保留）；
9. EOF 误断连（main 修复，已回补）。
