# 主线 C++ sommelier ↔ sommelier-rs-r 行为对照

本轮目标：消除"不必要的区别"，让**该兼容一致的地方**与主线基本一致。
对照对象是本机的主线 checkout `../sommelier/`（`sommelier.cc` 等，约
167k 行 C++）与本仓库 Rust 实现。行号引用均指主线 C++ 源码。

结论：**CLI/环境变量、global 版本、剪贴板 MIME、X11 启动与 app_id、
输出几何、INCR 阈值等 30 余处语义偏差已修正**；用户明确要求保留的
新协议能力、以及少数无法在单进程代理架构下 1:1 复刻的路径，列在
§3 / §4，属于有意保留，不是遗漏。

---

## 1. 本轮修正的不一致项

### 1.1 命令行与环境变量（最严重的一处）

| # | 主线行为 | 修正前 Rust | 修正后 |
|---|---|---|---|
| 1 | `--socket=SOCKET` 是 **guest 监听** socket 名，默认 `wayland-0`（`:4199`、`:4253`） | 没有 `--socket` | 新增，`SOMMELIER_SOCKET` 为 env 别名 |
| 2 | `--display=DISPLAY` / `SOMMELIER_DISPLAY` 是要连接的 **host** compositor（`:4176`、`:4255`；README.md:38-42） | 被当成**监听** socket 名，语义完全颠倒 | 改为 host display；相对名按 `$XDG_RUNTIME_DIR` 解析，并选择 unix transport（对应 `:4464-4466` 的 `use_virtual_context=false`） |
| 3 | 位置参数是 `[program] [args...]`，遇到第一个位置参数即停止解析选项（`:4354-4364`） | 第一个位置参数被当成 display 别名，`sommelier --scale=1.5 weston-terminal` 会把 socket 名改成 `weston-terminal` | 位置参数全部作为 `runprog`（`trailing_var_arg`），与主线一致 |
| 4 | `--scale` 非 direct 模式 `round()` 后 clamp 到 `[0.1,10]`；direct 模式取原值（`:4403-4413`） | 一律只 clamp，不取整 | 按模式取整；`desired_scale` 单独保留 |
| 5 | 检测到 host `wp_viewporter` 时 `ctx.scale = clamp(desired_scale)`，去掉整数取整（`:644-657`） | 无 | `Context::desired_scale` + `Transform::set_scale()`，在 registry 收到 `wp_viewporter` 时应用 |
| 6 | `--fullscreen-mode` 只接受 `immersive`/`plain`，否则 fatal（`:4446-4456`） | 任意字符串都接受 | `Config::validate()` 拒绝非法值并退出 |
| 7 | `SOMMELIER_SUPPORT_DAMAGE_BUFFER`：仅 `"1"` 或 unset 启用（`:4182`、`:4437`） | 只有 `--no-support-damage-buffer` flag | env 按主线判定 |
| 8 | `SOMMELIER_CLIPBOARD_MANAGER`：仅字面 `"0"` 禁用；X11 模式默认开（`:4394-4401`） | 用通用 bool 解析，且未读 env | 按主线判定 |
| 9 | `SOMMELIER_GLAMOR` 是字符串，`"0"` 才禁用；`-shm` 条件 `!drm \|\| !glamor \|\| glamor=="0"`（`:4085-4089`） | `glamor && render_node` 布尔近似，未读 env 的 `"0"` | 按主线字符串语义 |
| 10 | `SOMMELIER_XWAYLAND_GL_DRIVER_PATH`（`:4191`） | 仅 flag | flag + env |
| 11 | `--enable-linux-dmabuf`（`:4318`）、`--application-id`（`:4280`）、`--application-id-x11-property`（`:4277`）、`--peer-pid`、`--client-fd`、`--peer-cmd-prefix`、`--sd-notify=...`、`--timing-filename`、`--stats-*`、`--trace-*`、`--ignore-stateless-toplevel-configure`、`--only-client-can-exit-fullscreen`、`--windowed_accelerators` | 大部分会被 clap 拒绝启动 | 全部按主线拼写接受；`--enable-linux-dmabuf` 映射到 `gpu_accel`，`--sd-notify` 支持有值/无值 |
| 12 | X11 模式给 `runprog` 设 `WAYLAND_DISPLAY="."`；非 X11 模式 `unsetenv("DISPLAY")`（`:3630-3635`、`:4610-4613`） | 总是把 `WAYLAND_DISPLAY` 指向自己的 socket | 按主线 |
| 13 | `--enable-xshape` 默认 **false**（`sommelier-ctx.cc:210`） | `X11Options::default()` 为 true | 默认 false |
| 14 | `--dpi` 用 `strtok` + `atoi`（`"96.5"→96`、非数字→0→clamp 72，`:4519-4532`）；`--scale` 用 `atof`（尾部垃圾忽略，`:4404`） | clap `f64` / `u32` 严格解析，`"1.5junk"` 会直接拒绝启动 | 改为字符串接收 + `atoi_prefix()` / `atof_prefix()` 复刻 C 语义 |
| 15 | `--sd-notify=VALUE` 用 `sl_arg_value`（`:4294`） | bool flag | `Option<String>`，`require_equals` 保证不会吞掉后面的位置参数 |

### 1.2 协议 global 与版本

主线 `sl_global_create` 是唯一的 guest global 发布入口，版本都经过 `MIN()` 钳制。
修正前 Rust 把 host 版本原样透传。

| 接口 | 主线公布版本 | 位置 | 修正 |
|---|---|---|---|
| `wl_compositor` | **固定 4**（实现 `damage_buffer`），`--no-support-damage-buffer` 时 **3**；host 侧恒 bind **3** | `compositor/sommelier-compositor.cc:1150-1191` | `advertised_version()` / `host_bind_version()` 固定 |
| `wl_output` | `MIN(3, host)` | `:552` | 钳制 |
| `wl_seat` | `MIN(5, host)` | `:562` | 钳制 |
| `wl_data_device_manager` | `MIN(3, host)` | `:596` | 钳制 |
| `xdg_wm_base` | `MIN(3, host)` | `:615` | 钳制 |
| `zwp_linux_dmabuf_v1` | `MIN(4, host)`，且默认**不公布** | `:664-671` | 钳制；`--enable-linux-dmabuf` 等价开启 |
| `zcr_text_input_extension_v1` | `MIN(11, host)` | `:725` | 钳制 |
| `zwp_text_input_manager_v1` | 固定 1 | `:711` | 固定 |
| `zaura_shell` | 内部 bind `MIN(38, host)`，guest 侧改发 **`gtk_shell1` v1**（host ≥ 6） | `:629-643` | 见下 |
| `zxdg_output_manager_v1` | **不向 guest 公布**，仅 `--direct-scale` 时内部 bind | `:759-775` | 从 guest 公告中移除（内部 bind 未复刻，见 §4） |
| `wl_drm` | host dmabuf ≥ 2 时公布 v2 | `compositor/sommelier-drm.cc:207-217` | 允许 host `wl_drm` 透传并钳制 v2（模拟实现仍未做，见 §4） |
| X11 client | **不公布** `wl_data_device_manager` / `xdg_wm_base`，只内部 bind | `:589-628` | `is_advertisable()` 对 Xwayland 连接过滤 |

补充实现：

- **`gtk_shell1` 桥接**（`handler/gtk_shell.rs`）：对应主线
  `sommelier-gtk-shell.cc`。`get_gtk_surface` → `zaura_shell.get_aura_surface`，
  `set_dbus_properties` → `zaura_surface.set_application_id("org.chromium.guest_os.<vm>.wayland.<id>")`，
  `set_startup_id` 缓存并重放，`system_bell`/`set_modal`/`unset_modal`/`present` 为 DoNothing。
- **text-input**：主线公布 v1 manager，Rust 同时公布 v1（透明转发到 host）
  与 v3（用户要求保留的新能力，映射到内部 host v1 manager）。
- `zcr_text_input_extension_v1` 现在同时内部 bind 并向 guest 公布（≤11）。

### 1.3 输出 / 坐标

| 项 | 主线 | 修正 |
|---|---|---|
| `wl_output.geometry` 位置 | `geometry(virt_x, 0, ...)`，`virt_x` 为左侧所有输出的虚拟宽度累加，**y 恒 0**（`sommelier-output.cc:283-295`、`:401`） | 新增 `Transform::virtual_x_for_host_x()`，`on_geometry` 发送 `(virt_x, 0)` |
| X11 模式的 `wl_output.scale` | 强制 1（`sommelier-output.cc:106-108`） | Xwayland 连接强制 1 |
| `wp_fractional_scale.preferred_scale` | `round(wire_scale / ctx.scale)`（`sommelier-fractional-scale.cc:42-44`） | 已一致，保留 |
| `wl_output.scale`（Wayland） | `MIN(ceil(applied_scale/ctx.scale), 2)` | 已一致，保留 |
| `damage_buffer → damage` | 非 direct 分支 `±1` 外扩、min 角 trunc / max 角 ceil；direct 不外扩 | 已一致，保留 |
| `--stable-scaling` 负坐标 | `ceil`/`floor`（非 trunc） | 已一致，保留 |

### 1.4 X11 / 剪贴板

| 项 | 主线 | 修正前 Rust | 修正后 |
|---|---|---|---|
| Xwayland 命令行 | `[:N] -nolisten tcp -rootless [-shm] -displayfd <fd> -wm <fd> [-auth] [-fp]`（`:4029-4120`，**无 `-terminate`**） | 已一致 | 保留 |
| atom 表 | 39 个，含 `_WL_SELECTION`（下划线）、`WL_SURFACE_ID` | 已一致 | 保留 |
| MIME ↔ X11 target | **1:1 原样透传**（`sl_get_selection_targets:2819-2841`、`sl_send_data:2931-2967`）；X11 侧只额外插入 `TARGETS`/`TIMESTAMP`（`:354-371`） | 自造 `UTF8_STRING ↔ text/plain;charset=utf-8` 双向转换，导致 host 看到的 MIME 名与主线不同 | 导出/请求路径改回 1:1；文本别名降级为**额外 TARGETS 项**和预取候选，不再改写 MIME 名 |
| `TARGETS` 顺序 | `TARGETS`, `TIMESTAMP`, 然后 MIME 原名 | `UTF8_STRING`, `STRING`, `TEXT`, `TARGETS`, `TIMESTAMP`, ...；还把 `_WL_SELECTION` 当 target | 主线顺序；`_WL_SELECTION` 不再出现在 TARGETS |
| INCR | 阈值与 chunk 均 `64 KiB`（`:2214`）；INCR property 的值是 **chunk size** 不是总长度（`:2250-2257`） | 阈值 128 KiB / chunk 32 KiB；property 写总长度 | 全部按主线 |
| X11 app_id | `org.chromium.guest_os.<vm>.<wmclass\|xid\|wmclientleader\|xprop>.<id>`，`.wayland.` 只给原生 Wayland（`sommelier.h:25-27`、`sommelier-window.cc:29-35,1064-1097`） | X11 窗口也用 `.wayland.` 前缀 | `WmState::build_app_id()` 按主线优先级构造，支持 `--application-id` 与 CARDINAL property |
| `_NET_SUPPORTED` | 8 个 atom，固定顺序（`:3157-3175`） | 13 个且顺序不同 | 前 8 个与主线逐字节一致，额外实现的 5 个追加在后 |
| clipboard manager | X11 模式默认开，创建 `CLIPBOARD_MANAGER` owner + XFixes 监听（`:3362-3379`） | 已一致 | 保留 |

---

## 2. 与主线一致、本轮确认无需改动的部分

- `sl_spawn_xwayland()` 的参数顺序、`WAYLAND_SOCKET` / `SOMMELIER_VERSION` /
  `LIBGL_DRIVERS_PATH` 环境变量、三个 socketpair fd 传递。
- `-displayfd` 回传后父进程 `setenv("DISPLAY")`、`XCURSOR_SIZE = 24*scale`。
- WM 初始化顺序：SubstructureRedirect → XFixes(≥5) / Composite → WM 窗口 →
  `_NET_SUPPORTING_WM_CHECK` / `_NET_WM_NAME` → `_NET_SUPPORTED` → `WM_S0` owner。
- 剪贴板三向流转（X11 → guest → host / host → X11 / guest → host 回环）、
  `host_serial` 取自输入事件、`sl_set_selection` 夺回 X11 CLIPBOARD owner。
- `--dpi` 桶 `[72,9600]`、就近选择且平局取靠前、`dpi_to_physical_mm = px*25.4/dpi`。
- `MIN_SCALE/MAX_SCALE = 0.1/10`、`MAX_OUTPUT_SCALE = 2`、`DEFACTO_DPI = 96`。
- 三坐标系（physical / virtual / host logical）的 trunc、stable ceil/floor、
  `wl_fixed` 24.8 换算与旋转处理。
- `--sd-notify` → `READY=1`、`--no-exit-with-child`、`--parent` 的进程语义。

---

## 3. 有意保留的差异（用户明确要求的增强，不回退）

| 项 | 说明 |
|---|---|
| `zwp_text_input_manager_v3` | 用户要求保留；主线只有 v1 + zcr 扩展。Rust **同时**公布 v1 和 v3 |
| `wp_fractional_scale_v1`、新版协议 XML | 用户要求保留；Rust 有 33 个 XML，主线 protocol/ 只有 20 个 |
| 统一 transport `auto\|virtwl\|virtgpu\|unix` | 主线是编译期/启动期二选一；Rust 三种后端可运行时回退 |
| 单进程多 client | 主线 `--parent` 为每个 client fork 一个 sommelier；Rust 用单进程 task |
| `SOMMELIER_VERSION` 取 crate 版本 | 主线固定 `0.20`；此处保留可追溯的版本号 |
| `--transport` / `--virtio-wl` / `--local-compositor` / `SOMMELIER_SOCKET` | Rust 扩展，主线无；不覆盖任何主线参数 |
| `TARGETS` 末尾的 `UTF8_STRING`/`STRING`/`TEXT` 别名 | 主线只用 MIME 原名；额外别名扩大 X11 客户端兼容性，不改变导出到 host 的 MIME |
| 预取时优先 `UTF8_STRING`（`preferred_x11_target`） | 主线 `sl_begin_data_source_send` 始终用字面 MIME 名 intern atom；Rust 预取文本 MIME 时先试 `UTF8_STRING`，失败再回退字面名。这是**读取方向的宽松匹配**，只在 X11 owner 一侧生效；host 收到/发出的 `wl_data_source.offer` MIME 名仍是 1:1 原样，不受影响 |
| `_NET_SUPPORTED` 追加的 5 个 atom | Rust 确实读取 `_NET_WM_WINDOW_TYPE` / `_NET_WM_PID` / `_NET_FRAME_EXTENTS` 等 |

---

## 4. 仍未 1:1 对齐的项目（如实列出）

| 项 | 主线 | Rust 现状 | 原因 |
|---|---|---|---|
| `wl_drm` 本地模拟 | 从 host `zwp_linux_dmabuf_v1` v2 模拟出 `wl_drm` v2（含 `authenticate`/`create_prime_buffer`） | 仅当 host 自己公布 `wl_drm` 时透传 | 需要 DRM render node + GBM 句柄与 host 认证交互，在 virtio_wl 后端不可靠 |
| `sl_calculate_scale_for_xwayland` | Xwayland ready 后按 `device_scale_factor × preferred_scale` 重算 `ctx.scale`（`:3507-3549`） | 只实现了 `wp_viewporter` 出现时应用 `desired_scale` | 需要 host `zaura_output` 的 scale/DSF 事件；目前只有 `wl_output.scale` |
| `wl_output.mode` 宽高公式 | `host_w × ctx.scale × s / applied_scale`（含 aura_shell 的 `applied_scale`） | `trunc(host_w × ctx.scale)` | 两者算出的**逻辑尺寸**一致（Rust 用整数 `wl_output.scale` 承载差值），但 wire 上的 size/scale 数值不同 |
| `--direct-scale` 时的 `zxdg_output_manager_v1` 内部 bind | 内部 bind 并向每个 output 取 `logical_size` / `logical_position`，用于 direct 模式的坐标换算（`:759-775`） | 不公布也不 bind；`zxdg_output_v1` 事件 handler 保留但不会被触发 | Rust 的 direct 模式直接用 `wl_output` 几何 + `Transform` 换算，不依赖 xdg-output；guest 侧行为一致，但未复刻该 host 侧数据来源 |
| `--viewport-resize` | 在 surface commit 时用 `window->viewport_width/height` 覆盖 host viewport destination | 在 guest `wp_viewport.set_destination` 上按 scale 换算 | 实现路径不同，效果接近；未复刻 `viewport_pointer_scale` 指针补偿 |
| `zcr_text_input_crostini_manager_v1` / `zcr_text_input_x11_v1` | 由 sommelier 本地实现并公布 | 未公布 | 纯 guest 侧 IME 协议，需要完整实现才能安全公布；v3 路径已覆盖现代客户端 |
| `zaura_shell` → `zwp_tablet_manager_v2`（stylus） | stylus-v2 协议转换 | 透传 tablet-v2 | 主线做的是协议转换，Rust 保留直接透传 |
| `--peer-pid` / `--client-fd` / `--peer-cmd-prefix` | parent/child 进程间 fd 传递 | 只解析、不实现 | 与单进程架构不兼容；接受参数避免上游命令行启动失败 |
| `--timing-filename` / `--stats-*` / `--trace-*` | Perfetto / 帧统计 | 只解析、不实现 | 可观测性功能，不影响协议兼容 |
| INCR 接收侧缓冲策略 | 必须先缓冲 64 KiB 再决定 one-shot / INCR（`:2219-2235`） | 采用预取缓存 + 按需 ConvertSelection | 阈值与 chunk 已对齐，驱动方式不同 |
| `host_serial == 0` 的首次导出 | 使用真实 seat serial | 启动初期可能为 0 | 需在 guest 内首次输入事件后才有有效 serial |
| 真实 Crostini 端到端 | — | 未验证 | 无真机环境；由 Debian CI 覆盖编译与单元测试 |

---

## 5. 验证

- `cargo check --target x86_64-unknown-linux-gnu --workspace` — 通过，0 error / 0 代码 warning
- `cargo check --tests --target x86_64-unknown-linux-gnu --workspace` — 通过
- `cargo fmt --all -- --check` — 通过
- **CLI 形状实测**：把 `Args` 定义抽到独立 clap crate 在 Windows 本机**实际运行**，
  5 组断言全部通过 —— `--socket` vs `--display`、位置参数 runprog、`--`
  分隔、`--sd-notify` 不吃位置参数、`--scale=1.5junk` / `--dpi=96.5,nonsense`
  宽松解析、全量主线 flag 拼写。
- 142 个 `#[test]` 全部编译通过。Linux target 测试二进制在本机无法执行
  （WSL 被安全策略禁用、且缺 Linux linker `cc`），由
  `.github/workflows/ci-debian.yaml` 在 Debian 13 上运行
  `cargo test --workspace`。
- 新增/更新的回归测试覆盖：`--socket` vs `--display` 语义、位置参数 runprog、
  主线 flag 解析、`--scale` 取整与 `atof`/`atoi` 语义、`--fullscreen-mode`
  校验、MIME 1:1 与别名、`normalize_targets` 顺序、`gtk_shell1` 桥接、
  `virtual_x_for_host_x()`。
