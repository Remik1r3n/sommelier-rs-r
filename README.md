# sommelier-rs-r

面向 ChromeOS Crostini 的加强版 Wayland 代理。它在 `sommelier-rs` 两个分支
（`main` / `virtwl`）完整对比合并的基础上，补回主线 C++ `sommelier` 拥有、
而 `sommelier-rs` 缺失的能力，其中最重要的是 **X11 / XWayland 转发**。

目标是让它在 Crostini 里几乎可以在所有场景替代原来的主线 `sommelier`，
同时保留 `sommelier-rs` 的现代协议能力（text-input-v3、fractional-scale、
ChromeOS 专有扩展等）。

> 分支对比与逐文件合并决策见 [`docs/BRANCH_MERGE.md`](docs/BRANCH_MERGE.md)，
> 当前架构见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)，
> 功能完成度与已知限制见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md)，
> 剪贴板双向桥接与主线 C++ 实现的逐行对照见 [`docs/CLIPBOARD.md`](docs/CLIPBOARD.md)。

## 与上游的关系

| 实现 | 传输 | X11 | ChromeOS 专有协议 | 说明 |
|---|---|---|---|---|
| 主线 C++ `sommelier` | virtwl / virtgpu / X11 | ✅ | ✅ | 功能最全，C++ 维护成本高 |
| `sommelier-rs` `main` | virtio-gpu cross-domain | ❌ | ❌ | Crostini 里通常因缺 `CAPSET_CROSS_DOMAIN` 起不来 |
| `sommelier-rs` `virtwl` | `/dev/wl0` | ❌ | ✅ | 唯一能在 Crostini 跑起来的分支 |
| **本项目** | **三者统一，自动选择** | **✅** | **✅** | 以 `virtwl` 为运行时底座，回补 `main` 独占能力 |

合并策略不是"选一个分支再叠功能"：两分支是互斥实现且互相丢过修复，
`virtwl` 从较早的 `main` 分叉，丢了后续正确性修复和全部 virtio-gpu 代码；
`main` 则没有 Crostini 必需的传输和 ChromeOS 协议。

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
  `--no-support-damage-buffer` 会把 `wl_compositor` 降到 v3 并改写 `damage_buffer`。
- **X11 ↔ Wayland ↔ ChromeOS host 三向剪贴板**（对齐主线 C++ `sommelier`）：
  X11 `CLIPBOARD` owner 既转成 guest 侧 `wl_data_offer`，也转成 host 侧
  `wl_data_source` + `set_selection`，因此 ChromeOS host 能直接粘贴 X11 应用
  复制的内容；host 的 `wl_data_source.send` 由 `ConvertSelection` 异步回写 fd。
  反向把 host offer / guest data source 的 MIME 与文本载荷预取给 X11
  `SelectionRequest`，并夺回 CLIPBOARD owner；支持 `TARGETS`、`INCR` 与延迟回复。
  详见 [`docs/CLIPBOARD.md`](docs/CLIPBOARD.md)。
- **X11 桌面细节**：`--frame-color` / `--dark-frame-color` 绘制窗口边框并写
  `_NET_FRAME_EXTENTS`；`--allow-xwayland-emulate-screen-pos-size` 依据实时输出
  布局维护 `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`。
- **ChromeOS 集成**：`zaura_shell`（shelf app-id）、`zcr_keyboard_extension_v1`
  （系统快捷键 ack-key）、`zwp_text_input_v1` ↔ v3 互转、`SOMMELIER_ACCELERATORS`。
- **SHM 多路径降级**：`virtwl` alloc → virtgpu host blob → GBM BO → sealed memfd。
- **协议代码生成**：`third_party/protocols/*.xml`（33 个）由 `build.rs` 自动枚举，
  生成 `protocols::all` 聚合分发；新增协议只需放入一个 XML。
- **主线参数兼容**：clap + `SOMMELIER_*` 环境变量，命令行优先于环境变量。

## 在 Crostini 中替换主线 sommelier

```bash
# 1. 停掉主线 sommelier（X11 侧服务可按需保留）
systemctl --user stop sommelier@0 sommelier@1

# 2. 编译并安装
cargo build --release --target x86_64-unknown-linux-gnu
install -m 755 target/x86_64-unknown-linux-gnu/release/sommelier-rs-r \
        ~/.local/bin/sommelier-rs-r

# 3. 只跑 Wayland（与 virtwl 分支用法一致）
~/.local/bin/sommelier-rs-r --transport=auto --virtio-wl /dev/wl0 wayland-0

# 4. 同时启用 X11 转发
~/.local/bin/sommelier-rs-r --transport=auto -X --display=wayland-0
```

启用 `-X` 后进程会自动设置 `DISPLAY` / `XCURSOR_SIZE`，X11 应用直接可用。
Xwayland 默认路径为 `/opt/google/cros-containers/bin/Xwayland`，可用
`--xwayland-path` 覆盖。若 Xwayland 启动失败，代理会记录错误并继续只提供
Wayland 服务，不会整体退出。

systemd 用户单元见 `packaging/systemd/sommelier-rs@.service`，
在 `[Service]` 的 `ExecStart` 后追加 `-X` 即可启用 X11。

## 常用参数

| 参数 | 说明 |
|---|---|
| `--transport`, `--virtio-wl`, `--local-compositor`, `--virtgpu-channel` | host 传输选择 |
| `--force-drm-device=PATH` | 强制 GBM / virtgpu 使用的 render node |
| `-X`, `--x-display`, `--xwayland-path`, `--x-auth`, `--glamor` | X11 / Xwayland |
| `--scale`, `--direct-scale`, `--stable-scaling`, `--dpi`, `--viewport-resize` | 缩放 |
| `--accelerators`, `--windowed-accelerators`, `--vm-identifier` | ChromeOS 集成 |
| `--quirks-config`, `--print-enabled-features` | quirks 规则 |
| `--disable-xdg-decoration`, `--no-support-damage-buffer`, `--gpu-accel` | 协议开关 |
| `--parent`, `--no-exit-with-child`, `--sd-notify`, `--log-level` | 进程行为 |
| 末尾 `-- <program> [args...]` | 就绪后启动子进程 |

完整参数表：`sommelier-rs-r --help`。主线约定的 `SOMMELIER_*` 环境变量
（如 `SOMMELIER_SCALE`、`SOMMELIER_ACCELERATORS`、`SOMMELIER_X11`）同样生效。

## 构建

依赖：Rust toolchain、`libxkbcommon-dev`、`libgbm-dev`、`libdrm-dev`、
`libexpat1-dev`。`x11rb` 是纯 Rust 实现，不需要额外的 X11 开发库。

```bash
cargo build --release --target x86_64-unknown-linux-gnu
cargo test  --target x86_64-unknown-linux-gnu                 # 在 Linux 主机上运行
cargo check --tests --target x86_64-unknown-linux-gnu --workspace   # 仅类型检查
```

Debian 打包与交叉编译 CI 位于 `.github/workflows/`。

## 项目结构

```
sommelier/
  src/
    main.rs            统一 CLI / X11 启动 / 子进程与 sd_notify
    config.rs          参数 + SOMMELIER_* 环境变量 → Config
    proxy.rs           每客户端一条 host 连接，双向消息分发，X11 WM 命令注入
    state.rs           ShadowTable / Context / ID 分配
    connection.rs      Unix / VirtWl / VirtGpu 三种传输
    virtwl_channel.rs  /dev/wl0 ioctl
    virtgpu_channel.rs virtio-gpu cross-domain
    allocator.rs       GBM / sealed memfd
    transform.rs       三坐标系变换、DPI、wl_fixed
    quirks.rs          textproto-like 规则解析
    handler/           手写协议 handler（含 output.rs 缩放改写）
    x11/               Xwayland 启动、X11 WM、atom 表、剪贴板桥接、bridge
      selection.rs     X11 CLIPBOARD ↔ Wayland data device 状态机
      clipboard.rs     X11 selection / INCR / 延迟 SelectionRequest
      bridge.rs        WM 线程与代理之间的共享状态
  build.rs             枚举 XML → 生成协议模块与聚合分发
wayland_codegen/       XML 解析 + Rust 代码生成 + aggregate 生成
third_party/protocols/ 33 个协议 XML
docs/                  当前架构、剪贴板主线对照、分支合并记录、上游架构基线、功能矩阵
```

## 已知限制

详见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) §6。最需要注意的三点：

1. **尚未在真实 Crostini 设备上做端到端验证**：开发机是 Windows，只能做
   `x86_64-unknown-linux-gnu` 交叉类型检查；`cargo test` 已加入 Debian CI
   （`x86_64` 真机执行），但尚未在 Crostini guest 里跑过 `-X` 启动流程与
   「X11 复制 → ChromeOS 粘贴」的 host 协议行为。
2. **explicit fence / seccomp / `--parent` fork 模型**仍是参数或语义级简化，
   见功能矩阵逐项说明。
3. **首次复制可能缺 serial**：`wl_data_device.set_selection` 的 serial 取自
   host 输入事件；进程刚启动尚未收到输入时为 0（主线 `last_serial` 初值同样
   为 0），此时会打 warning，个别严格校验的 compositor 可能拒绝这一次导出。

## 版本里程碑

| Tag | 内容 |
|---|---|
| `v0.3.0-r-merge-baseline` | 两分支完整对比合并、X11 转发主体、统一 CLI，编译干净 |
| `v0.3.0-r` | X11 ↔ guest Wayland 剪贴板、缩放/输出与内部协议正确性修复、RandR 模拟、边框、viewport、CI 真实测试 |
| `v0.3.1-r` | 按主线补齐 X11 → ChromeOS host 剪贴板导出、host/Wayland 复制夺回 X11 owner、serial 跟踪、内部对象事件隔离 |

## 其他说明

本项目不是 Google 官方支持的产品，也不属于 ChromiumOS 组件，
不参与 [Google Open Source Software Vulnerability Rewards Program](https://bughunters.google.com/open-source-security)。

## 许可

Apache-2.0。
