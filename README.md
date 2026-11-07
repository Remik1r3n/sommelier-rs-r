# sommelier-rs-r

面向 ChromeOS Crostini 的加强版 Wayland 代理。它在 `sommelier-rs` 两个分支
（`main` / `virtwl`）完整对比合并的基础上，补回主线 C++ `sommelier` 拥有、
而 `sommelier-rs` 缺失的能力，其中最重要的是 **X11 / XWayland 转发**。

目标是让它在 Crostini 里几乎可以在所有场景替代原来的主线 `sommelier`，
同时保留 `sommelier-rs` 的现代协议能力（text-input-v3、fractional-scale、
ChromeOS 专有扩展等）。

> 分支对比与逐文件合并决策见 [`docs/BRANCH_MERGE.md`](docs/BRANCH_MERGE.md)，
> 功能完成度与已知限制见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md)。

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
  `wl_output` / `zxdg_output_v1` / `wp_fractional_scale_v1` 事件按虚拟坐标系重写。
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
    x11/               Xwayland 启动、X11 WM、atom 表、剪贴板、bridge
  build.rs             枚举 XML → 生成协议模块与聚合分发
wayland_codegen/       XML 解析 + Rust 代码生成 + aggregate 生成
third_party/protocols/ 33 个协议 XML
docs/                  分支合并记录、架构基线、功能矩阵
```

## 已知限制

详见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md)。最需要注意的两点：

1. **X11 ↔ Wayland 剪贴板尚未完全打通**：X11 selection 的 `TARGETS`/`INCR`
   处理和 Wayland data-device 转发各自可用，但两者之间的对象桥接
   （合成 `wl_data_offer` 并响应 `receive`）仍是预留接口。
2. **尚未在真实 Crostini 设备上做端到端验证**：本机完成的是编译、测试代码
   编译与静态检查；`cargo test` 的二进制需要在 Linux 环境执行。

## 其他说明

本项目不是 Google 官方支持的产品，也不属于 ChromiumOS 组件，
不参与 [Google Open Source Software Vulnerability Rewards Program](https://bughunters.google.com/open-source-security)。

## 许可

Apache-2.0。
