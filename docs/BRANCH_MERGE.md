# main ↔ virtwl 分支合并决策记录（历史文档）

> **历史文档**：记录合并当时的取舍依据，当前状态见 §5。
>
> 本文记录 `sommelier-rs` 两条分支 **逐文件 diff** 的结论，以及合并时的取舍。
> 基线：`main = e566da7`（`protocol: Add fractional_scale_v1`），
> `origin/virtwl = 922575c`（`Add systemd user service for sommelioer-rs for virtwl (#24)`）。
> `git diff --stat main..origin/virtwl`：40 文件，+6227 / −2324。

## 1. 先回答"为什么必须合并"，而不是"选一个分支"

| | main | virtwl |
|---|---|---|
| 传输后端 | virtio-gpu **cross-domain**（`virtgpu_channel.rs`，1445 行，靠 `/dev/dri/renderD*` 的 `CAPSET_CROSS_DOMAIN`） | **virtio_wl** `/dev/wl0`（`virtwl.rs` + `virtwl_channel.rs`） |
| ChromeOS 能否启动 | ❌ `VirtGpuChannel::new_with_device()` 找不到带 cross-domain capset 的 DRM 设备 → `Failed to connect to virtgpu` → 每个 client 连上即被丢弃 | ✅ |
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
| `docs/ARCHITECTURE.md` | **重写为当前架构文档** | 合并期曾保留上游 main 的原始底稿，现已删除；当前结构由 `ARCHITECTURE.md` 维护，完成度由 `FEATURE_MATRIX.md` 维护 |
| `.github/workflows/ci-debian.yaml`, `release-debian.yaml` | **保留并整改** | Debian 13 CI 与 `.deb` 发布；release 现调用 `packaging/build-deb.sh` 产出 `.deb` |
| `packaging/systemd/*`, `docs/KEYBOARD_SHORTCUT_INHIBITION.md` | **保留 virtwl** | ChromeOS 部署单元与 Exo 快捷键机制设计说明 |
| `accelerator.rs`, `handler/{keyboard,seat,text_input}.rs` | **保留 virtwl** | ChromeOS 独占能力；早期的 xkb 实验文件已删除 |
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

## 5. 当前状态

上表的合并决策已全部落地。本文记录的是**当时的取舍依据**，不是当前实现
说明：架构见 [`ARCHITECTURE.md`](ARCHITECTURE.md)，逐项完成度见
[`FEATURE_MATRIX.md`](FEATURE_MATRIX.md)，与主线 C++ 的对齐情况与真机验证见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md)，安装与打包见
[`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md)。
