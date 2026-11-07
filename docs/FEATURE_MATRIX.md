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
| X11 selection：TARGETS / TIMESTAMP / UTF8_STRING / STRING / INCR / XFixes | ✅ | `x11/clipboard.rs`；无缓存数据时延迟回复 `SelectionRequest`（1.5 s 超时） |
| X11 窗口边框绘制（frame / dark frame color） | ✅ | `wm.rs::border_color` / `parse_frame_color`，1px X11 border + `_NET_FRAME_EXTENTS`；不做 reparenting frame |
| `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` | ✅ | `handler/output.rs` 发布虚拟坐标布局 → `bridge.rs` → `wm.rs::update_randr_emu` 写 root 属性（内容不变时不重写） |
| X11 ↔ guest Wayland 剪贴板互通 | ✅ | `x11/selection.rs`：X11 owner → 合成 `wl_data_device.data_offer`/`offer`/`selection`，`receive` → `ConvertSelection` → 回写 fd；反向追踪 host offer / guest data source，预取小体积文本载荷给 X11 `SelectionRequest` |
| X11 → ChromeOS host 剪贴板导出 | ✅ | 对齐主线 `sl_get_selection_targets()`：内部 bind host `wl_data_device_manager` + 内部 `wl_data_device`，X11 TARGETS → `wl_data_source.offer`* → `set_selection`；应答 host `wl_data_source.send`。见 [`docs/CLIPBOARD.md`](CLIPBOARD.md) |
| host / guest Wayland 复制时夺回 X11 CLIPBOARD owner | ✅ | `ClipboardBridge::claim_clipboard()`，对应主线 `sl_set_selection()` |

## 3. 缩放 / 坐标 / 输出

| 能力 | 状态 | 实现位置 |
|---|---|---|
| 三坐标系（Physical / Virtual / Host Logical）与 `--scale` | ✅ | `transform.rs` |
| `--direct-scale` / `--stable-scaling`（trunc / ceil / floor 差异） | ✅ | `transform.rs` |
| `--dpi` 桶与 physical mm 换算 | ✅ | `transform.rs::physical_size_for_dpi` |
| `wl_fixed` 24.8 定点变换（无 f32 精度损失） | ✅ | `transform.rs::fixed_mul_frac` |
| `wl_output.geometry/mode/scale/name/description/done` 改写 | ✅ | `handler/output.rs`（本次接入运行时） |
| `zxdg_output_v1.logical_position/logical_size` 改写 | ✅ | `handler/output.rs`；按主线 `sommelier.cc:759-775`，`zxdg_output_manager_v1` **不向 guest 公布**（仅 `--direct-scale` 时内部 bind），handler 保留用于 host 内部状态 |
| `wp_fractional_scale_v1.preferred_scale` 改写 | ✅ | `handler/output.rs` |
| 多输出布局、按位置推断输出、虚拟桌面尺寸 | ✅ | `transform.rs::set_outputs` / `virtual_dimensions` |
| per-surface `wp_viewport` 缩放（非可缩放窗口） | ✅ | `handler/viewport.rs`：`--viewport-resize` 时把 `set_destination` 用 `Transform::viewport_scale` 换算后重写 host 请求；`-1/-1` 直通 |
| `wl_surface.damage_buffer` 支持开关 | ✅ | 与主线 `sommelier-compositor.cc:1170` 一致：guest global 固定 **v4**（`--no-support-damage-buffer` / `SOMMELIER_SUPPORT_DAMAGE_BUFFER≠1` 时 v3），host bind 恒 v3；v4 `damage_buffer` 请求由 `handler/compositor.rs` 改写为按坐标系变换的 `damage` |
| `preferred_scale` 严格等价于主线 `round(wire_scale / scale)` | ✅ | 保留 1/120 精度，不再量化到 120 的整数倍；含分数 scale 单测 |

## 4. ChromeOS 专有能力（保留自 `virtwl`）

| 能力 | 状态 |
|---|---|
| `zaura_shell` / `zaura_surface` shelf app-id | ✅ 内部绑定（≤v38），不向 guest 暴露 |
| `gtk_shell1` → `zaura_shell` 协议桥接 | ✅ `handler/gtk_shell.rs`，对应主线 `sommelier-gtk-shell.cc`；host aura shell ≥ v6 时向 guest 公布 `gtk_shell1` v1 |
| `zcr_keyboard_extension_v1` / `zcr_extended_keyboard_v1` 系统快捷键 ack-key | ✅ |
| `SOMMELIER_ACCELERATORS` / `SOMMELIER_WINDOWED_ACCELERATORS` 过滤 | ✅ `accelerator.rs` |
| `zwp_text_input_v1` + `zcr_text_input_extension_v1` ↔ guest `zwp_text_input_v3` | ✅ 同时公布主线 v1（透明转发到 host，固定 v1）与 v3（映射到内部 host v1 manager）；extension 按主线 `MIN(host,11)` 公布。`handler/text_input.rs` |
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
| registry global 过滤（deny-list + 内部协议） | ✅ | `handler/registry.rs` + `protocols::all::is_known()`；X11 连接按主线不公布 `wl_data_device_manager` / `xdg_wm_base` |
| 未知 opcode 只丢单条消息、不杀连接 | ✅ | `proxy.rs::handle_msgs` |
| EOF 判定 `bytes == 0 && read_fds.is_empty()` | ✅ | 从 `main` 回补 |
| 方向分离的 sender id | ✅ | `last_guest_sender_id` / `last_host_sender_id` |
| 协议 codegen 从 XML 自动枚举 | ✅ | `build.rs` 扫描目录，33 个 XML → 33 个模块 + 聚合分发 |
| 聚合 `dispatch_request` / `dispatch_event` / `impl_generated_delegates!` | ✅ | `wayland_codegen/src/aggregate.rs` |
| `&str` handler 参数（非 `&String`） | ✅ | 从 `main` 取回并统一全部手写 handler |
| 内部绑定协议的 host event 隔离 | ✅ | `aggregate.rs::INTERNAL_ONLY` 覆盖 10 个纯内部模块；text-input v1 / extension 因需要转 v3 事件而有意保留转发 |
| 主线式 global 版本钳制 | ✅ | `advertised_version()` / `host_bind_version()`：compositor 4/3、output ≤3、seat ≤5、data_device_manager ≤3、xdg_wm_base ≤3、dmabuf ≤4、text-input-extension ≤11、wl_drm ≤2 |
| `damage_buffer` → `damage` 改写 | ✅ | `handler/compositor.rs`，含 2 个单测 |
| quirks textproto-like 规则（steam_game_id / always / enable / disable / merge） | ✅ | `quirks.rs`；不引入 protobuf，多 condition 整条跳过 |
| 主线 CLI + `SOMMELIER_*` 环境变量 | ✅ | `config.rs`，12 个配置单测；`--socket`/`--display`/位置参数语义与主线一致。逐项对照见 [`docs/MAINLINE_PARITY.md`](MAINLINE_PARITY.md) |

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

其中 12 个协议有手写 delegation（见 `proxy.rs`），其余由聚合宏生成纯转发实现；
`aggregate.rs::INTERNAL_ONLY` 列出 10 个纯内部绑定模块，其 host event 在聚合
分发层直接丢弃，避免把 host 对象 id 注入 guest。

## 6. 已知限制（按影响排序）

> **修订说明**：此前 §6.1 记录的「X11 → ChromeOS host 剪贴板导出未实现」已补齐。
> 主线 C++ `sommelier` 本来就有完整的双向剪贴板（`sommelier.cc` 的
> `sl_set_selection` / `sl_get_selection_targets` / `sl_begin_data_source_send`），
> 上一版把它误判为「需要真机才能做」的风险项，并用 guest 侧合成 offer 代替了
> host 侧合成 source，方向不对。现按主线语义实现，逐行对照与链路图见
> [`docs/CLIPBOARD.md`](CLIPBOARD.md)。

1. **ChromeOS host 侧的人工交互尚未验证。** Wayland 与 X11 两条主链路已在
   Debian 13 / `penguin` / `/dev/wl0` 真机端到端跑通（见 §7 与
   [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5），但「X11 应用复制 → 在
   ChromeOS 界面里粘贴」需要人工在 host 侧确认；用 `--peer-pid`/`--client-fd`
   顶替现役实例、长时间稳定性与真实 GPU 加速路径同样未覆盖。
2. **explicit fence**：`--explicit-fence` 与
   `zwp_linux_explicit_synchronization_v1` XML 就绪，但未接入
   `linux_explicit_synchronization` 的 fence 收发逻辑；该协议目前按内部
   global 过滤，不向 guest 暴露。
3. **`--parent` 语义简化**：主线是 fork 出独立 worker 进程，本项目是同一进程内
   的 tokio task；隔离性弱于主线。
4. **大体积剪贴板载荷**：Wayland → X11 方向只预取文本类与常见图片 MIME，
   上限 4 MiB；超过上限或非预取类型由延迟 `SelectionRequest` 触发按需
   获取，1.5 s 内未取到会按 ICCCM 回复 property NONE。X11 → host / guest
   Wayland 方向不受此限制（`ConvertSelection` 直接回写 fd）。
5. **`wl_data_device.set_selection` 的 serial 可能为 0**：`Context::host_serial`
   由 host 输入事件提取；进程刚启动尚未收到输入时复制，会带 serial 0 发送并
   打印 warning（主线 `last_serial` 初值同样为 0）。若 ChromeOS compositor
   严格校验 serial，这一次导出可能被拒绝，下一次输入事件后恢复正常。
6. **`current_thread` Tokio runtime 是硬约束**：`KeyboardHandler` 中的
   `xkbcommon` 对象 `!Send`，依赖 `unsafe impl Send`；任何改为多线程 runtime
   的尝试都会破坏该不变量。剪贴板的管道读写也因此用 `tokio::fs` 的
   blocking pool 完成，不占 runtime 线程。

## 7. 验证结果

| 检查项 | 结果 |
|---|---|
| `cargo check --target x86_64-unknown-linux-gnu --workspace` | ✅ 通过，0 error / 0 warning |
| `cargo check --tests --target x86_64-unknown-linux-gnu --workspace` | ✅ 通过，0 error / 0 warning |
| build script 生成 | ✅ 33 个协议模块 + 1 个聚合模块（34 个 include） |
| 单元测试 | **143 个 `#[test]` 在真实 Crostini（Debian 13 / `penguin`）上全部运行通过**（139 + 4 + 0 doc-tests）；本轮新增 env/bool 绑定回归测试，并修正 `preferred_scale` 退化分支测试 |
| CLI 参数解析 | ✅ 用独立 clap crate 在 Windows 本机**实跑** 6 组形状验证全部通过：`--socket` vs `--display`、位置参数 runprog、`--` 分隔、`--sd-notify` 不吃位置参数、全量主线 flag 拼写、`--help`/`--version` |
| `cargo fmt --all -- --check` | ✅ 通过（已统一格式） |
| CI | `.github/workflows/ci-debian.yaml`：Debian 13 x86_64 / aarch64 构建、x86_64 `cargo test --workspace`、全目标 `cargo check --tests`、`cargo fmt --check` |
| Rust 源码规模 | 19,283 行（`sommelier/src` + `wayland_codegen/src`） |
| 真机端到端 | **已验证**：virtwl → host globals/事件、guest registry 与版本钳制、`gtk_shell1`/text-input-v3 synthetic global、Xwayland `:9` 启动、WM 管理真实 X11 窗口并映射为 host `xdg_toplevel`、RandR 模拟、X11 ↔ guest 剪贴板往返、键盘焦点。详见 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5 |
| 真实设备运行 | ✅ 已在 Debian 13 / `penguin` / `/dev/wl0` 真机运行（独立 `wayland-wb` / `:9`，现役 `wayland-0/1`、`:0/:1` 未受影响）；仍未覆盖 host 侧人工交互，见 §6 第 1 项 |
| Debian `.deb` 交付 | ✅ `packaging/build-deb.sh` 在真机上构建 `sommelier-rs-r_0.3.4-r_amd64.deb`（release + strip + `dpkg-shlibdeps` 解析依赖），随包附带两个 disabled-by-default systemd user 模板与全量文档；已实机 `dpkg -i` 安装并验证两个单元均秒级 `active`。安装与使用见 [`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md)，打包验证见 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5.4 |

### 合并过程中修掉的既有缺陷

1. `event new_id` 恒等映射 → 通用 `allocate_guest_server_id()` 重映射；
2. 解析失败直接杀连接 → 区分 `UnknownOpcode`（丢单条）与其它协议错误；
3. `last_sender_id` 方向混用 → 增加方向专用字段，旧字段保持兼容；
4. `--virtgpu-channel` 是死参数 → 真正接入 `connect_host()`；
5. `VirtGpuChannel::new()` / `Allocator::new()` 不支持指定设备 → `new_with_device` / `with_device`；
6. `Cargo.lock` 中 `sommelier` 版本与 `Cargo.toml` 不一致 → 两者同步（当前 `0.3.4-r`）；
7. `virtwl` 的 `&String` handler 参数 → 回退为 `&str`；
8. `WaylandConnection::new` 未设 `O_NONBLOCK`（virtwl 修复，已保留）；
9. EOF 误断连（main 修复，已回补）。
