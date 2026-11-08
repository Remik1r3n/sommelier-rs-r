# sommelier-rs-r

面向 ChromeOS Crostini 的 Wayland 代理，用 Rust 实现。它补回主线 C++
`sommelier` 拥有、而上游 `sommelier-rs` 缺失的能力，其中最重要的是
**X11 / XWayland 转发**，同时保留现代协议能力（text-input-v3、
fractional-scale、ChromeOS 专有扩展等）。

目标是让它在 Crostini 里可以替代主线 `sommelier`，并且**默认与主线共存、
不改动现役桌面**。

---

## 在 Crostini 里安装

> **不知道选哪种？** 打开 [`docs/CROSTINI_INSTALL.md`](docs/CROSTINI_INSTALL.md) §1，
> 里面有 30 秒决策树、安装方式与运行方式的利弊对照，以及每种方案的可复制步骤。

最快路径（Debian 13 / x86_64 容器）：

```bash
sudo dpkg -i sommelier-rs-r_0.3.7-r_amd64.deb
sommelier-rs-r --socket=wayland-rs -- your-wayland-app
```

装包只是把 `/usr/bin/sommelier-rs-r` 和两个**默认禁用**的 systemd user 模板
放进系统。**不启用任何服务、不替换主线、不动 `wayland-0/1` 与 `:0/:1`**。
装完可以一直放着不用，也可以按需选一种使用方式：

| 想做什么 | 怎么做 |
|---|---|
| 只装、暂时不配置 | 什么都不做，见[安装指南 §6.1](docs/CROSTINI_INSTALL.md#61-方式-0只装不启用默认) |
| 临时试用一次 | [安装指南 §6.2](docs/CROSTINI_INSTALL.md#62-方式-1临时前台跑一次最快试用) |
| 长期共存、可开机自启 | [安装指南 §6.3](docs/CROSTINI_INSTALL.md#63-方式-2systemd-user-服务常驻长期共存) |
| 完整替换主线 `wayland-0` / `:0` | [替换手册](docs/REPLACE_MAINLINE.md)（可灰度、可一键回滚） |
| 系统不是 Debian 13 / 想改代码 | [安装指南 §4 重建 `.deb`](docs/CROSTINI_INSTALL.md#4-方案-b在容器里从源码重建-deb-再安装) 或 [§5 源码直接编译](docs/CROSTINI_INSTALL.md#5-方案-c源码直接编译不装包) |

校验发布产物：`SHA256SUMS` 与 `.deb` 放同一目录后执行 `sha256sum -c SHA256SUMS`。

---

## 主要能力

- **统一传输层**：`--transport=auto|virtwl|virtgpu|unix`。`auto` 优先 `/dev/wl0`，
  再尝试 virtio-gpu cross-domain，最后退回本地 compositor socket。
- **X11 转发**：启动 rootless Xwayland，通过 `-displayfd` / `-wm` / `WAYLAND_SOCKET`
  接管窗口管理；实现 `SubstructureRedirect`、Composite redirect、ICCCM/EWMH、
  `WM_S0`、`CLIPBOARD_MANAGER`、`WL_SURFACE_ID` 配对，并把 XID 映射到 host 侧
  `wl_surface` / `xdg_toplevel` / `zaura_surface`。
- **缩放与坐标变换**：`--scale` / `--direct-scale` / `--stable-scaling` / `--dpi`，
  `wl_output` / `zxdg_output_v1` / `wp_fractional_scale_v1` 事件按虚拟坐标系重写；
  `--viewport-resize` 换算 `wp_viewport.set_destination`，
  `--no-support-damage-buffer` 把 `wl_compositor` 降到 v3 并改写 `damage_buffer`。
- **X11 ↔ Wayland ↔ ChromeOS host 三向剪贴板**（对齐主线 C++ `sommelier`）：
  X11 `CLIPBOARD` owner 同时转成 guest 侧 `wl_data_offer` 与 host 侧
  `wl_data_source` + `set_selection`；host 的 `wl_data_source.send` 由
  `ConvertSelection` 异步回写 fd。反向把 host offer / guest data source 的 MIME
  与文本载荷预取给 X11 `SelectionRequest` 并夺回 owner；支持 `TARGETS`、`INCR`
  与延迟回复。详见 [`docs/CLIPBOARD.md`](docs/CLIPBOARD.md)。
- **X11 桌面细节**：`--frame-color` / `--dark-frame-color` 绘制边框并写
  `_NET_FRAME_EXTENTS`；`--allow-xwayland-emulate-screen-pos-size` 依据实时输出
  布局维护 `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`。
- **ChromeOS 集成**：`zaura_shell`（shelf app-id）、`zcr_keyboard_extension_v1`
  （系统快捷键 ack-key）、`zwp_text_input_v1` ↔ v3 互转、Crostini 私有 IME 协议、
  本地模拟 `wl_drm` v2、`SOMMELIER_ACCELERATORS`。
- **SHM 多路径降级**：virtwl alloc → virtgpu host blob → GBM BO → sealed memfd。
- **协议代码生成**：`third_party/protocols/*.xml`（33 个）由 `build.rs` 自动枚举，
  生成 `protocols::all` 聚合分发；新增协议只需放入一个 XML。
- **主线参数兼容**：clap + `SOMMELIER_*` 环境变量，命令行优先于环境变量。
- **可观测性**：`--timing-filename` / `--stats-*` 输出 CSV 采样与实时摘要，
  `--trace-*` 输出逐消息 trace。

## 参数语义（与主线一致）

- `--socket` = **guest 监听** socket 名（默认 `wayland-0`）；
- `--display` / `SOMMELIER_DISPLAY` = 要连接的 **host** Wayland display；
- 位置参数 `[program] [args...]` = 代理就绪后启动的子进程，主线在第一个
  位置参数处停止解析选项。

| 参数组 | 说明 |
|---|---|
| `--socket`, `--display`, `--transport`, `--virtio-wl`, `--local-compositor`, `--virtgpu-channel` | 监听与 host 通道 |
| `--force-drm-device` | 强制 GBM / virtgpu 使用的 render node |
| `-X`, `--x-display`, `--xwayland-path`, `--x-auth`, `--glamor` | X11 / Xwayland |
| `--scale`, `--direct-scale`, `--stable-scaling`, `--dpi`, `--viewport-resize` | 缩放 |
| `--accelerators`, `--windowed-accelerators`, `--vm-identifier` | ChromeOS 集成 |
| `--quirks-config`, `--print-enabled-features` | quirks 规则 |
| `--disable-xdg-decoration`, `--no-support-damage-buffer`, `--gpu-accel` | 协议开关 |
| `--parent`, `--no-exit-with-child`, `--sd-notify`, `--log-level` | 进程行为 |
| `--timing-filename`, `--stats-log`, `--stats-summary`, `--stats-timer`, `--trace-filename`, `--trace-system` | 可观测性 |

完整参数表：`sommelier-rs-r --help`。完整安装与排障见
[`docs/CROSTINI_INSTALL.md`](docs/CROSTINI_INSTALL.md)。

启用 `-X` 后进程会自动设置 `DISPLAY` / `XCURSOR_SIZE`，X11 应用直接可用。
Xwayland 默认路径为 `/opt/google/cros-containers/bin/Xwayland`，可用
`--xwayland-path` 覆盖。若 Xwayland 启动失败，代理记录错误并继续只提供
Wayland 服务，不会整体退出。

## 构建

依赖：Rust toolchain、`libxkbcommon-dev`、`libgbm-dev`、`libdrm-dev`、
`libexpat1-dev`。`x11rb` 是纯 Rust 实现，不需要额外的 X11 开发库。

```bash
cargo build --release --target x86_64-unknown-linux-gnu
cargo test  --target x86_64-unknown-linux-gnu
cargo check --tests --target x86_64-unknown-linux-gnu --workspace
```

打 Debian 包：

```bash
bash packaging/build-deb.sh        # 产物在 dist/
```

脚本必须在目标 Debian 系统上运行（用 `dpkg-shlibdeps` 按本机 shlibs 解析依赖）。
CI 与 release workflow 位于 `.github/workflows/`。

## 项目结构

```
sommelier/
  src/
    main.rs            统一 CLI / X11 启动 / 子进程与 sd_notify
    config.rs          参数 + SOMMELIER_* 环境变量 → Config
    proxy.rs           每客户端一条 host 连接，双向消息分发，X11 WM 命令注入
    state.rs           ShadowTable / Context / ID 分配
    stats.rs           可观测性采样与 trace
    connection.rs      Unix / VirtWl / VirtGpu 三种传输
    virtwl_channel.rs  /dev/wl0 ioctl
    virtgpu_channel.rs virtio-gpu cross-domain
    allocator.rs       GBM / sealed memfd
    transform.rs       三坐标系变换、DPI、wl_fixed
    quirks.rs          textproto-like 规则解析
    handler/           手写协议 handler（含 output.rs 缩放改写、drm.rs）
    x11/               Xwayland 启动、X11 WM、atom 表、剪贴板桥接、bridge
  build.rs             枚举 XML → 生成协议模块与聚合分发
wayland_codegen/       XML 解析 + Rust 代码生成 + aggregate 生成
third_party/protocols/ 33 个协议 XML
packaging/             build-deb.sh 与两个 systemd user 单元模板
docs/                  安装指南、替换手册、架构、功能矩阵、主线对照等
```

## 当前限制

详见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) §6 与
[`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §4。最需要注意的：

1. **ChromeOS host 侧的人工交互尚未验证**：Wayland 与 X11 两条主链路、
   `wayland-0` / `:0` 整套替换流程都已在真机跑通，但「X11 复制 → 在
   ChromeOS 界面里粘贴」与输入法打字需要人在 host 侧确认；长时间稳定性与
   真实 GPU 加速路径也未覆盖。
2. **实现路径差距**：`wl_pointer` 坐标变换 / `viewport_pointer_scale` 未接入
   生产路径、stylus-v2 协议转换、explicit fence / seccomp、INCR 接收侧驱动、
   Perfetto 原生 trace 格式，以及 `--parent` 用共享 runtime 的 per-client task
   而非 fork 子进程。
3. **X11 屏幕 DPI**：`xrandr` 的 RandR monitor 物理尺寸与主线一致
   （`406x270mm` / 120 DPI），但 `xdpyinfo` 的 X screen 是 `508x339mm` /
   96 DPI。该值由 Xwayland 启动时的 `monitorResolution` 固定，代理侧无法改变；
   「每 guest client 独立 host 连接」使 host 输出状态晚于 Xwayland 初始化。
   受影响的是读取 `Xft.dpi` 的老式 X11 应用，可用 `Xft.dpi: 120` 兜底。
4. **首次复制可能缺 serial**：`wl_data_device.set_selection` 的 serial 取自
   host 输入事件，进程刚启动尚未收到输入时为 0（主线 `last_serial` 初值同样
   为 0 且照常发送）。个别严格校验 serial 的 compositor 可能拒绝这一次导出。

**想把主线整个换掉？** 默认安装始终保持共存；
[`docs/REPLACE_MAINLINE.md`](docs/REPLACE_MAINLINE.md) 提供可灰度、可一键回滚的
替换手册。剩下需要人工确认的只有 ChromeOS host 侧交互与长期稳定性。

## 其他说明

本项目不是 Google 官方支持的产品，也不属于 ChromiumOS 组件，
不参与 [Google Open Source Software Vulnerability Rewards Program](https://bughunters.google.com/open-source-security)。

## 许可

Apache-2.0。
