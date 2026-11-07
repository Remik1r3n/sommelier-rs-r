# main ↔ virtwl 分支合并分析（sommelier-rs-r 的基础）

> 本文记录 `sommelier-rs` 两条分支 **逐文件 diff** 的结论，以及合并时的取舍。
> 基线：`main = e566da7`（`protocol: Add fractional_scale_v1`），
> `origin/virtwl = 922575c`（`Add systemd user service for sommelioer-rs for virtwl (#24)`）。
> `git diff --stat main..origin/virtwl`：40 文件，+6227 / −2324。

## 1. 先回答"为什么必须合并"，而不是"选一个分支"

| | main | virtwl |
|---|---|---|
| 传输后端 | virtio-gpu **cross-domain**（`virtgpu_channel.rs`，1445 行，靠 `/dev/dri/renderD*` 的 `CAPSET_CROSS_DOMAIN`） | **virtio_wl** `/dev/wl0`（`virtwl.rs` + `virtwl_channel.rs`） |
| ChromeOS 能否启动 | ❌ `VirtGpuChannel::new()` 找不到带 cross-domain capset 的 DRM 设备 → `Failed to connect to virtgpu` → 每个 client 连上即被丢弃 | ✅ |
| ChromeOS 专属协议 | ❌ 无 `zaura_shell`、`zcr_keyboard_extension_v1`、`zwp_text_input_v1` / `zcr_text_input_extension_v1` | ✅ 含 shelf app-id、系统快捷键 ack-key、cros-im 输入法 |
| keyboard / seat / text-input handler | ❌ 全透传 | ✅ 1250 + 566 + 35 行真实逻辑 |
| `--virtio-wl` CLI | ❌ | ✅ |
| `wl_data_offer` 的 server-created new_id 重映射 | ✅ 有（0xff000000 段） | ❌ 丢了 |
| sealed-memfd fallback allocator | ✅ 无 `/dev/dri` 依赖、加 seals 防 SIGBUS | ❌ 只有 GBM |
| virtgpu blob 分配路径 | ✅ | ❌ |
| `&str` handler 参数（codegen） | ✅ | ❌ 退化成 `&String` |
| `docs/ARCHITECTURE.md`、Debian CI/release workflow | ✅ | ❌ 删了 |
| `--disable-xdg-decoration` 过滤 | ✅ | ❌ 丢了（改成默认关闭的 opt-in） |
| `build_message()` wire 助手、内部 NUL 截断、详细解析日志 | ❌ | ✅ |
| `track_host_interface` / 带类型 `GuestId`/`HostId` / host ID 分配防撞 | ❌ | ✅ |
| 反向队列 flush（host→client 时把 `ack_key` 回送 host） | ❌ | ✅ |
| xdg_toplevel → wl_surface → zaura_surface app-id 链 | ❌ | ✅ |
| systemd user service、`accelerator.rs` | ❌ | ✅ |

**结论：任何单一分支都不够。** `virtwl` 分支是"能在 Crostini 跑起来"的必要底座，
但它是从较早的 main 分叉出去的，丢了 main 后续若干正确性修复与全部 virtio-gpu 代码；
而 main 的新功能（text-input-v3、fractional-scale）在两分支都有，不构成取舍点。
因此合并策略是：**以 virtwl 为运行时底座，把 main 的独占能力逐项回补，并把互斥的传输层抽象成可共存的三个后端。**

> 注：README 说"ChromeOS 用 virtwl 分支，main 跑不起来"，指的就是上表第 1 行。
> 本项目把两条传输线合到同一个二进制里，用 `--transport=auto|virtwl|virtgpu|unix` 选择，
> `auto` 在 Crostini 上优先 virtwl、通用 VMM 上回落 virtgpu。

## 2. 逐文件合并决策表

| 文件 | 处置 | 理由 |
|---|---|---|
| `sommelier/src/virtgpu_channel.rs` | **从 main 恢复** | 唯一实现 virtio-gpu cross-domain；非 Crostini 用户需要，且是"两套传输共存"的一半 |
| `sommelier/src/virtwl.rs`, `virtwl_channel.rs` | **保留 virtwl** | Crostini 主路径 |
| `sommelier/src/connection.rs` | **合并**：`ConnectionTransport` 同时保留 `Unix` / `VirtWayland` / `VirtGpu` 三个变体 | main 的 `VirtGpu{tx,rx}` actor 与 virtwl 的 `VirtWayland(Arc<...>)` 并不冲突；virtwl 额外修了 `WaylandConnection::new` 未设 `O_NONBLOCK` 的问题，保留 |
| `sommelier/src/state.rs` | **以 virtwl 为主体 + 回补 main** | 保留 typed `GuestId`/`HostId`、`host_interfaces`、ID 分配防撞、ChromeOS 全字段、单测；**加回** `allocate_guest_server_id()`（server-created object 必需）与 `virtgpu_channel` 字段 |
| `sommelier/src/allocator.rs` | **合并双后端** | main 的 sealed memfd 更稳健（无需 `/dev/dri`、防 SIGBUS），virtwl 的 GBM 在有 render node 时可零拷贝。统一成 `AllocatedBuffer`，避免 `gbm::BufferObject` 类型污染 `state.rs`/`shm.rs` |
| `sommelier/src/handler/shm.rs` | **合并三路径** | `virtwl_channel.allocate()` → `virtgpu_channel.allocate_host_blob()` → GBM → sealed memfd，按可用性降级；保留 virtwl 的 `build_message` 与尺寸修正 |
| `sommelier/src/handler/data_device.rs` | **两边都要** | main 的 `on_data_offer` 用 `allocate_guest_server_id()` 重映射 host 主动创建的 `wl_data_offer`（否则与 guest 命名空间冲突）；virtwl 的 `on_receive` 用 `VIRTWL_IOCTL_NEW_PIPE_READ` 建跨 VM 管道 + tokio pump，是 Crostini 剪贴板能用的关键。二者作用在不同 opcode 上，直接合并 |
| `sommelier/src/wire.rs` | **保留 virtwl** | `build_message()`、`read_string` 内部 NUL 截断（对齐 C 行为）、`InsufficientData` 详细日志都是净改进 |
| `wayland_codegen/src/generator.rs` | **取 main 的 `&str`**，并同步修所有 handler 签名 | virtwl 把 `generate_handler_args`/`_fq` 的 `&str` 改成 `&#ty`(=`&String`)，是为配合自己 handler 的临时改法，接口质量更差。统一回 `&str` 后，手写 handler 里 10 处 `&String` 全部改回 `&str` |
| `sommelier/src/handler/display.rs` | 随 generator 回 `&str` | 同上 |
| `sommelier/src/handler/registry.rs` | **以 virtwl 为主体 + 回补 main** | 保留 4 个 ChromeOS 内部绑定（text-input v1/extension、keyboard extension、zaura_shell）、`track_host_interface`、`build_message`；**加回**被 virtwl 删掉的 `zxdg_decoration_manager_v1` 开关过滤（否则 `--xdg-decoration` 形同虚设） |
| `sommelier/src/handler/compositor.rs` | **保留 virtwl** | ChromeOS shelf 的 app-id 映射链 + 4 个单测 |
| `sommelier/src/proxy.rs` | **合并** | 保留 virtwl 的 ChromeOS handler 注册、`get_host_interface` 派发、反向队列 flush；**加回** main 的 `bytes == 0 && read_fds.is_empty()` 防误断连；新增统一传输后端选择、XWayland 专用 client、X11 命令注入 |
| `sommelier/src/main.rs` | **重写** | 统一 CLI：既有 `--virtio-wl`、`--local-compositor`，也有 `--virtgpu-channel`；保留 `current_thread` runtime（xkbcommon `!Send` 的安全约束） |
| `sommelier/build.rs` | **取并集 + 扩展** | 11 个原有协议 + 22 个新增协议，改由单一清单驱动 |
| `sommelier/Cargo.toml` | **取并集** | `gbm`/`xkbcommon`（virtwl）+ `x11rb`（新 X11 子系统）；版本统一 |
| `Cargo.toml` / `.gitignore` / `CONTRIBUTING.md` | 保留 virtwl 版 | 无实质冲突 |
| `docs/ARCHITECTURE.md` | **从 main 恢复为 `ARCHITECTURE_BASELINE.md`** | virtwl 删掉了它；本项目保留为架构基线，实现说明改由 `docs/FEATURE_MATRIX.md` 维护 |
| `.github/workflows/ci-debian.yaml`, `release-debian.yaml` | **从 main 恢复** | virtwl 删掉的 Debian 打包 CI |
| `.github/workflows/ci.yaml`, `release.yaml`, `packaging/systemd/*`, `sommelier/docs/KEYBOARD_SHORTCUT_INHIBITION.md` | **保留 virtwl** | ChromeOS 部署与快捷键文档 |
| `accelerator.rs`, `handler/{keyboard,seat,text_input}.rs`, `bin.rs`, `test_xkb.rs` | **保留 virtwl** | ChromeOS 独占能力 |
| `third_party/protocols/*.xml` | **取并集** | 两分支共有的 7 个 + virtwl 独有的 4 个 ChromeOS 协议 + 从主线 sommelier/wayland-protocols 补入的 22 个 |

## 3. 合并中顺带修掉的既有缺陷

1. `generator.rs`：event 里带 `interface` 的 `new_id` 原本是 `map_id(id, id)` 恒等映射，
   会把 host ID 直接塞进 guest 命名空间。改为 `allocate_guest_server_id()` +
   `map_id(guest_id, host_id)` + `track_interface`，`data_device` 的手写特例保留但不再是唯一路径。
2. `proxy.rs`：解析失败原本直接 `return false` 断掉整个 client 连接；保留断连但补上可诊断日志与
   未知 opcode 的单消息丢弃策略（对 `ProtocolError::UnknownOpcode` 不杀连接）。
3. `Context::last_sender_id` 方向语义混用：补 `last_guest_sender_id` / `last_host_sender_id`
   两个字段（由 codegen 按方向分别写入），旧字段保持兼容。
4. main 的 `--virtgpu-channel` 是死参数（`main.rs` 解析后从未读取），合并后真正生效。
5. `Cargo.lock` 里 `sommelier` 版本与 `Cargo.toml` 不一致（0.1.0 vs 0.2.x），统一。

## 4. 不能靠合并解决、必须新写的部分

两分支都**没有**的东西，才是本项目真正的新增工作量，与合并正交：

- XWayland 启动 + X11 WM（主线 C++ `sl_spawn_xwayland` / `sl_connect` / `sl_handle_*`）：见 `src/x11/`
- 缩放/坐标变换（`sommelier-transform.cc` / `sommelier-output.cc`）：见 `src/transform.rs`
- quirks 规则引擎（`quirks/quirks.proto`）：见 `src/quirks.rs`
- 主线 CLI / `SOMMELIER_*` 环境变量兼容：见 `src/config.rs`
- 缺失的 Wayland 协议覆盖：见 `docs/FEATURE_MATRIX.md`

## 5. 合并执行结果（验收）

上述策略已全部落地，最终状态：

| 项目 | 结果 |
|---|---|
| 工程目录 | `sommelier-rs-r/`，binary 名 `sommelier-rs-r`，版本 `0.3.4-r` |
| 传输层 | `ConnectionTransport` 三变体 + `Config::resolve_transport` / `validate_transport` |
| 协议 | 33 个 XML 由 `build.rs` 自动枚举，生成 33 个模块 + `protocols::all` 聚合分发 |
| X11 | `x11/{mod,bridge,atoms,xwayland,wm,clipboard,selection}.rs` 七个模块，`main.rs` 条件启动 |
| 缩放 | `transform.rs` 经 `handler/output.rs` 接入 `wl_output` / `zxdg_output_v1` / `wp_fractional_scale_v1` 事件；`handler/viewport.rs` 换算 `wp_viewport` 目标尺寸 |
| 剪贴板 | `x11/selection.rs` 三向桥接：X11 owner → guest `wl_data_offer` + host `wl_data_source` / `set_selection`；host `send` 与 guest `receive` 均由 `ConvertSelection` 异步回写 fd；Wayland offer / source → TARGETS + X11 `SelectionRequest` + 夺回 CLIPBOARD owner。见 [`CLIPBOARD.md`](CLIPBOARD.md) |
| CLI | `config.rs` 统一 clap + `SOMMELIER_*`，`main.rs` 完成 X11 启动、子进程、sd_notify 接线 |
| 打包 | systemd unit 与 Debian release workflow 已改为新 binary 名 |
| 编译 | `cargo check` 与 `cargo check --tests`（`x86_64-unknown-linux-gnu`）均 0 error / 0 warning |

### 合并后又补完的第二轮（tag `v0.3.0-r`）

基线 tag `v0.3.0-r-merge-baseline` 之后继续补齐了功能矩阵中列出的缺口：

1. `preferred_scale` 改为主线严格等价的 `round(wire_scale / scale)`（保留 1/120 精度）。
2. `--no-support-damage-buffer` 真正生效：`wl_compositor` global / bind 钳制到 v3，
   并把违规 `damage_buffer` 请求改写成缩放后的 `wl_surface.damage`。
3. 聚合分发层把 10 个纯内部协议标记为 internal-only，杜绝 host 对象 id 注入 guest。
4. X11 ↔ Wayland 剪贴板双向桥接（见上表），含延迟 `SelectionRequest` 与 INCR 收发。
5. `--frame-color` / `--dark-frame-color` 边框绘制、
   `--allow-xwayland-emulate-screen-pos-size` RandR 模拟、
   `--viewport-resize` per-surface 缩放注入。
6. Debian CI 增加 `cargo test --workspace`、全目标 `cargo check --tests`、
   `cargo fmt --check`。

### 第三轮（tag `v0.3.1-r`）：剪贴板对齐主线

复核主线 C++ 源码后确认：`sommelier.cc` 的 `sl_set_selection()` /
`sl_get_selection_targets()` / `sl_begin_data_source_send()` 本来就有完整的
X11 ↔ host 双向剪贴板。第二轮只做到「X11 ↔ guest Wayland」，把 X11 内容合
成成了 guest `wl_data_offer`，而主线是合成 **host** `wl_data_source`；当时以
「缺真机验证」为由不实现 host 导出也不成立，主线本身就是可照抄的规格。
本轮补齐：

1. `registry.rs` 在 Xwayland 连接内部绑定 host `wl_data_device_manager`；
   `selection.rs::ensure_host_selection_device()` 从 guest 的
   `get_data_device` 反查 host seat 并创建内部 `wl_data_device`。
2. X11 TARGETS → `create_data_source` / `offer(mime)`* /
   `set_selection(source, host_serial)`；host `wl_data_source.send` →
   `ConvertSelection` → 写回 host fd。owner 清空时发 `set_selection(0, ..)`。
3. `Context::internal_selection_ids` 在 `proxy.rs` 隔离所有内部 host 对象的
   事件，避免污染 guest；`on_host_selection()` 过滤 selection 回声。
4. host / guest Wayland 复制时 `ClipboardBridge::claim_clipboard()` 夺回 X11
   CLIPBOARD owner（主线 `sl_set_selection` 行为）。
5. `maybe_update_host_serial()` 从 host 输入事件提取 serial；修正了一个旧测试
   与按 target 缓存语义不一致、会在 Linux CI 上失败的问题。

逐项完成度、剩余限制与 135 个单元测试的编译情况见
[`docs/FEATURE_MATRIX.md`](FEATURE_MATRIX.md)；主线对照见
[`docs/CLIPBOARD.md`](CLIPBOARD.md)。需要特别说明的是：
**本环境为 Windows 主机，只能做交叉类型检查；Linux 测试二进制由 Debian CI
执行，仍未在真实 Crostini 设备上跑过 `-X` 启动流程。**
