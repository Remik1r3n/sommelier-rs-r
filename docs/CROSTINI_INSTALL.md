# 在 Crostini 里安装和使用 sommelier-rs-r

面向 **ChromeOS / FydeOS 的 Linux 容器（Crostini / penguin）**。

> **一句话**：装这个包不会替换主线 `sommelier`，不会动你正在用的桌面，
> 默认也不会启动任何服务。它只是往系统里放一个可执行文件和两个**默认禁用**
> 的 systemd 模板，用不用、怎么用，都由你决定。

---

## 1. 先选方案

安装方式和运行方式是两件独立的事，分开选：

- **怎么把程序装进容器** → §3 / §4 / §5 三选一
- **装好之后怎么跑** → §6 四选一

### 1.1 三十秒决策

```
你的容器是 Debian 13 (trixie) / x86_64 吗？
├─ 是 → 只想先试试？           → 方案 A（§3）装包 + 运行方式 1（§6.2）
│        想长期用但不动主线？   → 方案 A（§3）装包 + 运行方式 2（§6.3）
│        想让所有应用默认走它？ → 方案 A（§3）装包 + 运行方式 3（§6.4）
│        只装、暂时不配置？     → 方案 A（§3）装包 + 运行方式 0（§6.1）
└─ 否（Debian 12 / aarch64 / 想自己改代码）
   → 方案 B（§4）在本机重建 .deb 再装

要改 Rust 代码、频繁编译、或不想动系统目录？
   → 方案 C（§5）直接编译到 ~/.local/bin
```

### 1.2 三种安装方式的利弊

| | **方案 A：官方 `.deb` 直接装** | **方案 B：本机重建 `.deb` 再装** | **方案 C：源码直接编译到用户目录** |
|---|---|---|---|
| 一句话 | 下载一个包，`dpkg -i` 完事 | 在容器里跑打包脚本，生成本机专用包 | `cargo build --release` 后自己放到 `~/.local/bin` |
| 适合谁 | 绝大多数 Crostini 用户 | Debian 12 / aarch64 / 想改编译参数 | 开发者、调试者、不想用 root |
| 前置条件 | Debian 13 + x86_64 + glibc ≥ 2.39 | Rust 工具链 + 4 个 `-dev` 包 + `dpkg-dev` | Rust 工具链 + 4 个 `-dev` 包 |
| 耗时 | 1 分钟 | 首次约 1–5 分钟（含编译） | 首次约 1–5 分钟（含编译） |
| 优点 | 最快；依赖由 `dpkg-shlibdeps` 自动解析；`dpkg -r` 一键卸载；文档和 systemd 模板随包安装；有 SHA-256 校验 | 依赖按**本机**的 shlibs 解析，不受构建机 glibc 限制；可指定版本/架构；产物同样是可管理的 `.deb` | 不写系统目录、不需要 root；改完代码立刻能跑；可同时保留多个构建 |
| 缺点 / 代价 | 依赖 Debian 13 的 glibc ≥ 2.39，旧系统装不上 | 需要装构建依赖；构建目录约占 1–2 GB；产物需自己校验 | 没有包管理，卸载靠手动删；systemd 单元要自己复制；无随包文档；更新要手动重编译 |
| 是否改动系统目录 | 是（`/usr/bin`、`/usr/lib/systemd/user`、`/usr/share/doc`） | 同方案 A | 否，全部在 `~/` |
| 回退成本 | `sudo dpkg -r` 一条命令 | 同方案 A | 删 2–3 个文件 |
| 详细步骤 | [§3](#3-方案-a官方-deb-直接安装推荐) | [§4](#4-方案-b在容器里从源码重建-deb-再安装) | [§5](#5-方案-c源码直接编译不装包) |

> 三种方式装出来的**是同一个程序**，功能没有差别。区别只在「怎么放进去」和
> 「怎么管理」。不确定的话直接选方案 A。

### 1.3 四种运行方式的利弊

| | **方式 0：只装不启用** | **方式 1：临时前台跑** | **方式 2：systemd 常驻独立实例** | **方式 3：完整替换主线** |
|---|---|---|---|---|
| 一句话 | 装完什么也不做 | 开一个终端跑，关掉就没了 | 独立 socket 常驻，可开机自启 | 接管 `wayland-0` / `:0` |
| 适合谁 | 先装好，以后再决定 | 第一次试用、排错 | 日常使用，且**不动主线** | 想让容器里所有 Linux 应用默认走 Rust 版 |
| 影响现役桌面 | 无 | 无 | 无 | **会重启 `:0`，当前 X11 窗口会关闭** |
| 需要 root | 仅安装时 | 仅安装时 | 仅安装时 | 仅安装时 |
| 优点 | 零风险、零副作用 | 日志直接可见、随时 Ctrl-C、不写配置 | 后台稳定运行、崩溃自动重启、可自启 | 真正等价于主线，无需给应用单独设环境变量 |
| 缺点 / 风险 | 不跑就没有任何效果 | 每次要手动开终端；应用需显式指定 `WAYLAND_DISPLAY` / `DISPLAY` | 应用仍需显式指定环境变量；要理解 `--sd-notify` 与 drop-in | 风险最高；需人工确认 ChromeOS host 侧交互；必须准备回滚 |
| 回退 | — | Ctrl-C | `systemctl --user disable --now` | 按[替换手册](REPLACE_MAINLINE.md#9-一键回滚) 删 drop-in + unmask |
| 详细步骤 | [§6.1](#61-方式-0只装不启用默认) | [§6.2](#62-方式-1临时前台跑一次最快试用) | [§6.3](#63-方式-2systemd-user-服务常驻长期共存) | [§6.4](#64-方式-3完整替换主线进阶) |

---

## 2. 安装前检查（所有方案通用）

在 Linux 终端里执行：

```bash
uname -m                       # 方案 A 需要 x86_64
cat /etc/os-release            # 方案 A 需要 Debian 13 (trixie)
ls -l /dev/wl0                 # Crostini 的 virtio-wl 通道，必须存在
echo "$XDG_RUNTIME_DIR"        # 一般是 /run/user/1000
ls "$XDG_RUNTIME_DIR"/wayland-*   # 现役 socket，后面要避开这些名字
ls -l /opt/google/cros-containers/bin/Xwayland   # 用 X11 时需要
```

`/dev/wl0` 不存在时程序仍可用，但要显式指定 host display，见
[§10](#10-其它-host-连接方式)。

**命名边界**（安装后无论哪种运行方式都要遵守）：

| | 主线 sommelier | sommelier-rs-r |
|---|---|---|
| 可执行文件 | `/opt/google/cros-containers/bin/sommelier.elf` | `/usr/bin/sommelier-rs-r` |
| guest socket | `wayland-0`、`wayland-1` | **自己指定**，例如 `wayland-rs` |
| X display | `:0`、`:1` | **自己指定**，例如 `:9` |

- 不要用 `--socket=wayland-0`（会与现役实例抢同一个 socket 文件）；
- 不要覆盖 `/opt/google/cros-containers/bin/sommelier.elf`；
- 不要 `stop sommelier@0 sommelier@1`，除非你已读完[替换手册](REPLACE_MAINLINE.md)。

参数语义与主线 C++ 一致：

- `--socket` = **guest 监听**的 socket 名（默认 `wayland-0`）；
- `--display` / `SOMMELIER_DISPLAY` = 要连接的 **host** Wayland display；
  在 Crostini 上通常**不传**，让它走 `/dev/wl0`；
- 位置参数 `[program] [args...]` = 代理就绪后要启动的程序，
  第一个位置参数之后的所有内容（含 `-` 开头）都原样传给子进程。

---

## 3. 方案 A：官方 `.deb` 直接安装（推荐）

### 3.1 步骤 1 — 把 `.deb` 放进容器

包名：`sommelier-rs-r_0.3.7-r_amd64.deb`。三种传输方式任选：

```bash
# A1. 从 ChromeOS「文件」应用把 .deb 放进 Linux 文件 / MyFiles/Downloads
sudo dpkg -i /mnt/chromeos/MyFiles/Downloads/sommelier-rs-r_0.3.7-r_amd64.deb

# A2. 从别的机器 scp 进来（把地址换成你的容器地址）
scp -P <port> sommelier-rs-r_0.3.7-r_amd64.deb <user>@<container-ip>:~/
sudo dpkg -i ~/sommelier-rs-r_0.3.7-r_amd64.deb

# A3. 已经在容器里下载好了
sudo dpkg -i ./sommelier-rs-r_0.3.7-r_amd64.deb
```

> 直接把 `.deb` 拖进「Linux 文件」后，在终端里通常位于
> `~/` 或 `/mnt/chromeos/MyFiles/Downloads/`，用 `ls ~/*.deb` 找一下即可。

### 3.2 步骤 2 — 校验（可选但推荐）

发布页同时提供 `SHA256SUMS`。把它和 `.deb` 放在同一目录：

```bash
sha256sum -c SHA256SUMS
# sommelier-rs-r_0.3.7-r_amd64.deb: OK
```

`SHA256SUMS` 是仓库根目录的校验清单，**不随包分发**，因此校验要在安装前、
在手上有原始下载文件的目录里做。

### 3.3 步骤 3 — 安装

```bash
sudo dpkg -i sommelier-rs-r_0.3.7-r_amd64.deb
```

如果提示缺少依赖：

```bash
sudo apt-get -f install -y
```

> 运行时依赖由 `dpkg-shlibdeps` 从构建出的二进制自动解析，结果为
> `libc6 (>= 2.39)`、`libgbm1`、`libgcc-s1`、`libxkbcommon0`
> （`libdrm2` / `libexpat1` 由 `libgbm1` 传递带入）。Crostini 通常已装齐。
>
> **版本绑定**：`libc6 (>= 2.39)` 来自构建机的 glibc，因此本包适用于
> Debian 13 (trixie) 及更新版本。Debian 12 (bookworm) 等旧系统请改用
> [方案 B](#4-方案-b在容器里从源码重建-deb-再安装)。
>
> 若 `apt-get` 因某个失效的第三方源（例如 `cros.list ... no Release file`）
> 失败，先修复或临时禁用那个源；本包不需要任何第三方源。

### 3.4 步骤 4 — 确认

```bash
sommelier-rs-r --version        # sommelier-rs-r 0.3.7-r
dpkg -L sommelier-rs-r          # 查看装了哪些文件
sommelier-rs-r --help | head -20
```

安装过程**不启用任何服务、不写 `~/.Xauthority`、不改主线 unit**。
装完的那一刻，容器里唯一的变化就是多了下面这些文件：

| 路径 | 用途 |
|---|---|
| `/usr/bin/sommelier-rs-r` | 主程序（已 strip） |
| `/usr/lib/systemd/user/sommelier-rs-r@.service` | Wayland 模式的 user unit 模板（disabled） |
| `/usr/lib/systemd/user/sommelier-rs-r-x@.service` | X11 模式的 user unit 模板（disabled） |
| `/usr/share/doc/sommelier-rs-r/` | 本文档、README、其余技术文档、copyright |

接着去 [§6 选一种运行方式](#6-装好之后选一种运行方式)。

---

## 4. 方案 B：在容器里从源码重建 `.deb` 再安装

适用于：Debian 12 及更新的旧系统、aarch64、想指定版本/架构、或想在
安装前改动源码。**产物的安装与卸载方式与方案 A 完全相同**，只是这个包
是按你本机的共享库版本生成的。

### 4.1 前置依赖

```bash
sudo apt-get update
sudo apt-get install -y \
    curl build-essential pkg-config dpkg-dev \
    libxkbcommon-dev libgbm-dev libdrm-dev libexpat1-dev

# Rust 工具链（若尚未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
```

`x11rb` 是纯 Rust 实现，不需要额外的 X11 开发库。

### 4.2 步骤

```bash
git clone <your-repo> sommelier-rs-r
cd sommelier-rs-r
bash packaging/build-deb.sh
ls -lh dist/sommelier-rs-r_*_amd64.deb
sudo dpkg -i dist/sommelier-rs-r_*.deb
sommelier-rs-r --version
```

脚本流程：`cargo build --release --workspace --locked` → strip →
`dpkg-shlibdeps` 按本机 shlibs 解析依赖 → 生成 `DEBIAN/control` / `postinst`
→ `dpkg-deb --build --root-owner-group -Zxz`。

可覆盖的环境变量：

| 变量 | 默认 | 用途 |
|---|---|---|
| `OUT_DIR` | `dist/` | 产物目录 |
| `CARGO_TARGET_DIR` | `target/` | 构建缓存目录 |
| `PKG_VERSION` | 读自 `sommelier/Cargo.toml` | 包名中的版本号（不改二进制内版本） |
| `PKG_ARCH` | `dpkg --print-architecture` | 目标架构 |
| `PKG_MAINTAINER` | 项目默认值 | control 里的维护者 |

> 构建缓存 `target/` 会占 1–2 GB，确认不再需要时可以 `rm -rf target/`。

接着去 [§6 选一种运行方式](#6-装好之后选一种运行方式)。

---

## 5. 方案 C：源码直接编译（不装包）

适用于：开发调试、频繁改动、不想往 `/usr` 写东西、或没有 root。

### 5.1 前置依赖

同 [§4.1](#41-前置依赖)（不需要 `dpkg-dev`）。

### 5.2 步骤

```bash
git clone <your-repo> sommelier-rs-r
cd sommelier-rs-r
cargo build --release --workspace --locked

mkdir -p ~/.local/bin
install -m 755 target/release/sommelier-rs-r ~/.local/bin/sommelier-rs-r
~/.local/bin/sommelier-rs-r --version
```

若 `~/.local/bin` 不在 PATH：

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.profile
. ~/.profile
```

### 5.3 使用 systemd（可选）

不装包就没有随包的 unit 模板，需要时手动复制：

```bash
mkdir -p ~/.config/systemd/user
cp packaging/systemd/*.service ~/.config/systemd/user/
systemctl --user daemon-reload
```

之后可以按 [§6.3](#63-方式-2systemd-user-服务常驻长期共存) 操作，但要把命令里的
`/usr/bin/sommelier-rs-r` 换成 `~/.local/bin/sommelier-rs-r`（systemd 不展开
`~`，请写绝对路径 `/home/<user>/.local/bin/sommelier-rs-r`）。

### 5.4 更新与清理

```bash
# 更新：拉新代码后重新执行上面两条命令即可
git pull && cargo build --release --workspace --locked \
  && install -m 755 target/release/sommelier-rs-r ~/.local/bin/sommelier-rs-r

# 彻底移除
rm -f ~/.local/bin/sommelier-rs-r
rm -f ~/.config/systemd/user/sommelier-rs-r*.service
systemctl --user daemon-reload
```

接着去 [§6 选一种运行方式](#6-装好之后选一种运行方式)。

---

## 6. 装好之后：选一种运行方式

以下示例统一用 `--socket=wayland-rs` 和 `--x-display=9`，**都不会碰主线**。
方案 C 请把 `sommelier-rs-r` 换成实际路径。

### 6.1 方式 0：只装不启用（默认）

什么都不做即可。此时：

```bash
systemctl --user list-units --all 'sommelier-rs-r*' --no-pager   # 无 active 单元
ls "$XDG_RUNTIME_DIR"/wayland-*                                  # 不会出现新 socket
sommelier-rs-r --version                                         # 程序可用
```

这是最干净的状态：程序就位、零副作用。想用了随时回到本节换一种方式。

### 6.2 方式 1：临时前台跑一次（最快试用）

**Wayland 应用**（开一个终端跑代理）：

```bash
sommelier-rs-r --socket=wayland-rs
```

另开一个终端：

```bash
WAYLAND_DISPLAY=wayland-rs your-wayland-app
```

**X11 应用**：

```bash
sommelier-rs-r --socket=wayland-rs -X --x-display=9
```

等日志出现 `Xwayland ready` / `WM initialized` 后，另开终端：

```bash
DISPLAY=:9 xdpyinfo | grep dimensions
DISPLAY=:9 xmessage hello
```

**一条命令带起应用**（子进程退出时代理默认一起退出，不留后台进程）：

```bash
# Wayland
sommelier-rs-r --socket=wayland-rs -- your-wayland-app

# X11：位置参数启动的程序会自动继承 DISPLAY
sommelier-rs-r --socket=wayland-rs -X --x-display=9 -- xmessage hello
```

> `--transport` 可省略（默认 `auto`，优先 `/dev/wl0`）。`--` 之后的内容
> 全部作为 `[program] [args...]`。结束用 Ctrl-C。
> 其它终端里的 X11 程序仍需自己写 `DISPLAY=:9`。

### 6.3 方式 2：systemd user 服务常驻（长期共存）

```bash
systemctl --user daemon-reload

# Wayland 模式：实例名就是 guest socket 名
systemctl --user start sommelier-rs-r@wayland-rs.service
systemctl --user status sommelier-rs-r@wayland-rs.service

# X11 模式：必须用不同的实例名，避免抢同一个 socket
systemctl --user start sommelier-rs-r-x@wayland-rsx.service
systemctl --user status sommelier-rs-r-x@wayland-rsx.service

journalctl --user -u sommelier-rs-r-x@wayland-rsx.service -f
```

确认稳定后再设开机自启：

```bash
systemctl --user enable --now sommelier-rs-r@wayland-rs.service
```

调整环境变量：

```bash
systemctl --user set-environment SOMMELIER_SCALE=1.5
systemctl --user restart sommelier-rs-r@wayland-rs.service
```

或用 drop-in 精确覆盖某个实例（`systemctl --user edit sommelier-rs-r@wayland-rs.service`）：

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/sommelier-rs-r --transport=auto --virtio-wl /dev/wl0 --socket=wayland-rs -X --x-display=9 --sd-notify
Environment=SOMMELIER_SCALE=1.5
```

> **覆盖 `ExecStart` 时必须保留 `--sd-notify`**。两个单元都是 `Type=notify`，
> 只有带这个参数，代理才会在监听 socket 就绪后发 `READY=1`；漏掉会让
> `start` 卡到超时并把服务杀掉。若不想用通知机制，在 drop-in 里同时写
> `Type=simple`。systemd 要求覆盖 `ExecStart` 时先写一行空的 `ExecStart=`。

停止 / 取消自启：

```bash
systemctl --user disable --now sommelier-rs-r@wayland-rs.service
systemctl --user daemon-reload
```

### 6.4 方式 3：完整替换主线（进阶）

让 `wayland-0` / `:0` 由 Rust 版提供，容器里的 Linux 应用无需再单独设置
`WAYLAND_DISPLAY` / `DISPLAY`。这是**唯一会关闭现有 X11 窗口**的方式，请按
[`REPLACE_MAINLINE.md`](REPLACE_MAINLINE.md) 操作：支持只切 `0` 保留 `1`
的灰度策略、drop-in 写法、验收清单和一键回滚。

动手前需要知道：切换会重启 `:0`，当前窗口会关闭；ChromeOS host 侧的
复制粘贴 / 输入法、长时间稳定性仍需人工确认；X11 `xdpyinfo` 的 DPI
存在已知降级（见 §13）。

---

## 7. 验证：怎么确认它真的在工作

```bash
# 代理进程与 socket
pgrep -af '[s]ommelier-rs-r'
ls -l "$XDG_RUNTIME_DIR/wayland-rs"

# Wayland 侧：让应用连到我们的 socket
WAYLAND_DISPLAY=wayland-rs your-wayland-app
# 若装了 wayland-utils，可直接列出代理公布的 globals
WAYLAND_DISPLAY=wayland-rs wayland-info 2>/dev/null | head -20 || true

# X11 侧：窗口、RandR、WM
DISPLAY=:9 xdpyinfo | grep dimensions
DISPLAY=:9 xrandr --query
DISPLAY=:9 xprop -root _NET_SUPPORTING_WM_CHECK
DISPLAY=:9 xmessage hello
```

剪贴板自测（X11 ↔ guest Wayland ↔ ChromeOS host）：

```bash
printf "hello-from-x11" | DISPLAY=:9 xclip -selection clipboard
DISPLAY=:9 xclip -selection clipboard -o
DISPLAY=:9 xclip -selection clipboard -o -t TARGETS
```

X11 侧 `TARGETS` 为 `TARGETS` + `UTF8_STRING`（MIME 名 1:1 原样透传），
同一时刻 guest Wayland 客户端能收到 `data_offer` / `selection`。
若没装这些 X11 小工具（验证用，程序本身不需要）：

```bash
sudo apt install xclip x11-apps x11-utils x11-xserver-utils
# 可选：查看 guest 公布的 Wayland globals
sudo apt install wayland-utils
```

---

## 8. 常用参数速查

| 参数 | 说明 |
|---|---|
| `--socket=NAME` | guest 监听 socket 名（默认 `wayland-0`），**别和现役重名** |
| `--display=NAME` / `SOMMELIER_DISPLAY` | 要连接的 **host** Wayland display；Crostini 上一般不传 |
| `--transport=auto\|virtwl\|virtgpu\|unix` | host 通道，默认 `auto`（先试 `/dev/wl0`） |
| `--virtio-wl=PATH` | virtio_wl 设备路径，默认 `/dev/wl0` |
| `-X` / `--x11` | 启动 Xwayland + 内置 WM |
| `--x-display=N` | 强制 X display 号（同时隐式启用 X11） |
| `--xwayland-path`, `--x-auth`, `--x-font-path` | Xwayland 二进制 / auth / 字体路径 |
| `--scale`, `--direct-scale`, `--stable-scaling`, `--dpi` | 缩放与 DPI；数值按主线 `atof()`/`atoi()` 宽松解析 |
| `--glamor` | Xwayland glamor 加速（不加 `-shm`）；见 §9 的字符串 env 说明 |
| `--no-clipboard-manager` | 关闭 X11 CLIPBOARD_MANAGER |
| `--accelerators`, `--windowed-accelerators`, `--vm-identifier` | ChromeOS 快捷键与 app_id 前缀 |
| `--no-exit-with-child` | 子进程退出后代理继续运行 |
| `--log-level=0..3` | 数值越大越安静；`RUST_LOG` 优先 |
| `--timing-filename`, `--stats-log`, `--stats-summary`, `--stats-timer`, `--trace-filename`, `--trace-system` | 可观测性输出（CSV 采样 / 实时摘要 / 逐消息 trace） |
| `-h` / `-V` | 完整帮助 / 版本 |
| `[program] [args...]` | 就绪后启动的子进程 |

完整列表：`sommelier-rs-r --help`。

---

## 9. `SOMMELIER_*` 环境变量

Crostini 的 user manager 会注入一批 `SOMMELIER_*` 变量，程序按主线 C++ 语义解析：

| 变量 | 语义 |
|---|---|
| `SOMMELIER_SCALE`, `SOMMELIER_DPI` | 缩放 / DPI 分档 |
| `SOMMELIER_GLAMOR` | **字符串**：只有字面量 `"0"` 关闭 glamor，其它值（含 `"1"`）都开启 |
| `SOMMELIER_CLIPBOARD_MANAGER` | **字符串**：只有 `"0"` 关闭剪贴板管理器 |
| `SOMMELIER_SUPPORT_DAMAGE_BUFFER` | **字符串**：`"0"` 时不公告 `damage_buffer` |
| `SOMMELIER_VM_IDENTIFIER`, `SOMMELIER_FRAME_COLOR`, `SOMMELIER_ACCELERATORS` | app_id 前缀、边框颜色、快捷键 |
| `SOMMELIER_X11`, `SOMMELIER_DIRECT_SCALE`, `SOMMELIER_STABLE_SCALING`, `SOMMELIER_VIRTGPU_CHANNEL` | 布尔开关，按主线字符串语义读取 |
| `SOMMELIER_SOCKET`, `SOMMELIER_DISPLAY`, `SOMMELIER_TRANSPORT` | CLI 对应项 |

这些是**字符串**而不是 clap 的 `true/false` 布尔值，因此
`SOMMELIER_GLAMOR=1` 可以直接工作，无需手动 `unset`。
命令行参数优先于环境变量。

---

## 10. 其它 host 连接方式

默认（推荐）：不传 `--display`，走 `/dev/wl0` virtwl。

要连到一个**本地的** host compositor socket（调试用）时才用 `--display`：

```bash
sommelier-rs-r --display=wayland-0 --socket=wayland-rs
```

`--display` 是**名字**（相对 `$XDG_RUNTIME_DIR` 解析），不是完整路径；
要直接给路径用 `--local-compositor=/path/to/socket`。显式指定 `--display`
会跳过 virtwl 通道，与主线 `use_virtual_context = !display` 一致。

virtio-gpu 后端：

```bash
sommelier-rs-r --transport=virtgpu --force-drm-device=/dev/dri/renderD128 --socket=wayland-rs
```

---

## 11. 故障排查

| 现象 | 原因 / 处理 |
|---|---|
| `Job for sommelier-rs-r@....service failed because a timeout was exceeded` / 卡在 `activating (auto-restart)` | 单元是 `Type=notify`，但 `ExecStart` 少了 `--sd-notify`，systemd 等不到 `READY=1`。在 drop-in 里补上该参数，或同时写 `Type=simple` |
| `bind: Address already in use` / socket 已存在 | 换实例名，或清掉陈旧文件：`rm -f "$XDG_RUNTIME_DIR"/wayland-rs*` |
| `X display :9 already in use` | 上一次运行留下了孤儿 Xwayland。先 `pgrep -af "[X]wayland.elf :9"` 再按 PID `kill`；**不要**用 `pkill -f "Xwayland.elf :9"`，那会连执行 pkill 的 shell 自己一起杀掉 |
| `Can't open display: (null)` | 命令忘了写 `DISPLAY=:9`；`export DISPLAY=:9` 或每次显式加前缀 |
| `glamor: 'wl_drm' not supported` / `falling back to sw` | 代理本地模拟 `wl_drm` v2，正常不应出现。若出现，检查 `SOMMELIER_DRM_DEVICE` / `/dev/dri/renderD*` 是否存在 |
| `No such file or directory: /dev/wl0` | 指定 `--transport=unix --display=...`，或检查 Linux 容器是否正常运行 |
| 安装时 `apt-get -f install` 报第三方源无 Release | 修复 / 临时禁用该源；本包只依赖 Debian 官方仓库的运行时库 |
| 日志里出现 `BadWindow` | X11 窗口退出时的无害竞态，按 debug 级记录（`x11: ignoring BadWindow packet`），不是告警，可忽略 |
| `xdpyinfo` 显示 96 DPI（主线 `:0` 是 120 DPI） | 已知差距，见 §13。`xrandr` 的 RandR monitor 已是正确的 `406x270mm`；若老式 X11 应用偏小，可 `echo "Xft.dpi: 120" \| xrdb -merge` |
| 应用窗口不显示 / 键盘没焦点 | 看代理日志是否有 `X11 window ... -> host surface ...`；确认应用连的是 `:9` 而不是 `:0` |
| `--version` 找不到命令 | 方案 C 下检查 `~/.local/bin` 是否在 PATH；方案 A/B 下执行 `dpkg -L sommelier-rs-r \| grep bin` |

把日志调详细：

```bash
RUST_LOG=debug sommelier-rs-r --socket=wayland-rs -X --x-display=9 2>&1 | tee /tmp/sommelier-rs-r.log
```

---

## 12. 卸载与回退

**方案 A / B（`.deb`）**

```bash
# 先停掉自己启用过的实例（按实际实例名）
systemctl --user disable --now sommelier-rs-r@wayland-rs.service 2>/dev/null || true
systemctl --user disable --now sommelier-rs-r-x@wayland-rsx.service 2>/dev/null || true
systemctl --user daemon-reload

sudo dpkg -r sommelier-rs-r      # 或 sudo apt remove sommelier-rs-r
sudo dpkg -P sommelier-rs-r      # 彻底清除
```

卸载只删除本包自己的文件，**不影响**主线 sommelier、其 systemd unit 或
`~/.Xauthority`。运行期间产生的 `$XDG_RUNTIME_DIR/wayland-rs*`、
`/tmp/.X9-lock`、`:9` 的 Xwayland 不属于包内容，可手动删除。

**方案 C（源码）** 见 [§5.4](#54-更新与清理)。

**若做过方式 3（替换主线）**：先按
[`REPLACE_MAINLINE.md` §9](REPLACE_MAINLINE.md#9-一键回滚) 回滚到主线，
再卸载本包。

---

## 13. 当前验证状态与已知差距

程序在 Debian 13 / `penguin` / `/dev/wl0` 的 Crostini 真机上完成过 Wayland
与 X11 端到端验证：160 个单元测试全绿、release 构建 0 warning、
`wl_drm` v2 与 Crostini IME 的 wire 探针通过、真实 X11 窗口映射到 host
`xdg_toplevel`、剪贴板往返正常、两个 systemd 模板可秒级 `active`。
`wayland-0` / `:0` 的整套替换流程也已在真机跑通并自动回滚。

**仍需人工确认**（代码无法自证）：ChromeOS host 侧的复制粘贴与输入法打字、
连续数天的稳定性、真实 GPU 加速路径、替换态下注销重登后的长期自启。

**已知技术差距**（未在默认路径造成功能故障）：

| 项 | 现状 |
|---|---|
| X11 screen DPI | `xrandr` 的 RandR monitor 与主线同为 `406x270mm` / 120 DPI，但 `xdpyinfo` 的 X screen 是 `508x339mm` / 96 DPI。该值由 Xwayland 启动时的 `monitorResolution` 固定，代理侧无法改写；影响仅限读 `Xft.dpi` 的老式 X11 应用，可用 `Xft.dpi: 120` 兜底 |
| `wl_pointer` 坐标变换 | 主线会做 host→guest fixed 变换与 `viewport_pointer_scale` 补偿，本项目 `wl_pointer` 事件是纯转发；`--viewport-resize` 默认关闭，已验证路径不受影响 |
| stylus-v2 | 主线做协议转换，本项目透传 tablet-v2 |
| explicit fence / seccomp | 未接入 |
| INCR 接收侧 | 阈值与 chunk 与主线一致，驱动策略为预取缓存 + 按需 `ConvertSelection` |
| 帧统计格式 | `--timing-filename` / `--stats-*` / `--trace-*` 输出 CSV / 文本 / 逐消息 trace，不产生 Perfetto 原生 trace |
| `--parent` 进程模型 | 主线为每个 client fork worker 进程，本项目用同一进程内的 tokio task |

逐项对照见 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md)，功能清单见
[`FEATURE_MATRIX.md`](FEATURE_MATRIX.md)。
