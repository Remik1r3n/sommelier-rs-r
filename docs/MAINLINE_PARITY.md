# 与主线 C++ `sommelier` 的一致性

本文档描述 **当前实现** 与主线 C++ `sommelier`（`vm_tools/sommelier/`）
的对齐情况：哪些行为一致、哪些是有意保留的增强、哪些仍有差距。
行号引用均指主线 C++ 源码。

结论：**CLI / 环境变量、global 版本、剪贴板 MIME、X11 启动与 app_id、
输出几何、INCR 阈值等语义与主线一致**；`wl_drm` v2 本地模拟、Crostini 私有
IME 协议、`zaura_output` / `zxdg_output_v1` 内部订阅、Direct 模式真实 logical
size 换算、可观测性参数与 `--peer-pid` / `--client-fd` 均已实现。仍有差距的
项目集中在 §4，属于架构差异或未接入的可选路径。

---

## 1. 兼容基线

### 1.1 命令行与环境变量

| 参数 / 变量 | 当前行为（对齐主线） | 主线位置 |
|---|---|---|
| `--socket` | **guest 监听** socket 名，默认 `wayland-0`；`SOMMELIER_SOCKET` 为 env 别名 | `:4199`、`:4253` |
| `--display` / `SOMMELIER_DISPLAY` | 要连接的 **host** compositor；相对名按 `$XDG_RUNTIME_DIR` 解析并选择 unix transport | `:4176`、`:4255`、`:4464-4466` |
| 位置参数 | `[program] [args...]`；遇第一个位置参数即停止解析选项（`trailing_var_arg`） | `:4354-4364` |
| `--scale` | 非 direct 模式 `round()` 后 clamp 到 `[0.1,10]`；direct 模式取原值。检测到 host `wp_viewporter` 时去掉整数取整 | `:4403-4413`、`:644-657` |
| `--dpi` | `strtok` + `atoi`（`"96.5"→96`、非数字→0→clamp 72）；`--scale` 用 `atof` 且忽略尾部垃圾 | `:4519-4532`、`:4404` |
| `--fullscreen-mode` | 只接受 `immersive` / `plain`，否则退出 | `:4446-4456` |
| `SOMMELIER_GLAMOR` | **字符串**：只有 `"0"` 禁用；`-shm` 条件为 `!drm \|\| !glamor \|\| glamor=="0"` | `:4085-4089` |
| `SOMMELIER_CLIPBOARD_MANAGER` | **字符串**：仅字面 `"0"` 禁用；X11 模式默认开 | `:4394-4401` |
| `SOMMELIER_SUPPORT_DAMAGE_BUFFER` | 仅 `"1"` 或 unset 启用 | `:4182`、`:4437` |
| `SOMMELIER_XWAYLAND_GL_DRIVER_PATH` | flag + env 均支持 | `:4191` |
| `--sd-notify[=VALUE]` | `Option<String>`，`require_equals` 保证不吞位置参数；支持有值/无值两种写法 | `:4294` |
| `--enable-linux-dmabuf` | 映射到 `gpu_accel` | `:4318` |
| 其余主线 flag | `--application-id`、`--application-id-x11-property`、`--peer-pid`、`--client-fd`、`--peer-cmd-prefix`、`--timing-filename`、`--stats-*`、`--trace-*`、`--ignore-stateless-toplevel-configure`、`--only-client-can-exit-fullscreen`、`--windowed_accelerators` 全部按主线拼写接受 | — |
| `runprog` 环境 | X11 模式给子进程设 `WAYLAND_DISPLAY="."`；非 X11 模式 `unsetenv("DISPLAY")` | `:3630-3635`、`:4610-4613` |
| `--enable-xshape` | 默认 **false** | `sommelier-ctx.cc:210` |

命令行参数优先于环境变量。`SOMMELIER_GLAMOR`、`SOMMELIER_CLIPBOARD_MANAGER`、
`SOMMELIER_SUPPORT_DAMAGE_BUFFER` 等按字符串读取，**不绑定 clap 的 bool flag**
（`config.rs::Config::from_args`），因此 Crostini 注入的 `SOMMELIER_GLAMOR=1`
可以直接工作。

### 1.2 协议 global 与版本

主线 `sl_global_create` 是唯一的 guest global 发布入口，版本都经过 `MIN()` 钳制。
本项目的 `advertised_version()` / `host_bind_version()` 实现同一组规则：

| 接口 | guest 公布版本 | 主线位置 |
|---|---|---|
| `wl_compositor` | 固定 **4**（实现 `damage_buffer`）；`--no-support-damage-buffer` 时 **3**；host 侧恒 bind **3** | `compositor/sommelier-compositor.cc:1150-1191` |
| `wl_output` | `MIN(3, host)` | `:552` |
| `wl_seat` | `MIN(5, host)` | `:562` |
| `wl_data_device_manager` | `MIN(3, host)` | `:596` |
| `xdg_wm_base` | `MIN(3, host)` | `:615` |
| `zwp_linux_dmabuf_v1` | `MIN(4, host)`，默认不公布；`--enable-linux-dmabuf` 开启 | `:664-671` |
| `zcr_text_input_extension_v1` | `MIN(11, host)`，同时内部 bind 并向 guest 公布 | `:725` |
| `zwp_text_input_manager_v1` | 固定 1 | `:711` |
| `zaura_shell` | 内部 bind `MIN(38, host)`；host ≥ 6 时 guest 侧改发 **`gtk_shell1` v1** | `:629-643` |
| `zxdg_output_manager_v1` | **不向 guest 公布**，仅 `--direct-scale` 时内部 bind；`logical_*` 本地消费 | `:759-775` |
| `wl_drm` | host dmabuf ≥ 2 时公布 v2，由**本地模拟**实现；host 侧同名 global 列入 `INTERNAL_GLOBALS` | `compositor/sommelier-drm.cc:207-217` |
| `zcr_text_input_crostini_manager_v1` / `zcr_text_input_x11_v1` | 本地实现并公布 v1 | `sommelier-text-input.cc:548-697` |
| X11 client | **不公布** `wl_data_device_manager` / `xdg_wm_base`，只内部 bind | `:589-628` |

补充实现：

- **`gtk_shell1` 桥接**（`handler/gtk_shell.rs`）：`get_gtk_surface` →
  `zaura_shell.get_aura_surface`；`set_dbus_properties` →
  `zaura_surface.set_application_id("org.chromium.guest_os.<vm>.wayland.<id>")`；
  `set_startup_id` 缓存并重放；`system_bell` / `set_modal` / `unset_modal` /
  `present` 为 DoNothing。
- **text-input**：同时公布 v1（透明转发到 host）与 v3（映射到内部 host v1
  manager）。v3 是有意保留的增强，见 §2。
- **`wl_drm` v2 本地模拟**（`handler/drm.rs`）：`device(string)`、`format` ×5、
  `authenticated`、`capabilities(PRIME=1)` 全部本地回复；`create_prime_buffer`
  转成内部 host `zwp_linux_dmabuf_v1` **v2** proxy 的 `create_params` →
  `add(fd, MOD_INVALID hi/lo)` → `create_immed` → `destroy`；多平面按主线拒绝。
  固定 bind v2 以规避 v4/v5 `invalid_format` 规则。
- **Crostini 私有 IME**（`handler/text_input_crostini.rs`）：
  `zcr_text_input_crostini_manager_v1` / `zcr_text_input_crostini_v1` /
  deprecated `zcr_text_input_x11_v1` 全部本地实现；核心是 XID → host
  `wl_surface` → host `zwp_text_input_v1.activate`。

### 1.3 输出 / 坐标

| 项 | 当前行为 | 主线位置 |
|---|---|---|
| `wl_output.geometry` 位置 | `(virt_x, 0)`，`virt_x` 为左侧所有输出的虚拟宽度累加，**y 恒 0** | `sommelier-output.cc:283-295`、`:401` |
| X11 模式的 `wl_output.scale` | 强制 1 | `sommelier-output.cc:106-108` |
| `wp_fractional_scale.preferred_scale` | `round(wire_scale / ctx.scale)`，保留 1/120 精度 | `sommelier-fractional-scale.cc:42-44` |
| `wl_output.scale`（Wayland） | `MIN(ceil(applied_scale/ctx.scale), 2)` | — |
| `damage_buffer → damage` | 非 direct 分支 `±1` 外扩、min 角 trunc / max 角 ceil；direct 不外扩 | — |
| `--stable-scaling` 负坐标 | `ceil` / `floor`（非 trunc） | — |
| `device_scale_factor` 初值 | **1.0**，仅 aura 事件覆盖，不用 `wl_output.scale` 兜底 | `sommelier-output.cc:649` |
| X11 分支目标 DPI | `DEFACTO_DPI × device_scale_factor`；真实 DSF（1.25）→ 120 DPI → `406×270mm` | `:112` |
| display-ready 后的 output update | DSF 到达时 `resend_output_state()` 重发 geometry / mode / scale / done，**包含 Xwayland 连接** | `:3541-3548` |
| Direct 模式坐标换算 | 内部 xdg-output 的 `logical_size`，优先 `virtual_width / logical_width`，无数据时回退 `scale × DSF` | `:759-775` |
| `zaura_output` 事件 | `scale`（CURRENT=0x1 / PREFERRED=0x2，线值 1/1000）、`connection`（INTERNAL=1）、`device_scale_factor` 本地消费；`insets` / `logical_transform` / `release` DoNothing | `sommelier-output.cc:510-548` |

DSF 单独存于 `OutputGeometry`，**不折叠进 `Transform.scale`**，避免双重应用；
`output.rs::xwayland_wire_state_matches_mainline_formula` 锁定默认参数下
guest 侧 `wl_output` wire 与主线公式等价。

### 1.4 X11 / 剪贴板

| 项 | 当前行为 | 主线位置 |
|---|---|---|
| Xwayland 命令行 | `[:N] -nolisten tcp -rootless [-shm] -displayfd <fd> -wm <fd> [-auth] [-fp]`，**无 `-terminate`** | `:4029-4120` |
| atom 表 | 39 个，含 `_WL_SELECTION`（下划线）、`WL_SURFACE_ID` | — |
| MIME ↔ X11 target | **1:1 原样透传**；X11 侧只额外插入 `TARGETS` / `TIMESTAMP`，文本别名作为额外 TARGETS 项与预取候选 | `sl_get_selection_targets:2819-2841`、`sl_send_data:2931-2967`、`:354-371` |
| `TARGETS` 顺序 | `TARGETS`, `TIMESTAMP`, 然后 MIME 原名；`_WL_SELECTION` 不出现在 TARGETS | — |
| INCR | 阈值与 chunk 均 **64 KiB**；INCR property 写 **chunk size** 而非总长度 | `:2214`、`:2250-2257` |
| X11 app_id | `org.chromium.guest_os.<vm>.<wmclass\|xid\|wmclientleader\|xprop>.<id>`，`.wayland.` 只给原生 Wayland | `sommelier.h:25-27`、`sommelier-window.cc:29-35,1064-1097` |
| `_NET_SUPPORTED` | 前 8 个 atom 与主线逐字节一致，额外实现的 5 个追加在后 | `:3157-3175` |
| clipboard manager | X11 模式默认开，创建 `CLIPBOARD_MANAGER` owner + XFixes 监听 | `:3362-3379` |
| `ChangeWindowAttributes` | unchecked 请求，`BadWindow` 按 debug 记录 | — |
| `host_serial == 0` | 按主线语义照常发送 `set_selection`，不告警 | — |

### 1.5 进程与启动

- `sl_spawn_xwayland()` 的参数顺序、`WAYLAND_SOCKET` / `SOMMELIER_VERSION` /
  `LIBGL_DRIVERS_PATH` 环境变量、三个 socketpair fd 传递。
- `-displayfd` 回传后 `setenv("DISPLAY")`、`XCURSOR_SIZE = 24*scale`；
  Xwayland 启动顺序由 `main` 提前建好 socketpair、`wl_parent` 立即交给
  `proxy::run()`，`start_x11(..., wl_child)` 放在独立 OS 线程，避免握手死锁。
- WM 初始化顺序：SubstructureRedirect → XFixes(≥5) / Composite → WM 窗口 →
  `_NET_SUPPORTING_WM_CHECK` / `_NET_WM_NAME` → `_NET_SUPPORTED` → `WM_S0` owner。
- `--sd-notify` → `READY=1`、`--no-exit-with-child`、`--parent` 的进程语义。
- `--peer-pid` / `--client-fd`：主线 parent 在 `accept()` → `SO_PEERCRED` →
  fork worker 时生成（`:3962-4022`），**不是外部调用契约**。本项目
  `accept()` 时同样取 `SO_PEERCRED` 得 pid（显式 `--peer-pid` 优先），注入
  `Context.client_pid` 并写入连接日志；`--client-fd` 作为首个预连接 client 服务。
- `--timing-filename` / `--stats-*` / `--trace-*`（`stats.rs`）：CSV 采样、
  实时摘要、逐消息 trace；全进程共享一个 `Stats`。不引入 Perfetto 依赖。
- `--dpi` 桶 `[72,9600]`、就近选择且平局取靠前、`dpi_to_physical_mm = px*25.4/dpi`；
  `MIN_SCALE/MAX_SCALE = 0.1/10`、`MAX_OUTPUT_SCALE = 2`、`DEFACTO_DPI = 96`；
  三坐标系的 trunc、stable ceil/floor、`wl_fixed` 24.8 换算与旋转处理。

---

## 2. 有意保留的增强

| 项 | 与主线的差别 |
|---|---|
| `zwp_text_input_manager_v3` | 主线只有 v1 + zcr 扩展；本项目同时公布 v1 和 v3 |
| `wp_fractional_scale_v1`、新版协议 XML | 33 个 XML，主线 `protocol/` 只有 20 个 |
| 统一 transport `auto\|virtwl\|virtgpu\|unix` | 主线是编译期/启动期二选一；本项目三种后端可运行时回退 |
| 单进程多 client | 主线 `--parent` 为每个 client fork 一个 sommelier；本项目用单进程 task |
| `SOMMELIER_VERSION` | 取 crate 版本，主线固定 `0.20` |
| `--transport` / `--virtio-wl` / `--local-compositor` / `SOMMELIER_SOCKET` | 本项目扩展，不覆盖任何主线参数 |
| `TARGETS` 末尾的 `UTF8_STRING`/`STRING`/`TEXT` 别名 | 扩大 X11 客户端兼容性，不改变导出到 host 的 MIME |
| 预取时优先 `UTF8_STRING` | 读取方向的宽松匹配，只在 X11 owner 一侧生效；host 侧 MIME 名仍是 1:1 |
| `_NET_SUPPORTED` 追加的 5 个 atom | 对应确实读取的 `_NET_WM_WINDOW_TYPE` / `_NET_WM_PID` / `_NET_FRAME_EXTENTS` 等 |

---

## 3. 当前验证状态

程序在 Debian 13 / `penguin` / `/dev/wl0` 的 Crostini 真机上完成端到端验证：

- `cargo test --workspace --locked` **160 个测试全绿**（156 + 4 + 0 doc-tests）；
  `cargo build --release --workspace --locked` **0 warning**；
  `cargo fmt --all -- --check` 通过；Windows 开发机交叉 `cargo check --workspace
  --tests --target x86_64-unknown-linux-gnu` 通过。
- guest registry 25~26 个 globals、0 协议错误；版本钳制全部命中；
  `wl_drm` v2 的 `device` / `format` / `capabilities` / `authenticated` 与两个
  Crostini IME manager 的 wire 探针全部 PASS。
- Xwayland `:9` 启动、WM 管理真实 X11 窗口并映射为 host `xdg_toplevel`、
  剪贴板往返、RandR monitor `406mm x 270mm`、真实 Xwayland 调用
  `wl_drm.create_prime_buffer` 分配窗口缓冲。
- `wayland-0` / `:0` 的整套 drop-in 切换流程在保留 `@1` 主线的灰度配置下
  跑通并自动回滚；回滚后主线单元与环境变量恢复原状。操作手册见
  [`REPLACE_MAINLINE.md`](REPLACE_MAINLINE.md)。
- `.deb` 在真机 `dpkg -i` 安装、共存单元秒级 `active`、主线 `:0` 几何未变。

**仍需人工确认**（代码无法自证）：ChromeOS host 侧的复制粘贴与 IME 输入、
连续数天的稳定性、真实 GPU 加速路径、替换态下注销重登后的长期自启。

---

## 4. 已知差距

| 项 | 主线 | 本项目现状 | 原因 / 影响 |
|---|---|---|---|
| **X11 screen DPI** | `:0` 的 `xdpyinfo` 为 `1920x1280 (406x270mm)` / 120 DPI | `xrandr` monitor 已是主线同款 `406x270mm`，但 `xdpyinfo` X screen 为 `508x339mm` / 96 DPI | Xwayland `update_screen_size()` 用 `width × 25.4 / monitorResolution` 计算 screen mm，该值在 X server 启动时固定。首帧 seeding DSF、`-dpi 120`、运行时 `xrandr --dpi 120` 均无法从代理侧改变。根因是「每 guest client 独立 host 连接」使 host 输出握手晚于 Xwayland 初始化；主线共享 host display 在启动前已完成。影响仅限读 `Xft.dpi` 的老式 X11 应用，可用 `Xft.dpi: 120` 兜底 |
| `sl_calculate_scale_for_xwayland` 的完整重算 | Xwayland ready 后 `ctx.scale = desired_scale × DSF × preferred_scale`（`:3507-3549`） | 不折叠 DSF 进 `Transform.scale`；`host_to_guest_size = host_w × desired_scale` | 主线把 DSF 乘进 `ctx.scale` 后又在 `sl_output_get_host_output_state` 里用 `applied_scale = DSF × current_scale` 除掉，只要 `preferred_scale == current_scale` 两者就化简为同一个公式；硬套主线公式会双重应用 DSF。默认参数下 wire 等价已由回归测试锁定。**剩余差异**：用户 zoom 使 `preferred_scale ≠ current_scale` 时，主线会额外乘 `preferred/current`，本项目不会 |
| `wl_output.mode` 宽高公式 | `host_w × ctx.scale × s / applied_scale` | `trunc(host_w × desired_scale)`，X11 client 强制整数 scale=1 | 默认参数下 guest 逻辑尺寸与 scale=1 一致。差异在普通 Wayland client 的 wire `wl_output.scale`；整体重构坐标基准会影响所有已验证的换算，收益低于回归风险 |
| Crostini IME 的 XID → surface 解析 | 共享 host display，可直接找到 Xwayland 的 host `wl_surface` | 非 Xwayland 连接走内部创建的 placeholder `wl_surface` | 每 client 独立 host connection 导致 XID 关联表不在同一连接上；ChromeOS host 侧人工 IME 输入尚未验证 |
| `wl_pointer` 坐标变换 / `viewport_pointer_scale` | `sl_transform_pointer`（`sommelier-transform.cc:302-315`）先做 host→guest fixed 变换，再对 `viewport_override` 窗口乘 `viewport_pointer_scale` | `wl_pointer` 事件纯转发；`Transform::host_to_guest_fixed` 只在单测中调用；viewport 只换算 `set_destination` | 补偿依赖 surface↔window↔viewport_override 关联与主坐标变换链，未做半接入。`--viewport-resize` 默认关闭，已验证路径不受影响 |
| stylus 协议 | `zaura_shell` → `zwp_tablet_manager_v2` 的 stylus-v2 协议转换 | 透传 tablet-v2 | 主线做协议转换，本项目保留直接透传 |
| INCR 接收侧缓冲策略 | 必须先缓冲 64 KiB 再决定 one-shot / INCR（`:2219-2235`） | 预取缓存 + 按需 `ConvertSelection` | 阈值与 chunk 已对齐，驱动方式不同 |
| explicit fence / seccomp | 已实现 | 未接入 | 可选路径，不影响默认功能 |
| 帧统计格式 | Perfetto | CSV / 文本 / 逐消息 trace | 不引入 Perfetto 依赖 |
| `--parent` 进程隔离 | 每 client fork worker 进程 | 同一 runtime 内 per-client task | 多客户端能力等价，隔离性弱于主线 |
| `--peer-cmd-prefix` | parent fork worker 时的命令前缀 | 只解析，不执行 | 本项目 parent 不 fork，无需命令前缀 |

### 4.1 替换就绪度

协议与功能面已具备替换条件；真正剩下的不是继续补功能，而是「必须由人在
ChromeOS 侧确认」的验证与「长时间运行」两类（见 §3 末）。操作手册见
[`REPLACE_MAINLINE.md`](REPLACE_MAINLINE.md)。

| 维度 | 状态 |
|---|---|
| 主线 4 个 unit 的 `ExecStart` 参数 | ✅ 全部可解析：`--parent`、`--sd-notify="READY=1"`、`--no-exit-with-child`、`--x-auth`、`--enable-xshape`、`--stable-scaling`、`--enable-linux-dmabuf`、尾随 `/bin/sh -c ...` |
| `--parent` 多客户端 | ✅ `UnixListener` + `accept()` 循环 + per-client task；隔离 socket 上两个独立 wire 探针先后连接均 PASS |
| 尾随命令链 | ✅ `spawn_runprog` 在 `DISPLAY` 设置后执行；`xauth` / `. /etc/sommelierrc` / `set-environment` 能拿到正确值 |
| `Type=notify` | ✅ listener bind 后发 `READY=1` |
| 用户 `SOMMELIER_ACCELERATORS` override | ✅ 读环境变量，drop-in 与用户 `override.conf` 共存 |
| 结构差异 | ⚠️ 主线是 parent + X11 两进程，本项目单进程同时管 socket 与 Xwayland；两个 unit 都换时必须给 X11 那个显式改名，或改用单实例方案 |
