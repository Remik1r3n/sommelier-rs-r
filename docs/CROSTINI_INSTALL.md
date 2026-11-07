# 在 Crostini 里安装和使用 sommelier-rs-r（.deb）

这份指南针对 **ChromeOS / FydeOS 的 Linux 容器（Crostini / penguin）**。
`.deb` 已在真机验证：Debian 13 (trixie)、`x86_64`、`/dev/wl0` virtwl、
现役主线 sommelier 0.20（`wayland-0` / `wayland-1`、`:0` / `:1`）保持运行。

> **一句话**：装这个包**不会替换**主线 sommelier，也不会动你现在开着的桌面。
> 它只是多放一个 `/usr/bin/sommelier-rs-r`，你要用**独立 socket**主动启动它。

---

## 0. 安装前检查

在 Linux 终端里确认：

```bash
uname -m                       # 应为 x86_64（本 .deb 是 amd64）
cat /etc/os-release            # 本 .deb 在 Debian 13 (trixie) 上构建并验证
ls -l /dev/wl0                 # Crostini 的 virtio-wl 通道，必须存在
echo "$XDG_RUNTIME_DIR"        # 一般是 /run/user/1000
ls "$XDG_RUNTIME_DIR"/wayland-*   # 现役 socket，后面要避开这些名字
ls -l /opt/google/cros-containers/bin/Xwayland   # 用 -X 时需要
```

`/dev/wl0` 不存在时也能用，但要显式指定 host display（见 §8）。

---

## 1. 把 `.deb` 放进容器

包名格式：`sommelier-rs-r_<版本>_amd64.deb`，例如
`sommelier-rs-r_0.3.4-r_amd64.deb`。

三种方式任选：

```bash
# A. 从 ChromeOS「文件」应用把 .deb 放进 Linux 文件 / MyFiles/Downloads
sudo dpkg -i /mnt/chromeos/MyFiles/Downloads/sommelier-rs-r_0.3.4-r_amd64.deb

# B. 从别的机器 scp 进来（把地址换成你的容器地址）
scp -P 22122 sommelier-rs-r_0.3.4-r_amd64.deb remik1r3n@10.0.0.106:~/
sudo dpkg -i ~/sommelier-rs-r_0.3.4-r_amd64.deb

# C. 直接在容器里从源码重建（见 §10）
bash packaging/build-deb.sh
```

如果 `dpkg -i` 提示缺少依赖：

```bash
sudo apt-get -f install -y
```

> 本包的运行时依赖由 `dpkg-shlibdeps` 从构建出的二进制自动解析，真机结果为
> `libc6 (>= 2.39)`, `libgbm1`, `libgcc-s1`, `libxkbcommon0`（`libdrm2` /
> `libexpat1` 由 `libgbm1` 传递依赖带入，无需直接列出）。Crostini 通常已装齐。
>
> **版本绑定**：`libc6 (>= 2.39)` 来自构建机的 glibc，所以这个 `.deb` 适用于
> Debian 13 (trixie) 及更新版本；在 Debian 12 (bookworm) 等旧系统上请按 §10
> 在**目标机器本地**重建，脚本会按该系统的 shlibs 重新解析依赖。
>
> 若 `apt-get` 因为某个失效的第三方源（例如报 `cros.list ... no Release file`）
> 而失败，先修复/临时禁用那个源；本包不需要任何第三方源。

确认安装结果：

```bash
sommelier-rs-r --version        # 0.3.4-r
dpkg -L sommelier-rs-r          # 查看装了哪些文件
sommelier-rs-r --help | head -20
```

包装进去的内容：

| 路径 | 用途 |
|---|---|
| `/usr/bin/sommelier-rs-r` | 主程序（已 strip） |
| `/usr/lib/systemd/user/sommelier-rs-r@.service` | Wayland 模式的 user unit 模板 |
| `/usr/lib/systemd/user/sommelier-rs-r-x@.service` | X11 模式的 user unit 模板 |
| `/usr/share/doc/sommelier-rs-r/` | 本指南、README、`MAINLINE_PARITY.md` 等全部文档、copyright |

**安装过程不会启用任何服务，也不会写 `~/.Xauthority`、不改主线 unit。**

---

## 2. 与主线共存（最重要的一节）

| | 主线 sommelier | sommelier-rs-r |
|---|---|---|
| 可执行文件 | `/opt/google/cros-containers/bin/sommelier.elf` | `/usr/bin/sommelier-rs-r` |
| guest socket | `wayland-0`、`wayland-1` | **自己指定**，例如 `wayland-wb` |
| X display | `:0`、`:1` | **自己指定**，例如 `:9` |
| 服务 | `sommelier@0/1`、`sommelier-x@0/1` | `sommelier-rs-r@wayland-wb`、`sommelier-rs-r-x@wayland-wbx` |

因此：

- 本指南全程用 `--socket=wayland-wb` 和 `--x-display=9`；
- **不要**用 `--socket=wayland-0`（会与现役实例抢同一个 socket 文件）；
- **不要**执行 `systemctl --user stop sommelier@0 sommelier@1`，除非你已读完 §9
  并打算真正替换主线；
- 不要覆盖 `/opt/google/cros-containers/bin/sommelier.elf`。

关于参数语义（和主线 C++ 完全一致）：

- `--socket` = **guest 监听**的 socket 名（默认 `wayland-0`）；
- `--display` / `SOMMELIER_DISPLAY` = 要连接的 **host** Wayland display；
  在 Crostini 上通常**不传**，让它走 `/dev/wl0` virtwl；
- 位置参数 `[program] [args...]` = 代理就绪后要启动的程序，
  第一个位置参数之后的所有内容（含 `-` 开头）都原样传给子进程。

---

## 3. 快速开始：Wayland 模式

开一个终端跑代理（前台运行，方便看日志）：

```bash
sommelier-rs-r --transport=virtwl --virtio-wl /dev/wl0 --socket=wayland-wb
```

另开一个终端，让应用连到我们的 socket：

```bash
WAYLAND_DISPLAY=wayland-wb your-wayland-app
```

或者一条命令搞定（子进程退出时代理默认一起退出）：

```bash
sommelier-rs-r --transport=virtwl --virtio-wl /dev/wl0 --socket=wayland-wb -- your-wayland-app
```

`--transport` 可以省略（默认 `auto`，会先试 `/dev/wl0`）。
`--` 后面的内容全部作为 `[program] [args...]`。

---

## 4. 快速开始：X11 模式

```bash
sommelier-rs-r --transport=virtwl --virtio-wl /dev/wl0 \
    --socket=wayland-wb -X --x-display=9
```

等日志出现 `Xwayland ready` / `WM initialized` 后，另开终端：

```bash
DISPLAY=:9 xdpyinfo | grep dimensions     # 1920x1280（真机实测）
DISPLAY=:9 xrandr --query                 # XWAYLAND0 输出与 host 几何一致
DISPLAY=:9 xmessage hello                # 真实 X11 窗口，会映射成 host xdg_toplevel
```

要点：

- `-X` 会启动 `Xwayland` 并自己充当窗口管理器；默认二进制路径
  `/opt/google/cros-containers/bin/Xwayland`，可用 `--xwayland-path` 覆盖；
- 通过位置参数启动的程序会**自动继承** `DISPLAY`，无需手动 export：
  `sommelier-rs-r ... -X --x-display=9 -- xmessage hello`；
- 但**其它终端**里的 X11 程序仍需自己写 `DISPLAY=:9`（systemd user unit
  启动的代理不会改写你 shell 的环境）；
- Xwayland 启动失败时不会连累 Wayland 侧：代理会记录错误并继续服务
  Wayland 客户端，这是与主线一致的降级行为。

剪贴板自测（X11 ↔ guest Wayland ↔ ChromeOS host）：

```bash
printf "hello-from-x11" | DISPLAY=:9 xclip -selection clipboard
DISPLAY=:9 xclip -selection clipboard -o          # 应回显 hello-from-x11
DISPLAY=:9 xclip -selection clipboard -o -t TARGETS
```

真机实测：X11 侧 `TARGETS` 为 `TARGETS` + `UTF8_STRING`（MIME 名 1:1 原样透传），
同一时刻 guest Wayland 客户端能收到 `data_offer` / `selection`。
若没装 `xclip`/`x11-utils`：`sudo apt install xclip x11-utils x11-xserver-utils`。

---

## 5. 用 systemd 用户服务长期运行（可选）

```bash
systemctl --user daemon-reload

# Wayland 模式，实例名就是 guest socket 名
systemctl --user start sommelier-rs-r@wayland-wb.service
systemctl --user status sommelier-rs-r@wayland-wb.service

# X11 模式（用不同的实例名，避免和上面抢同一个 socket）
systemctl --user start sommelier-rs-r-x@wayland-wbx.service
systemctl --user status sommelier-rs-r-x@wayland-wbx.service

# 看日志、确认 Xwayland 起来了
journalctl --user -u sommelier-rs-r-x@wayland-wbx.service -f
```

开机自启（**确认不再需要手动试验后再做**）：

```bash
systemctl --user enable --now sommelier-rs-r@wayland-wb.service
```

调整环境变量（例如缩放、VM id、glamor）：

```bash
systemctl --user set-environment SOMMELIER_SCALE=1.5 SOMMELIER_GLAMOR=1
systemctl --user restart sommelier-rs-r@wayland-wb.service
```

或者用 drop-in 精确覆盖某个实例：

```bash
systemctl --user edit sommelier-rs-r@wayland-wb.service
```

填入（systemd 要求覆盖 `ExecStart` 时先写一行空的 `ExecStart=`）：

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/sommelier-rs-r --transport=auto --virtio-wl /dev/wl0 --socket=wayland-wb -X --x-display=9 --sd-notify
Environment=SOMMELIER_SCALE=1.5
```

> **覆盖 `ExecStart` 时务必保留 `--sd-notify`**：两个单元都是 `Type=notify`，
> 只有带这个参数，代理才会在监听 socket 就绪后向 systemd 发 `READY=1`。
> 漏掉它 `systemctl --user start` 会卡到超时并把服务杀掉（真机实测过）。
> 若确实不想用通知机制，在 drop-in 里同时写 `Type=simple`。

---

## 6. 常用参数速查

| 参数 | 说明 |
|---|---|
| `--socket=NAME` | guest 监听 socket 名（默认 `wayland-0`），**别和现役重名** |
| `--display=NAME` / `SOMMELIER_DISPLAY` | 要连接的 **host** Wayland display；Crostini 上一般不传 |
| `--transport=auto\|virtwl\|virtgpu\|unix` | host 通道选择，默认 `auto`（先试 `/dev/wl0`） |
| `--virtio-wl=PATH` | virtio_wl 设备路径，默认 `/dev/wl0` |
| `-X` / `--x11` | 启动 Xwayland + 内置 WM |
| `--x-display=N` | 强制 X display 号（同时隐式启用 X11） |
| `--xwayland-path`, `--x-auth`, `--x-font-path` | Xwayland 二进制 / auth / 字体路径 |
| `--scale`, `--direct-scale`, `--stable-scaling`, `--dpi` | 缩放与 DPI；数值按主线 `atof()`/`atoi()` 宽松解析 |
| `--glamor` | Xwayland glamor 加速（不加 `-shm`）；见 §7 的字符串 env 说明 |
| `--no-clipboard-manager` | 关闭 X11 CLIPBOARD_MANAGER |
| `--accelerators`, `--windowed-accelerators`, `--vm-identifier` | ChromeOS 快捷键与 app_id 前缀 |
| `--no-exit-with-child` | 子进程退出后代理继续运行 |
| `--log-level=0..3` | 数值越大越安静；`RUST_LOG` 优先 |
| `-h` / `-V` | 完整帮助 / 版本 |
| `[program] [args...]` | 就绪后启动的子进程 |

完整列表：`sommelier-rs-r --help`。

---

## 7. `SOMMELIER_*` 环境变量

Crostini 的 user manager 通常会注入一批 `SOMMELIER_*` 变量，本包会继承并按
主线 C++ 语义解析：

| 变量 | 语义 |
|---|---|
| `SOMMELIER_SCALE`, `SOMMELIER_DPI` | 缩放 / DPI 分档 |
| `SOMMELIER_GLAMOR` | **字符串**：只有字面量 `"0"` 关闭 glamor，其它值（含 `"1"`）都开启 |
| `SOMMELIER_CLIPBOARD_MANAGER` | **字符串**：只有 `"0"` 关闭剪贴板管理器 |
| `SOMMELIER_SUPPORT_DAMAGE_BUFFER` | **字符串**：`"0"` 时不公告 `damage_buffer` |
| `SOMMELIER_VM_IDENTIFIER`, `SOMMELIER_FRAME_COLOR`, `SOMMELIER_ACCELERATORS` | app_id 前缀、边框颜色、快捷键 |
| `SOMMELIER_X11`, `SOMMELIER_DIRECT_SCALE`, `SOMMELIER_STABLE_SCALING`, `SOMMELIER_VIRTGPU_CHANNEL` | 布尔开关，按主线字符串语义读取 |
| `SOMMELIER_SOCKET`, `SOMMELIER_DISPLAY`, `SOMMELIER_TRANSPORT` | 本包/主线共用的 CLI 对应项 |

> 这些是**字符串**而不是 clap 的 `true/false` 布尔值。真机上的
> `SOMMELIER_GLAMOR=1` 曾让早期版本在启动阶段就被 clap 拒绝；`v0.3.3-r`
> 起已按主线语义修复，无需手动 `unset`。

命令行参数优先于环境变量。

---

## 8. 其它 host 连接方式

默认（推荐）：不传 `--display`，走 `/dev/wl0` virtwl。

要连到一个**本地的** host compositor socket（调试用）时，才用 `--display`：

```bash
sommelier-rs-r --display=wayland-0 --socket=wayland-wb
```

注意 `--display` 是**名字**（相对 `$XDG_RUNTIME_DIR` 解析），不是完整路径；
要直接给路径用 `--local-compositor=/path/to/socket`。显式指定 `--display`
会跳过 virtwl 通道，这与主线 `const bool use_virtual_context = !display;` 一致。

virtio-gpu 后端：

```bash
sommelier-rs-r --transport=virtgpu --force-drm-device=/dev/dri/renderD128 --socket=wayland-wb
```

---

## 9. 想真正「替换」主线 sommelier？（进阶，先读）

**目前不建议在生产使用习惯里直接替换**，原因见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4：
`--peer-pid` / `--client-fd` 的 parent/child fd 传递、`wl_drm` 本地模拟、
完整 `--viewport-resize`、stylus→tablet 转换、Perfetto/stats 等还未 1:1 对齐。
真机上现役主线实例确实在被 QQ / VSCode 用 `--peer-pid --client-fd` 调用。

如果只是想试，流程是：

```bash
# 1. 停掉主线（会关掉正在跑的 Linux 应用窗口，先保存工作）
systemctl --user stop sommelier@0 sommelier@1
systemctl --user stop sommelier-x@0 sommelier-x@1

# 2. 用主线原来的 socket 名启动我们的代理
sommelier-rs-r --transport=auto --virtio-wl /dev/wl0 --socket=wayland-0 -X --x-display=0

# 3. 回滚
systemctl --user start sommelier@0 sommelier@1
```

更稳妥的做法是给主线 unit 做 drop-in 覆盖 `ExecStart`，而不是 kill 进程，
并且**先在 `wayland-wb` / `:9` 上把你的应用全跑通**。

---

## 10. 从源码重建 `.deb`

在容器里（需要 Rust 工具链与 `libxkbcommon-dev libgbm-dev libdrm-dev libexpat1-dev`）：

```bash
git clone <your-repo> sommelier-rs-r
cd sommelier-rs-r
bash packaging/build-deb.sh
ls -lh dist/sommelier-rs-r_*_amd64.deb
```

脚本会：`cargo build --release --workspace --locked` → strip →
`dpkg-shlibdeps` 解析依赖 → 组装 `DEBIAN/control` / `postinst` →
`dpkg-deb --build --root-owner-group -Zxz`。

可覆盖的环境变量：`OUT_DIR`（默认 `dist/`）、`CARGO_TARGET_DIR`、
`PKG_VERSION`、`PKG_ARCH`、`PKG_*` 中的版本号只影响包名，不影响二进制。

---

## 11. 故障排查

| 现象 | 原因 / 处理 |
|---|---|
| `error: invalid value '1' for '--glamor'` | 旧版本 bug；升级到 `v0.3.3-r` 及以上，或临时 `unset SOMMELIER_GLAMOR` |
| Xwayland 报 `could not connect to wayland server`，X11 静默降级 | `v0.3.2-r` 及更早的启动顺序死锁；升级到 `v0.3.3-r` 及以上 |
| `Job for sommelier-rs-r@....service failed because a timeout was exceeded` / 状态卡在 `activating (auto-restart)` | 单元是 `Type=notify`，但 `ExecStart` 少了 `--sd-notify`，systemd 等不到 `READY=1`。在 drop-in 里补上该参数，或同时改 `Type=simple` |
| `bind: Address already in use` / socket 已存在 | 换了实例名，或清掉陈旧文件：`rm -f "$XDG_RUNTIME_DIR"/wayland-wb*` |
| `X display :9 already in use` | 上一次测试留下了孤儿 Xwayland。先 `pgrep -af "[X]wayland.elf :9"` 再按 PID `kill`；**不要**用 `pkill -f "Xwayland.elf :9"`，那会连执行 pkill 的 shell 自己一起杀掉 |
| `Can't open display: (null)` | 命令忘了写 `DISPLAY=:9`；`export DISPLAY=:9` 或每次显式加前缀 |
| `glamor: 'wl_drm' not supported` | 已知差距（见 `MAINLINE_PARITY.md` §4），Xwayland 会回退软件渲染，功能不受影响 |
| `No such file or directory: /dev/wl0` | 指定 `--transport=unix --display=...`，或检查 Linux 容器是否正常运行 |
| 安装时 `apt-get -f install` 报第三方源无 Release | 修复/临时禁用该源；本包只依赖 Debian 官方仓库的运行时库 |
| 日志里 `BadWindow` 警告 | X11 窗口退出时的无害竞态，真机实测不影响功能 |
| 应用窗口不显示 / 键盘没焦点 | 看代理日志是否有 `X11 window ... -> host surface ...`；确认应用连的是 `:9` 而不是 `:0` |

调试时把日志调详细：

```bash
RUST_LOG=debug sommelier-rs-r --socket=wayland-wb -X --x-display=9 2>&1 | tee /tmp/sommelier-rs-r.log
```

---

## 12. 卸载

```bash
# 若启用过服务，先停掉（按你实际用的实例名）
systemctl --user disable --now sommelier-rs-r@wayland-wb.service 2>/dev/null || true
systemctl --user disable --now sommelier-rs-r-x@wayland-wbx.service 2>/dev/null || true
systemctl --user daemon-reload

sudo dpkg -r sommelier-rs-r     # 保留配置（本包没有可修改的配置）
# 或
sudo apt remove sommelier-rs-r
sudo dpkg -P sommelier-rs-r     # 彻底清除
```

卸载只删除本包自己的文件，**不会影响**主线 sommelier、其 systemd unit 或
你的 `~/.Xauthority`。测试期间产生的 `$XDG_RUNTIME_DIR/wayland-wb*`
和 `/tmp/.X9-lock` 不属于包内容，可手动删除。

---

## 13. 真机验证记录（v0.3.3-r / v0.3.4-r）

在 Debian 13 trixie / penguin / `/dev/wl0` / 主线 sommelier 0.20 并存的环境下，
全程用 `--socket=wayland-wb`、`--x-display=9` 完成：

- 143 个单元测试通过、`cargo fmt --check` 与 release 构建通过；
- Wayland：22 个 host global、0 协议错误；`wl_compositor v4`、`wl_output v3`、
  `wl_seat v5`、`wl_data_device_manager v3`、`xdg_wm_base v3` 全部命中主线版本钳制；
  `bind` / `create_surface` / `get_data_device` 正常，host 的
  `geometry(0,0)`、`mode 1920x1280@60`、`scale 2`、`done`、seat capabilities 完整转发；
- X11：`Xwayland 1.20.14` 在 `:9` 就绪、WM 初始化、`_NET_SUPPORTED` 前 8 个 atom
  与主线一致、RandR 模拟生效；真实 `xmessage` 窗口映射到 host `xdg_toplevel`；
  剪贴板往返成功；guest Wayland 客户端同时收到 `data_offer` / `selection`；
- 测试结束后现役 `wayland-0/1`、`:0/:1` 完好，测试用 `:9` 无残留。

**`v0.3.4-r` 安装包本身**：`sudo dpkg -i` 在真机通过，
`/usr/bin/sommelier-rs-r --version` = `0.3.4-r`；随包的
`sommelier-rs-r@wayland-wb.service` 与 `sommelier-rs-r-x@wayland-wbx.service`
两个模板都实测秒级 `active`（registry 22 globals / 0 错误、Xwayland `:9`
ready、WM initialized、剪贴板往返 `readback=unit-clip-3401`）；安装前后现役
`sommelier@0/1` 始终 `active`。打包过程中的 3 个真机问题与修复见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5.4。

未覆盖：ChromeOS host 侧人工「粘贴」、用 `--peer-pid/--client-fd` 顶替现役实例、
长时间稳定性、真实 GPU 加速路径。完整对照见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4 / §5。
