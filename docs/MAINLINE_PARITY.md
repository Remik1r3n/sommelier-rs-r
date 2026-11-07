# 主线 C++ sommelier ↔ sommelier-rs-r 行为对照

本轮目标：消除"不必要的区别"，让**该兼容一致的地方**与主线基本一致。
对照对象是本机的主线 checkout `../sommelier/`（`sommelier.cc` 等，约
167k 行 C++）与本仓库 Rust 实现。行号引用均指主线 C++ 源码。

结论：**CLI/环境变量、global 版本、剪贴板 MIME、X11 启动与 app_id、
输出几何、INCR 阈值等 30 余处语义偏差已修正**；`v0.3.6-r` 又补齐了
`wl_drm` v2 本地模拟、两个 Crostini 私有 IME 协议、`zaura_output` /
`zxdg_output_v1` 内部订阅与 Direct 模式真实 logical size 换算（见 §1.6）。
用户明确要求保留的新协议能力、以及少数无法在单进程代理架构下 1:1
复刻的路径，列在 §3 / §4，属于有意保留或有据可查的已知差距，不是遗漏。

后续又在**真实 Crostini 环境**（Debian 13 / `penguin` / `/dev/wl0`）做了
端到端验证，并据此修掉 3 个只有真机才能暴露的问题，见 §1.5 与 §5。

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
| `zxdg_output_manager_v1` | **不向 guest 公布**，仅 `--direct-scale` 时内部 bind | `:759-775` | `v0.3.6-r` 从 guest 公告中移除并在 Direct 模式下内部 bind，`logical_*` 本地消费 |
| `wl_drm` | host dmabuf ≥ 2 时公布 v2，由**本地模拟**实现 | `compositor/sommelier-drm.cc:207-217` | `v0.3.6-r` 本地模拟 v2；host 侧同名 global 列入 `INTERNAL_GLOBALS` 不再透传 |
| `zcr_text_input_crostini_manager_v1` / `zcr_text_input_x11_v1` | sommelier 本地实现并公布 | `sommelier-text-input.cc:548-697` | `v0.3.6-r` 本地实现并公布 v1 |
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
| `device_scale_factor` 初值 | `1000`（1.0），仅 aura 事件覆盖（`sommelier-output.cc:649`） | 缺省 1.0，不再用 `wl_output.scale` 兜底 |
| X11 分支的目标 DPI | `DEFACTO_DPI × device_scale_factor`（`:112`） | `physical_size_for_dpi()` 用真实 DSF（真机 1.25 → 120 DPI → 406×270mm） |
| display-ready 后的 output update | `sl_calculate_scale_for_xwayland` 重发完整 state（`:3541-3548`） | DSF 到达时 `resend_output_state()` 重发 geometry/mode/scale/done |
| Direct 模式坐标换算 | 内部 xdg-output 的 `logical_size` | 同样内部 bind，优先 `virtual_width / logical_width` |

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

### 1.5 真机端到端发现并修复的问题

前四节是静态对照的结果；以下三项只有在真实 Crostini 上**实际运行**才暴露，
均已修复并在 §5 复验。

| # | 现象 | 根因 | 修复 |
|---|---|---|---|
| 1 | 继承现役 systemd 环境后直接拒绝启动：`error: invalid value '1' for '--glamor' [possible values: true, false]` | 主线把 `SOMMELIER_GLAMOR` / `SOMMELIER_CLIPBOARD_MANAGER` 当**字符串**（只有 `"0"` 禁用），真机注入的是 `"1"`；Rust 却把 6 个 bool flag 直接绑到 clap 的 `env`，clap 只接受 `true/false`。这会让 Rust 版在标准 Crostini 环境下**完全无法启动** | 去掉 `glamor` / `no_clipboard_manager` / `direct_scale` / `stable_scaling` / `x11` / `virtgpu_channel` 的 clap `env` 绑定，改由 `Config::from_args` 按主线字符串语义读取（`virtgpu_channel` 补上 `env_bool`）；新增 `mainline_string_envs_are_not_bound_to_bool_flags` 回归测试，直接断言这些 flag 不再绑 env |
| 2 | `-X` 启动 10 秒后 `Xwayland displayfd did not contain a display number`，Xwayland 报 `(EE) could not connect to wayland server`，X11 静默降级 | Xwayland 要先完成 Wayland 握手才往 `-displayfd` 写 display 号；原实现在 `proxy::run()` **之前**同步阻塞等 displayfd，而 Xwayland 的 `WAYLAND_SOCKET` fd 要等 `start_x11()` 返回后才交给 proxy，形成死锁。主线用单个 event loop 同时服务两者，不会死锁 | `main` 提前创建 `WAYLAND_SOCKET` socketpair，`wl_parent` 立即交给 `proxy::run()`（开头即 `spawn_client(..., true)`）；`start_x11(..., wl_child)` 移到独立 OS 线程（runtime 是 `current_thread`，不能阻塞 main task），成功后设置 `DISPLAY` 再启动 `runprog`；`X11Runtime` 不再回传 fd |
| 3 | 真机 `cargo test` 有 1 个失败：`preferred_scale_matches_mainline_round_wire_over_scale` 期望 180、实际 1800 | `Transform::new(0.0)` 会 clamp 到 `MIN_SCALE=0.1`，进不了 `preferred_scale()` 的退化分支，算出 `180/0.1`。Windows 上只能 `cargo check --tests`（不运行），所以此前从未暴露 | 测试改为直接置 `scale = 0.0` / `NaN` 来覆盖防御分支，并补一条断言 |

### 1.6 v0.3.6-r 补齐的主线能力

| 主线实现 | Rust 新增 | 对齐要点 |
|---|---|---|
| `compositor/sommelier-drm.cc`：`wl_drm` v2 本地模拟 | `handler/drm.rs` | `device(string)`、`format` ×5、`authenticated`、`capabilities(PRIME=1)` 全部本地回复；`create_prime_buffer` 转成内部 host `zwp_linux_dmabuf_v1` **v2** proxy 的 `create_params` → `add(fd, MOD_INVALID hi/lo)` → `create_immed` → `destroy`；多平面（offset1/stride1 非零）按主线拒绝。固定 bind v2 以规避 v4/v5 `invalid_format` 规则 |
| `sommelier-text-input.cc:548-697`：Crostini 私有 IME | `handler/text_input_crostini.rs` | `zcr_text_input_crostini_manager_v1` / `zcr_text_input_crostini_v1` / deprecated `zcr_text_input_x11_v1` 全部本地实现；核心是 XID → host `wl_surface` → host `zwp_text_input_v1.activate`。因 Rust 每 client 一条 host connection，非 Xwayland 连接无法解析 XID，降级为 placeholder `wl_surface`（见 §4） |
| `sommelier-output.cc:510-548`：`zaura_output` 监听器 | `OutputHandler` 的 `ZauraOutputHandler` | `scale`（CURRENT=0x1 / PREFERRED=0x2，线值 1/1000）、`connection`（INTERNAL=1）、`device_scale_factor` 本地消费；`insets` / `logical_transform` / `release` 按主线 DoNothing |
| `sommelier.cc:3541-3548` + `:3599-3603`：display-ready 后重发 output state | `resend_output_state()` | DSF 事件（在主线里是 display-ready）到达后重发 geometry / mode / scale / done，主线注释明确说明这是 Xwayland DPI 生效所需的 output update。**保留对 Xwayland 的重发**，未做「X11 不重发」的降级 |
| `sommelier-output.cc:649`：DSF 初值 1000 | `sync_transform()` | 未收到 aura 事件时缺省 **1.0**，不再用整数 `wl_output.scale`（真机上该值为 2，会算出 192 DPI 的错误首帧） |
| `sommelier.cc:759-775`：direct 模式内部 bind xdg-output | `create_output_extensions()` + `Transform::axis_factors()` | Direct 模式优先用 `virtual_width / logical_width`，无 xdg 数据时回退 `scale × DSF` |
| `sommelier.cc:3522` 的 serial 采集、`sl_set_selection` 的 unchecked 请求 | `proxy.rs` / `x11/wm.rs` / `x11/selection.rs` | 扩充 pointer/keyboard/touch/data_device 的 serial opcode；`ChangeWindowAttributes` 改 unchecked 并对 `BadWindow` 降到 debug；`host_serial == 0` 不再告警（主线初值同为 0 且照常发 `set_selection`） |

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

`v0.3.6-r` 已把 §4 中的 `wl_drm` 本地模拟、Crostini IME 协议、
`zaura_output` / `zxdg_output_v1` 内部订阅、`host_serial == 0` 告警与
`BadWindow` 竞态 5 类差距补齐（见 §1.6）。以下为**复查后仍然成立**的差距。

| 项 | 主线 | Rust 现状 | 原因 |
|---|---|---|---|
| **X11 screen DPI** | `:0` 的 `xdpyinfo` 为 `1920x1280 (406x270mm)` / 120 DPI | `:9` 的 `xrandr` monitor 已是主线同款 `406x270mm`，但 `xdpyinfo` X screen 仍为 `508x339mm` / 96 DPI | Xwayland `update_screen_size()` 用 `width × 25.4 / monitorResolution` 计算 screen mm，该值在 X server 启动时固定。真机三项实验均证明代理侧无法改变：① 让首帧即以 DSF=1.25 发 geometry；② Xwayland 启动加 `-dpi 120`；③ 运行时 `xrandr --dpi 120` / `--fb --dpi`。根因是本项目的**每 guest client 独立 host 连接**：host 输出握手晚于 Xwayland 初始化，主线共享 host display 则在启动 Xwayland 前已完成。影响仅限读取 `Xft.dpi` 的老式 X11 应用 |
| `sl_calculate_scale_for_xwayland` 的完整重算 | Xwayland ready 后 `ctx.scale = desired_scale × DSF × preferred_scale`，并 round/clamp（`:3507-3549`） | 已消费 `scale` / `preferred_scale` / DSF 并重发 output state，但未把三者完整乘回 `ctx.scale` | 需要把 internal output 的 `preferred_scale` 与 DSF 一并注入 `Transform`；当前 `wp_viewporter` 分支只应用 `desired_scale` |
| `wl_output.mode` 宽高公式 | `host_w × ctx.scale × s / applied_scale`（含 aura_shell 的 `applied_scale`） | `trunc(host_w × ctx.scale)` | 两者算出的**逻辑尺寸**一致（Rust 用整数 `wl_output.scale` 承载差值），但 wire 上的 size/scale 数值不同 |
| Crostini IME 的 XID → surface 解析 | 共享 host display，可直接找到 Xwayland 的 host `wl_surface` | 非 Xwayland 连接走内部创建的 placeholder `wl_surface` | 每 client 独立 host connection 导致 XID 关联表不在同一连接上；真实 ChromeOS host 侧人工 IME 输入尚未验证 |
| `--viewport-resize` | 在 surface commit 时用 `window->viewport_width/height` 覆盖 host viewport destination | 在 guest `wp_viewport.set_destination` 上按 scale 换算 | 实现路径不同，效果接近；未复刻 `viewport_pointer_scale` 指针补偿 |
| `zaura_shell` → `zwp_tablet_manager_v2`（stylus） | stylus-v2 协议转换 | 透传 tablet-v2 | 主线做的是协议转换，Rust 保留直接透传 |
| `--peer-pid` / `--client-fd` / `--peer-cmd-prefix` | parent 内部 fork worker 时传 client pid 与 fd，用于 client 身份 / app_id 推断（`:3962-4022`） | 只解析、不实现；parent 换成我们之后这两个参数自然消失 | **不是替换阻塞项**：真机 grep 容器脚本与 systemd 单元无外部 caller，仅主线 parent 自调用；早期文档把它列为阻塞项属误判，已在 [`REPLACE_MAINLINE.md`](REPLACE_MAINLINE.md) §2 更正。实际代价是缺少 peer pid → app_id 关联 |
| `--timing-filename` / `--stats-*` / `--trace-*` | Perfetto / 帧统计 | 只解析、不实现 | 可观测性功能，不影响协议兼容 |
| INCR 接收侧缓冲策略 | 必须先缓冲 64 KiB 再决定 one-shot / INCR（`:2219-2235`） | 采用预取缓存 + 按需 ConvertSelection | 阈值与 chunk 已对齐，驱动方式不同 |
| 真实 Crostini 端到端 | — | **已在 Debian 13 / `penguin` / `/dev/wl0` 真机验证**（见 §5），Wayland 与 X11 两条主链路均打通，`wl_drm` / IME wire 探针 PASS | 仍未覆盖：ChromeOS **host 侧**粘贴/复制与 IME 的人工确认、**把 `wayland-0` / `:0` 整套 drop-in 切换一整轮**、长时间稳定性与真实 GPU 加速 |

### 4.1 替换就绪度（`v0.3.6-r` 复查）

用户问「距离替代主线还差多少」时，答案是：**协议与功能面差得不多了，
剩下的主要是「一次生产切换演练」与「几项必须由人确认的验证」，而不是
继续补功能**。完整操作手册见 [`REPLACE_MAINLINE.md`](REPLACE_MAINLINE.md)。

真机复查推翻了一个此前的误判：`--peer-pid` / `--client-fd` 由主线 parent
**内部** fork worker 时生成（`sommelier.cc:3962-4022`），容器内无外部 caller；
把 parent 一并换掉后这两个参数自然消失，不再是阻塞项。

| 维度 | 结论 | 证据 |
|---|---|---|
| 主线 4 个 unit 的 `ExecStart` 参数 | ✅ 全部可解析 | `--parent`、`--sd-notify="READY=1"`、`--no-exit-with-child`、`--x-auth`、`--enable-xshape`、`--stable-scaling`、`--enable-linux-dmabuf`、尾随 `/bin/sh -c ...`；`config.rs` 的 `trailing_var_arg` 复刻了主线「首个位置参数后停止解析」 |
| `--parent` 多客户端 | ✅ 真机 PASS | `proxy.rs:1144` bind + `:1165` `accept()` 循环 + `spawn_client`；隔离 socket `wayland-wbpar` 上两个独立 wire 探针先后连接均 PASS（26 globals、`wl_drm` 完整事件、IME bind OK） |
| 尾随命令链 | ✅ 可执行 | `main.rs:70` `spawn_runprog`；`xwayland.rs:308` 先 set `DISPLAY`，`main.rs:205-207` 再启动 runprog，主线 `xauth` / `. /etc/sommelierrc` / `set-environment` 能拿到正确值 |
| `Type=notify` | ✅ | `proxy.rs:1153` 在 listener bind 后发 `READY=1`；真机共存单元秒级 `active` |
| 用户 `SOMMELIER_ACCELERATORS` override | ✅ 保留 | `state.rs:505` 读环境变量；drop-in 与用户 `override.conf` 共存 |
| 结构差异（需注意） | ⚠️ | 主线是 parent + X11 两进程，我们单进程同时管 socket 与 Xwayland；`--socket` 默认 `wayland-0` 且每实例都 bind，两个 unit 都换时必须给 X11 那个显式改名，或改用单实例方案 |
| 完整 drop-in 切换演练 | ❌ 未执行 | 会中断当前桌面上的 Linux 应用；需用户确认后按手册 §6→§8→§9 跑一整轮 |

---

## 5. 验证

### 5.1 开发机（Windows）

- `cargo check --target x86_64-unknown-linux-gnu --workspace --tests` — 通过，0 error / 0 代码 warning
- `cargo fmt --all -- --check` — 通过
- **CLI 形状实测**：把 `Args` 定义抽到独立 clap crate 在 Windows 本机**实际运行**，
  5 组断言全部通过 —— `--socket` vs `--display`、位置参数 runprog、`--`
  分隔、`--sd-notify` 不吃位置参数、`--scale=1.5junk` / `--dpi=96.5,nonsense`
  宽松解析、全量主线 flag 拼写。
- 开发机（Windows）当时只能 `cargo check --tests`（无 Linux linker），无法运行
  Linux 测试二进制，这正是 §1.5 第 3 项能漏到真机的原因；该缺口已由 §5.2 的
  真机测试与 Debian CI 补齐。

### 5.2 真实 Crostini 环境

环境：FydeOS / ChromeOS 的 `penguin` 容器，`Debian GNU/Linux 13 (trixie)`，
内核 `6.6.119`，`/dev/wl0` + `/dev/dri/renderD128`，现役主线 sommelier `0.20`
共 4 个实例（`sommelier@0/1` parent、`sommelier-x@0/1` X11）。
工具链 `rustc/cargo 1.99.1`、`rustfmt 1.10.0`。

测试全程使用独立 `--socket=wayland-wb` 与独立 `--x-display=9`，**未触碰现役
`wayland-0/1`、`:0/:1` 或用户的 `~/.Xauthority`**。

- `cargo test --workspace --no-fail-fast` — **143 个测试全部通过**（139 + 4 + 0 doc-tests）
- `cargo build --workspace` / `cargo fmt --all -- --check` — 通过

**Wayland 链路**（纯 Python wire 客户端，不依赖任何 toolkit）：

- guest socket `/run/user/1000/wayland-wb` 创建成功；`wl_display.get_registry`
  + `sync` 返回 **22 个 global、0 协议错误**；name 不连续（1,2,4,7,11…）证明来自 host virtwl 真实转发
- 版本钳制全部命中：`wl_compositor v4`、`wl_output v3`、`wl_seat v5`、
  `wl_data_device_manager v3`、`xdg_wm_base v3`、`zcr_text_input_extension_v1 v11`
- `zxdg_output_manager_v1` 未公布；`gtk_shell1 v1` 与 `zwp_text_input_manager_v3`
  两个 synthetic global 正常出现
- 绑定 5 个 global + `create_surface` + `get_data_device` 全部成功，proxy 日志零警告
- host 事件完整转发并改写：`wl_output.geometry x=0 y=0 phys=280x187`、
  `mode 1920x1280@60000`、`scale 2`、`done`、`wl_seat.capabilities 7`

**X11 链路**（`SOMMELIER_GLAMOR=1`，与现役主线同款环境）：

- Xwayland 1.20.14 在 `:9` 就绪，`xdpyinfo` 返回 `1920x1280 (280x187mm)`
- `xrandr` 显示 `XWAYLAND0 connected 1920x1280+0+0 ... 280mm x 187mm`，RandR 模拟生效
- WM 初始化：`_NET_SUPPORTING_WM_CHECK=0x200000`，wm window 的
  `_NET_WM_NAME="Sommelier"`，`_NET_SUPPORTED` 前 8 个 atom 顺序与主线逐项一致
- 真实 `xmessage` 窗口被接管（`_NET_FRAME_EXTENTS=1,1,1,1`），proxy 日志
  `X11 window 400020 -> host surface 50 / xdg_toplevel 53`，键盘 `on_enter/on_leave` 焦点正常
- 剪贴板 X11 → X11 往返 `readback=wb-e2e-clip-7788`；TARGETS 为
  `TARGETS` + `UTF8_STRING`（主线 1:1 原样透传）
- 同一时刻 guest Wayland 客户端收到 `wl_data_device.data_offer` 与 `selection`，
  offer 的 MIME 即 `UTF8_STRING` 原名 —— **X11 → guest 剪贴板桥接真机打通**
- X11 模式下普通 guest Wayland 客户端仍能看到 `wl_data_device_manager` /
  `xdg_wm_base`，只有 Xwayland 连接被过滤，符合主线

### 5.3 回归测试覆盖

`--socket` vs `--display` 语义、位置参数 runprog、主线 flag 解析、
`--scale` 取整与 `atof`/`atoi` 语义、`--fullscreen-mode` 校验、
**字符串 env 不再绑定 bool flag**、MIME 1:1 与别名、`normalize_targets` 顺序、
`gtk_shell1` 桥接、`virtual_x_for_host_x()`、`preferred_scale` 退化分支。

### 5.4 Debian 包交付（v0.3.4-r）

`packaging/build-deb.sh` 在同一台 Crostini 真机上执行
`cargo build --release --workspace --locked`（首次 53 s）→ strip →
`dpkg-shlibdeps` 解析运行时依赖 → `dpkg-deb --build --root-owner-group -Zxz`。
产物 `sommelier-rs-r_0.3.4-r_amd64.deb`（约 1.3 MiB），依赖解析为
`libc6 (>= 2.39)`, `libgbm1`, `libgcc-s1`, `libxkbcommon0`（`libdrm2` /
`libexpat1` 由 `libgbm1` 传递引入）。

- `sudo dpkg -i` 实机安装通过；`/usr/bin/sommelier-rs-r --version` = `0.3.4-r`；
  安装前后现役 `sommelier@0/1` 始终 `active`，`wayland-0/1` 与 `:0/:1` 未变
- 随包两个 systemd user 模板实测：`sommelier-rs-r@wayland-wb.service` 与
  `sommelier-rs-r-x@wayland-wbx.service` 均秒级 `active`；前者 registry
  22 globals / 0 错误，后者 `Xwayland ready` + `WM initialized` +
  `1920x1280` + 剪贴板往返成功；停止后测试单元 `inactive`、现役单元仍 `active`
- 143 个单元测试在 `0.3.4-r` 树上再次全绿，`cargo fmt --all -- --check` 通过

**打包过程本身又暴露并修掉 3 个只有真机才能发现的问题**：

| # | 现象 | 根因 / 修复 |
|---|---|---|
| 1 | 脚本读不出版本并 die | `sommelier/Cargo.toml` 是历史 CRLF 文件，`sed 's/^version = "\(.*\)"$/\1/p'` 因行尾 `\r` 不匹配 → 改为 `'s/^version = "\([^"]*\)".*/\1/p'` |
| 2 | 包内目录权限是 `drwxrwxr-x` | `dpkg-deb` 原样记录 staging 目录的 umask 权限 → 打包前 `find "$STAGE" -type d -exec chmod 0755 {} +` |
| 3 | `systemctl --user start` 卡 90 s 超时、服务被 kill（`activating (auto-restart) (Result: timeout)`），但进程实际已在工作 | 单元是 `Type=notify`，而 `proxy::run` 只在 `cfg.sd_notify` 为真时发 `READY=1`；改写单元时漏了裸 `--sd-notify` → 两个 `ExecStart` 补回该参数，并在 `CROSTINI_INSTALL.md` §5 / §11 明确写出「覆盖 `ExecStart` 必须保留 `--sd-notify`」 |

安装、共存、systemd、参数速查、故障排查与卸载见
[`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md)。

### 5.5 v0.3.5-r 清理后的回归验证

历史包袱清理（删除死文件、未用依赖 `drm`、零调用符号与半接线的
`WmEvent::Shutdown` 分支，统一 CRLF→LF，精简文档、整改 release workflow）
在同一台 Crostini 真机上重跑：

- `cargo fmt --all -- --check` 通过；`cargo test --workspace --locked`
  143 个测试（139 + 4）全绿；`cargo build --release --workspace --locked`
  **0 warning**（此前被 `#[allow(dead_code)]` 掩盖的 9 个警告，按「主线变换
  API 面」与「bridge getter/setter 对称」两类分别改为带说明的局部 allow，
  未删除任何主线能力）。
- `packaging/build-deb.sh` 重建 `sommelier-rs-r_0.3.5-r_amd64.deb`
  （约 1.3 MiB），`dpkg-shlibdeps` 正常解析出 `libc6 (>= 2.39)`、
  `libgbm1`、`libgcc-s1`、`libxkbcommon0`，未走兜底；包内目录权限统一 0755，
  当时 7 份 `docs/*.md` 全部随包分发（`v0.3.6-r` 起为 8 份，新增
  `REPLACE_MAINLINE.md`）。
- `sudo dpkg -i` 从 `0.3.4-r` 原地升级到 `0.3.5-r`；`--help` 中主线兼容参数
  （`--peer-pid` / `--client-fd` / `--timing-filename` / `--stats-summary` /
  `--trace-filename` 等）仍全部可解析。
- 两个 systemd user 模板再次秒级 `active`：Wayland registry 探针的
  bind / surface / data_device / host 事件全部 PASS、0 协议错误；X11 `:9`
  的 `Xwayland ready` + `WM initialized` + 真实窗口映射为 host
  `xdg_toplevel` + RandR 1920x1280 + 剪贴板往返 `wb-e2e-clip-7788` 均通过。
- 停止后测试单元 `inactive`、无孤儿 `:9` Xwayland / xclip owner，
  现役 `sommelier@0/1` 始终 `active`。

§5.4 表格第 1 项的 CRLF 兼容补丁已随之移除：历史 CRLF 文件统一为 LF，
并用 `.gitattributes` 固定，`build-deb.sh` 恢复严格版本匹配。

### 5.6 v0.3.6-r 功能补齐与回归验证

SSH 开发机重启后重新连通（同一台 `penguin`），以下结果均在该真机上取得。
测试仍使用独立 `--socket=wayland-wbx` / `--x-display=9`，现役
`wayland-0/1`、`:0/:1` 未受影响。

- `cargo fmt --all -- --check` 通过；`cargo test --workspace --locked`
  **154 个测试全绿**（150 + 4 + 0 doc-tests）；`cargo build --release --locked`
  **0 warning**。Windows 开发机侧 `cargo check --workspace --tests
  --target x86_64-unknown-linux-gnu` 同样通过。
- `packaging/build-deb.sh` 产出
  `sommelier-rs-r_0.3.6-r_amd64.deb`（约 1.3 MiB），`sudo dpkg -i` 安装后
  `sommelier-rs-r --version` = `0.3.6-r`。
- **新增 wire 探针**（纯 Python、无 toolkit 依赖）连到 guest socket：
  `TOTAL_GLOBALS 25`；`wl_drm` v2 可 bind 且依次收到
  `device=/dev/dri/renderD128`、5 个 `format`、`capabilities=1`、
  `authenticated=1`；`zcr_text_input_crostini_manager_v1` 与
  `zcr_text_input_x11_v1` 均可 bind；全程无 `wl_display.error` →
  `IME_BINDS_OK True` / `RESULT PASS`。
- **X11 链路**：`sommelier-rs-r-x@wayland-wbx.service` 秒级 `active`；
  `Xwayland :9` 命令行不再带 `-shm`；新启动日志中
  `glamor: 'wl_drm' not supported` 与 `falling back to sw` 均已消失；
  WM 初始化、真实窗口映射到 host `xdg_toplevel`、剪贴板往返
  `readback=wb-e2e-clip-7788` / `TARGETS` = `TARGETS` + `UTF8_STRING` 保持通过。
- **`wl_drm` 真实调用**：不只是 probe 能 bind —— 真机 Xwayland 每次映射
  `xmessage` 窗口都会走 `wl_drm.create_prime_buffer 202x62
  fmt=0x34325258 stride=808 -> host buffer NN`，即本地模拟的 PRIME 路径
  确实被 glamor 用来分配窗口缓冲，而非回退软件渲染。
- 最终 `.deb` 安装后按 §11 的排查表逐项扫描，legacy 错误关键字
  （`not supported` / `falling back to sw` / `select client window events failed`
  / `set_selection` serial warning）命中数 **0**；现役 4 个主线单元
  `sommelier@0/1`、`sommelier-x@0/1` 仍全部 `active`，`:0/:1` 几何未变。
- **输出 / DPI**：host 确实下发 `zaura_output.device_scale_factor=1250`
  （→ 1.25），`resend_output_state()` 按 120 DPI 算出 `406x270mm`；
  `xrandr` :9 显示 `XWAYLAND0 ... 406mm x 270mm`，与主线 `:0` 完全一致。
  `xdpyinfo` :9 仍为 `508x339mm` / 96 DPI，见 §4 首行：这是 Xwayland
  启动期 `monitorResolution` 决定的 X screen 值，三项真机实验证明代理侧
  无法改变，已如实记录为已知差距。
- 日志洁净度：不再出现 `BadWindow` 的 `select client window events failed`
  warning（改 unchecked + debug 降级），也不再出现 `host_serial == 0`
  的 `set_selection` warning（与主线一致，降为 debug）。
