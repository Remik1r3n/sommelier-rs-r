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
> 剪贴板双向桥接与主线 C++ 实现的逐行对照见 [`docs/CLIPBOARD.md`](docs/CLIPBOARD.md)，
> 与主线 C++ 的逐项对齐、真机验证与已知差距见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md)，
> ChromeOS 快捷键抑制机制见 [`docs/KEYBOARD_SHORTCUT_INHIBITION.md`](docs/KEYBOARD_SHORTCUT_INHIBITION.md)，
> **Crostini 安装与使用（`.deb`）见 [`docs/CROSTINI_INSTALL.md`](docs/CROSTINI_INSTALL.md)**。

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

## 在 Crostini 中安装与使用

**最快的方式是直接装 `.deb`**（构建脚本 `packaging/build-deb.sh`，完整步骤见
[`docs/CROSTINI_INSTALL.md`](docs/CROSTINI_INSTALL.md)）：

```bash
sudo dpkg -i sommelier-rs-r_0.3.7-r_amd64.deb
sommelier-rs-r --transport=virtwl --virtio-wl /dev/wl0 --socket=wayland-wb
WAYLAND_DISPLAY=wayland-wb your-app          # 另一个终端
```

包只安装一个独立的 `/usr/bin/sommelier-rs-r` 和两个默认不启用的 systemd
user 模板，**不替换主线 sommelier、不动现役 `wayland-0/1` 与 `:0/:1`**。
下面的源码编译流程供开发，或用于「真正替换主线」的场景。

```bash
# 1. 停掉主线 sommelier（X11 侧服务可按需保留）
systemctl --user stop sommelier@0 sommelier@1

# 2. 编译并安装
cargo build --release --target x86_64-unknown-linux-gnu
install -m 755 target/x86_64-unknown-linux-gnu/release/sommelier-rs-r \
        ~/.local/bin/sommelier-rs-r

# 3. 只跑 Wayland（--socket 是 guest 监听的 display 名）
~/.local/bin/sommelier-rs-r --transport=auto --virtio-wl /dev/wl0 --socket=wayland-0

# 4. 同时启用 X11 转发
~/.local/bin/sommelier-rs-r --transport=auto -X --socket=wayland-0

# 5. 主线写法：--display 指定要连接的「宿主」compositor（不是监听 socket）
~/.local/bin/sommelier-rs-r --display=wayland-0 --socket=wayland-1 -X
```

> **与主线一致的参数语义**：`--socket` = 监听 socket 名（默认 `wayland-0`），
> `--display` / `SOMMELIER_DISPLAY` = 要连接的 **host** Wayland display，
> 位置参数 = `[program] [args...]`（主线在第一个位置参数处停止解析选项）。
> 详细说明见 `docs/MAINLINE_PARITY.md`。

启用 `-X` 后进程会自动设置 `DISPLAY` / `XCURSOR_SIZE`，X11 应用直接可用。
Xwayland 默认路径为 `/opt/google/cros-containers/bin/Xwayland`，可用
`--xwayland-path` 覆盖。若 Xwayland 启动失败，代理会记录错误并继续只提供
Wayland 服务，不会整体退出。

systemd 用户单元模板：`packaging/systemd/sommelier-rs-r@.service`（Wayland）与
`sommelier-rs-r-x@.service`（X11，`:9`）；`.deb` 会把它们装到
`/usr/lib/systemd/user/`，实例名即 guest socket 名，例如
`systemctl --user start sommelier-rs-r@wayland-wb.service`。
更多参数与 drop-in 写法见 `docs/CROSTINI_INSTALL.md` §5。

## 常用参数

| 参数 | 说明 |
|---|---|
| `--socket` | guest 监听 socket 名（主线 `--socket`，默认 `wayland-0`） |
| `--display` / `SOMMELIER_DISPLAY` | 要连接的 host Wayland display（主线语义） |
| `--transport`, `--virtio-wl`, `--local-compositor`, `--virtgpu-channel` | host 传输选择 |
| `--force-drm-device=PATH` | 强制 GBM / virtgpu 使用的 render node |
| `-X`, `--x-display`, `--xwayland-path`, `--x-auth`, `--glamor` | X11 / Xwayland |
| `--scale`, `--direct-scale`, `--stable-scaling`, `--dpi`, `--viewport-resize` | 缩放 |
| `--accelerators`, `--windowed-accelerators`, `--vm-identifier` | ChromeOS 集成 |
| `--quirks-config`, `--print-enabled-features` | quirks 规则 |
| `--disable-xdg-decoration`, `--no-support-damage-buffer`, `--gpu-accel` | 协议开关 |
| `--parent`, `--no-exit-with-child`, `--sd-notify`, `--log-level` | 进程行为 |
| `[program] [args...]`（可用 `--` 分隔） | 就绪后启动子进程，与主线一致 |

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
packaging/             build-deb.sh 与两个 systemd user 单元模板
docs/                  当前架构、安装指南、功能矩阵、剪贴板对照、主线一致性、
                       合并决策记录、ChromeOS 快捷键机制
```

## 已知限制

详见 [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) §6。最需要注意的四点：

1. **ChromeOS host 侧的人工交互尚未验证**：Wayland 与 X11 两条主链路已在
   Debian 13 / `penguin` / `/dev/wl0` 真机端到端跑通，**`wayland-0` / `:0`
   整套替换也已在真机演练并自动回滚**（切换态 wire 探针、Xwayland、WM、
   真实 X11 窗口、`wl_drm` PRIME、RandR 全 PASS）。仍需人工在 host 侧确认
   的是「X11 复制 → 在 ChromeOS 界面里粘贴」与输入法打字；长时间稳定性、
   真实 GPU 加速路径也未覆盖。详见 `docs/MAINLINE_PARITY.md` §4 / §5.7。
2. **与主线仍有实现路径差距**：`wl_drm` 本地模拟、Crostini IME 协议、
   `zaura_output` / `zxdg_output_v1` 内部订阅、`--peer-pid` / `--client-fd`
   真实语义与 `--timing-filename` / `--stats-*` / `--trace-*` 均已在
   `v0.3.6-r` 补齐；Xwayland scale 重算与 `wl_output.mode` wire 公式已用
   回归测试锁定与主线等价。仍存在差距的是 `wl_pointer` 坐标变换（整套事件
   纯转发，`viewport_pointer_scale` 未接入）、stylus-v2 协议转换、
   X11 屏幕 DPI（见下）、explicit fence / seccomp，以及 parent 用共享 runtime
   上的 per-client task 而非 fork 子进程（功能等价，真机多客户端探针 PASS）。见
   [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §4 / §4.1 与
   [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md) §6。
3. **X11 屏幕 DPI 与主线 :0 不同**：`xrandr` 的 RandR monitor 物理尺寸已与主线
   一致（`406x270mm` / 120 DPI），但 `xdpyinfo` 的 X screen 仍为
   `508x339mm` / 96 DPI。该值由 Xwayland 启动时的 `monitorResolution` 决定，
   真机实测 seeding DSF、`-dpi 120`、`xrandr --dpi 120` 三种方式均无法从代理
   侧改变；根因是本项目「每 guest client 独立 host 连接」的架构使 host 输出
   状态晚于 Xwayland 初始化就绪，而主线共享 host display 在启动 Xwayland 前
   已完成输出握手。受影响的只是读取 `Xft.dpi` 的老式 X11 应用。详见
   [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §4。
4. **首次复制可能缺 serial**：`wl_data_device.set_selection` 的 serial 取自
   host 输入事件；进程刚启动尚未收到输入时为 0（主线 `last_serial` 初值同样
   为 0，且照常发送 `set_selection`，`v0.3.6-r` 起不再打 warning）。个别严格
   校验 serial 的 compositor 可能拒绝这一次导出。

**想把主线整个换掉？** 默认安装始终保持共存；`v0.3.6-r` 起提供可灰度、
可一键回滚、且**已在真机完整演练并自动回滚**的替换手册：
[`docs/REPLACE_MAINLINE.md`](docs/REPLACE_MAINLINE.md)。启动参数映射、
`--parent` 多客户端、单实例 Wayland+X11、整套 drop-in 切换与回滚均已在真机
逐项验证；剩下需要你亲自确认的只有 ChromeOS host 侧粘贴 / IME 与长期稳定性。

## 版本里程碑

| Tag | 内容 |
|---|---|
| `v0.3.0-r-merge-baseline` | 两分支完整对比合并、X11 转发主体、统一 CLI，编译干净 |
| `v0.3.0-r` | X11 ↔ guest Wayland 剪贴板、缩放/输出与内部协议正确性修复、RandR 模拟、边框、viewport、CI 真实测试 |
| `v0.3.1-r` | 按主线补齐 X11 → ChromeOS host 剪贴板导出、host/Wayland 复制夺回 X11 owner、serial 跟踪、内部对象事件隔离 |
| `v0.3.2-r` | 逐项对照主线消除语义偏差：`--socket`/`--display`/位置参数、global 版本钳制、`gtk_shell1` 桥接、剪贴板 MIME 1:1、X11 app_id、INCR 64 KiB、输出几何、env 与 C 风格数值解析。详见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) |
| `v0.3.3-r` | **真实 Crostini 端到端验证**并修复 3 个真机特有问题：字符串 `SOMMELIER_*` env 绑定 bool flag 导致拒绝启动、X11 启动顺序死锁（Xwayland 连不上 Wayland proxy）、一个仅在运行时暴露的测试断言错误。Wayland 与 X11 两条主链路实测打通。详见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §1.5 / §5 |
| `v0.3.4-r` | **Debian `.deb` 交付与 Crostini 安装指南**：`packaging/build-deb.sh`（release + strip + `dpkg-shlibdeps` 自动解析依赖 + 两个 systemd user 模板 + 全量文档），`docs/CROSTINI_INSTALL.md` 覆盖安装、与主线共存、Wayland/X11/systemd 三种启动、参数速查、故障排查与卸载；`.deb` 已实机 `dpkg -i` 安装、两个 systemd 单元实测通过。详见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §5.4 |
| `v0.3.5-r` | **历史包袱清理**（无功能变更）：删除死文件、未用依赖与零调用符号，移除半接线的停机分支，统一 CRLF→LF 并去掉打包脚本的行尾兼容补丁，精简合并期过程文档、修正过时表述，release workflow 改为产出 `.deb`。主线 CLI 兼容面与三套 transport 保持不变 |
| `v0.3.6-r` | **继续补齐主线缺失能力**：本地模拟 `wl_drm` v2（`device`/`format`/`authenticated`/`capabilities`/`create_prime_buffer` → host dmabuf v2），真机 Xwayland 不再打印 `glamor: 'wl_drm' not supported` / `falling back to sw`；新增 `zcr_text_input_crostini_manager_v1` / `zcr_text_input_crostini_v1` / `zcr_text_input_x11_v1` 本地 IME 协议；内部订阅 `zaura_output`（scale / connection / device_scale_factor）与 `--direct-scale` 下的 `zxdg_output_v1`，Direct 模式改用真实 logical size，DSF 缺省值与主线一致（1.0）；`BadWindow` 改用 unchecked 请求、`host_serial == 0` 不再告警、扩展输入 serial 采集。真机 154 个测试全绿、release 0 warning、wire 探针 25 globals / `wl_drm` + IME 全部 PASS。另复查主线启动链路后新增 [`docs/REPLACE_MAINLINE.md`](docs/REPLACE_MAINLINE.md)（可灰度、可回滚的替换手册），并更正 `--peer-pid` 并非替换阻塞项。详见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §4.1 / §5.6 |
| `v0.3.7-r` | **收尾与发布**：补齐 `--peer-pid` / `--client-fd` 的真实语义（`SO_PEERCRED`、`Context.client_pid`、预连接 fd）与 `--timing-filename` / `--stats-*` / `--trace-*` 可观测性实现（`stats.rs`），用回归测试锁定 Xwayland output wire 与主线等价；新增 [`docs/REPLACE_MAINLINE.md`](docs/REPLACE_MAINLINE.md) 并在真机完成 `wayland-0` / `:0` 整套切换演练 + 自动回滚；修正架构文档过时的 delegation 计数、全面对齐差距表与验证记录。160 个测试、release 0 warning、`.deb` 实机安装验证。详见 [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md) §5.7 / §5.8 |

## 其他说明

本项目不是 Google 官方支持的产品，也不属于 ChromiumOS 组件，
不参与 [Google Open Source Software Vulnerability Rewards Program](https://bughunters.google.com/open-source-security)。

## 许可

Apache-2.0。
