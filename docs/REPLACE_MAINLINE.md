# 把 Crostini 的主线 sommelier 换成 sommelier-rs-r

> 适用版本：`v0.3.7-r`（演练在 `v0.3.6-r` 构建产物上完成，代码一致）。
> 本文是**可选的进阶操作**；默认安装方式见
> [`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md) §2，只新增共存实例，不动主线。
>
> **状态声明（请先读）**：本文给出的启动参数映射、CLI 兼容性、
> `--parent` 多客户端能力、单实例同时提供 Wayland + X11、sd-notify、
> 尾随命令执行链，均已在 Debian 13 / `penguin` / `/dev/wl0` 真机逐项验证。
> **§6 的完整切换也已在真机实跑一遍并自动回滚**（`v0.3.6-r`，见 §11）：
> `wayland-0` / `:0` 切换态下 wire 探针、Xwayland、WM、真实 X11 窗口、
> `wl_drm` PRIME、RandR 全部 PASS，保留的主线 `@1` 全程可用，回滚后
> 5 个单元与全部环境变量恢复原状。最终的 `v0.3.7-r` `.deb` 也已在同一台
> 真机 `dpkg -i` 安装，并在共存 socket 上重新跑通 wire 探针 / X11 冒烟，
> 5 个单元全部 `active`、主线 `:0` 几何未变（见
> [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5.8）。本文因此按
> 「可灰度、可一键回滚」编写；**仍需人工确认的只有 ChromeOS host 侧交互
> 与长期稳定性**，不是"生产无风险"承诺。

---

## 1. 现在到底能不能换？

先给结论：**可以换了。** 差距表里的阻塞项已清零：协议与启动契约
逐项验证过，整套切换在真机跑通并成功回滚，剩下的是「必须由人在
ChromeOS 侧确认」和「长时间运行」两类，代码无法自证。

| 层次 | 状态 | 说明 |
|---|---|---|
| Wayland / X11 主链路 | ✅ 已验证 | 160 个测试、release 0 warning、wire 探针 PASS、真实 X11 窗口映射到 host `xdg_toplevel`、剪贴板往返、`wl_drm.create_prime_buffer` 被真实 Xwayland 调用 |
| 主线 CLI / 环境变量 / global 版本 | ✅ 已对齐 | 30 余处语义偏差已修正，见 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §1 |
| 主线 unit 的 `ExecStart` 参数 | ✅ 已验证可解析 | `--parent`、`--sd-notify="READY=1"`、`--no-exit-with-child`、`--x-auth`、`--enable-xshape`、`--stable-scaling`、`--enable-linux-dmabuf`、尾随 `/bin/sh -c ...` 全部能吃下 |
| `--parent` 多客户端 | ✅ 已验证 | `UnixListener` + `accept()` 循环 + 每 client 一个 task；真机隔离 socket 上两个独立探针先后连接均 PASS |
| **完整 drop-in 切换 + 自动回滚** | ✅ **真机已执行** | 切换态 §8 清单全过，保留主线 `@1` 作 A/B，trap 无条件回滚；逐项数据见 §11 与 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §5.7 |
| `--peer-pid` / `--client-fd` | ✅ 已实现 | `SO_PEERCRED` 取 pid（显式 `--peer-pid` 优先）、`--client-fd` 作为首个连接，真机日志 `peer_pid=Some(20355)` |
| `--timing-filename` / `--stats-*` / `--trace-*` | ✅ 已实现 | CSV 采样、实时摘要、逐消息 trace；真机实测落盘 |
| X11 screen DPI | ⚠️ 有条件 | `xrandr` monitor 已是主线同款 `406x270mm`，但 `xdpyinfo` X screen 仍 96 DPI（主线 120）。可用 `Xft.dpi` 兜底，见 §10 |
| Crostini IME host 侧交互 | ⚠️ 有条件 | 协议与 XID→surface 解析已实现，但「在 ChromeOS 界面里真正打字」需人工确认 |
| ChromeOS host 侧人工粘贴 / 长时间稳定性 / 真实 GPU 加速 | ❌ 未覆盖 | 需要人或连续数天运行，代码侧无法自证 |

按 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4 的差距表数：剩 7 项，
其中 2 项已用回归测试锁定与主线等价（Xwayland scale 重算、
`wl_output.mode` wire 公式）、1 项是 INCR 驱动策略、1 项是 stylus 协议转换；
真正影响日常体验的只有 X11 screen DPI（有兜底）、`wl_pointer` 坐标变换
（默认 `--viewport-resize` 关闭时不影响已验证路径）与 IME host 侧确认。

---

## 2. 启动链路对照：为什么能顶上去

真机上主线由 4 个 systemd user 单元组成（节选自 `systemctl --user cat`）：

| 主线单元 | 实际命令 | 我们的等价形态 |
|---|---|---|
| `sommelier@0.service` | `--parent --sd-notify="READY=1" --socket=wayland-0 --stable-scaling --enable-linux-dmabuf /bin/sh -c '...'` | `--parent --socket=wayland-0 ...`（`--parent` 是真 listener，不是空壳） |
| `sommelier@1.service` | 同上，`wayland-1`（low-density） | **建议保留主线不动**，作 A/B 对照与应急 |
| `sommelier-x@0.service` | `-X --x-display=0 --sd-notify="READY=1" --no-exit-with-child --x-auth=... --stable-scaling --enable-xshape --enable-linux-dmabuf /bin/sh -c '...'` | 单实例里的 `-X --x-display=0` |
| `sommelier-x@1.service` | 同上，`:1` | 保留主线不动 |

支撑这个映射的源码与真机证据：

- **CLI 解析**：主线在首个位置参数处停止解析选项（`sommelier.cc:4354-4364`）；
  我们用 `trailing_var_arg = true, allow_hyphen_values = true` 复刻，
  尾随 `/bin/sh -c ...` 会作为 `runprog` 被真正执行（`main.rs:70` 的 `spawn_runprog`）。
- **`--sd-notify`**：主线传 `"READY=1"`，我们两种写法都收（`num_args = 0..=1`），
  `proxy.rs:1153` 在 listener bind 成功后才发 `READY=1`，`Type=notify` 不会超时。
- **X11 环境变量顺序**：`xwayland.rs:308` 先 `set_var("DISPLAY", ...)`，
  `main.rs:205-207` 才启动 `runprog`，所以主线尾随命令里的
  `systemctl --user set-environment DISPLAY=$${DISPLAY}`、`xauth add`、
  `. /etc/sommelierrc` 能拿到正确值。
- **快捷键**：`SOMMELIER_ACCELERATORS`（真机 `~/.config/systemd/user/sommelier-x@.service.d/override.conf`
  里有自定义值）由 `state.rs:505` 读取，drop-in 不会丢用户 override。

### 关于 `--peer-pid` / `--client-fd`：此前文档有误，这里更正

早期文档写「真机上现役主线实例确实在被 QQ / VSCode 用 `--peer-pid --client-fd`
调用」，并把它列为替换阻塞项。**复查后这个描述不准确**：

- 这两个参数由主线 **parent 内部 fork worker 时自己生成**
  （`sommelier.cc:3962-4022`：`accept()` → `SO_PEERCRED` → `--peer-pid=<pid>
  --client-fd=<fd>` → exec 自己），不是 ChromeOS 外部调用契约；
  真机 grep 容器的 `/opt/google/cros-containers` 与 systemd 单元目录，
  没有找到任何外部 caller。
- 因此**把 parent 一起换掉，这两个参数自然消失**，不会导致启动失败。
- `v0.3.6-r` 起这两个参数**不再是占位**：`accept()` 后我们用
  `SO_PEERCRED` 取到同一个 pid（显式 `--peer-pid` 优先），注入
  `Context.client_pid` 并写入连接日志；`--client-fd` 会作为首个预连接
  client 被服务。真机日志实测 `peer_pid=Some(20355)`。
- 仍存的细微差别：主线把 peer pid 进一步用于 app_id / 窗口归属推断，
  我们目前只用于连接身份与日志；每个 guest client 一条独立 host
  connection，不做跨连接的归属关联，少数依赖精确 app_id 的场景
  可能与主线略有差异。

---

## 3. 一个必须知道的结构差异（会影响你怎么配）

主线是**两个进程**：`sommelier@0` 监听 `wayland-0`（Wayland 原生应用），
`sommelier-x@0` 单独跑 Xwayland `:0`（X11 应用），二者各自连 host。
我们是**一个进程同时**监听 guest socket 并跑 Xwayland —— 现役
`sommelier-rs-r-x@wayland-wbx.service` 就是 `--socket=wayland-wbx -X --x-display=9`
的单实例形态，真机已验证。

关键约束：**`--socket` 默认是 `wayland-0`（`config.rs:472`），且每个实例都会
`UnixListener::bind`（`proxy.rs:1144`）**。所以不能把主线两个 unit 都原样换成
我们的 binary 而不给第二个显式改名，否则两个进程抢同一个 socket。
这也是包内 `packaging/systemd/sommelier-rs-r-x@.service` 注释强调
「两个 service 不能共用实例名」的原因。

由此有两条路径：**方案 B（推荐，单实例顶替 0 号这一对）** 或
**方案 A（保守，两个 unit 都换但给 X11 那个显式换 socket 名）**。

---

## 4. 前置自检（有任何一项不满足就先别往下走）

```bash
# 版本与包
/usr/bin/sommelier-rs-r --version          # 应为 0.3.7-r
dpkg -l sommelier-rs-r | tail -2

# 共存实例必须已经把你要用的应用全跑通
systemctl --user is-active sommelier-rs-r-x@wayland-wbx.service
WAYLAND_DISPLAY=wayland-wbx python3 /tmp/wb_new_globals_probe.py   # RESULT PASS

# 记录现状，回滚时用来比对
systemctl --user cat sommelier@0.service > ~/sommelier@0.before
systemctl --user cat sommelier-x@0.service > ~/sommelier-x@0.before
DISPLAY=:0 xdpyinfo | grep -E 'dimensions|resolution' > ~/dpi.before
systemctl --user show-environment > ~/env.before

# 确认 host 通道与渲染节点
test -e /dev/wl0 && test -c /dev/dri/renderD128 && echo transport-ok
```

另外请先保存所有 Linux 应用里的工作：**切换会重启 `:0`，当前 X11 窗口会关闭**。

---

## 5. 灰度策略：只切 `0`，保留 `1`

主线 `wayland-1` / `:1` 是 low-density 备用显示。建议**保持 `sommelier@1`
与 `sommelier-x@1` 完全不动**：

- 出问题可立刻用 `DISPLAY=:1` / `WAYLAND_DISPLAY=wayland-1` 继续工作；
- 可以 A/B 对比同一应用在主线与 Rust 下的行为；
- 回滚锚点始终在线。

只处理 `sommelier@0.service` 与 `sommelier-x@0.service`。

---

## 6. 方案 B（推荐）：单实例接管 `wayland-0` / `:0`

思路：给 `sommelier-x@0.service` 加一个 drop-in，把它的 `ExecStart` 换成
「我们的单实例命令」，同时 `mask` 掉 `sommelier@0.service`（防止它抢
`wayland-0`）。这样只改一处 + 一个 mask，环境变量、`ExecStopPost`、
`Restart=always` 和用户自己的 `override.conf` 全部保留。

### 6.1 建 drop-in

```bash
mkdir -p ~/.config/systemd/user/sommelier-x@0.service.d
```

新建 `~/.config/systemd/user/sommelier-x@0.service.d/10-rs-r.conf`：

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/sommelier-rs-r \
    --transport=auto --virtio-wl /dev/wl0 \
    --socket=wayland-0 \
    -X --x-display=0 \
    --sd-notify="READY=1" \
    --no-exit-with-child \
    --x-auth=${HOME}/.Xauthority \
    --stable-scaling --enable-xshape --enable-linux-dmabuf \
    /bin/sh -c 'PASTE_UPSTREAM_TAIL_COMMAND_HERE'
```

> **尾随命令段不要手敲**：直接执行
> `systemctl --user cat sommelier-x@0.service`，把原 `ExecStart` 里
> `/bin/sh -c "..."` 的整段引号内容原样复制过来，再补一行
> `systemctl --user set-environment WAYLAND_DISPLAY=wayland-0;`（原来由
> `sommelier@0` 负责，现在被 mask 了）。
> 主线这里的 `$${DISPLAY}` / `${HOME}` / `$(xxd ...)` 转义很细，复制比重写安全。
>
> 这段命令会完成：`set-environment DISPLAY` / `XCURSOR_SIZE` /
> `WAYLAND_DISPLAY`、`import-environment SOMMELIER_VERSION`、
> `xauth add :0`、`. /etc/sommelierrc`。我们的 `spawn_runprog` 会执行它，
> 且 `start_x11` 已先把 `DISPLAY` 设好（见 §2）。

**§11 真机演练实际使用的精简 drop-in**（已跑通，可直接复制）：

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/sommelier-rs-r \
    --transport=auto --virtio-wl /dev/wl0 \
    --socket=wayland-0 \
    -X --x-display=0 \
    --sd-notify=READY=1 \
    --no-exit-with-child \
    --stable-scaling --enable-xshape --enable-linux-dmabuf \
    /bin/sh -c 'systemctl --user set-environment WAYLAND_DISPLAY=wayland-0 DISPLAY=:0; . /etc/sommelierrc'
```

> 与上面「原样复制主线」版的区别只有两点：① 省略 `--x-auth`，
> 与包内两个 unit 模板一致，Xwayland 不启用 Xauthority；② 尾随命令
> 只保留 `set-environment` 与 `. /etc/sommelierrc`。
> **两者必须配套**：保留 `--x-auth` 就一定要保留主线尾随命令里的
> `xauth add :0`，否则 X11 客户端会因认证失败连不上 `:0`。
> 演练选择精简版是为了完全不改动 `~/.Xauthority`。

先静态校验，别急着重启：

```bash
systemd-analyze --user verify sommelier-x@0.service
```

### 6.2 切换

```bash
systemctl --user daemon-reload

# 先停 0 号主线（会关掉 :0 上的窗口），并禁止它自启
systemctl --user stop sommelier@0.service sommelier-x@0.service
systemctl --user mask sommelier@0.service

systemctl --user start sommelier-x@0.service
systemctl --user is-active sommelier-x@0.service     # active
```

`Type=notify` 会在我们的 listener bind + Xwayland 就绪后收到 `READY=1`。
若 90 秒内没起来，直接跳到 §9 回滚。

### 6.3 环境变量

主线用尾随命令写 user manager 环境；若你的应用仍读到旧值，补一次：

```bash
systemctl --user set-environment WAYLAND_DISPLAY=wayland-0
systemctl --user set-environment DISPLAY=:0
```

---

## 7. 方案 A（保守）：贴近主线拓扑，两个 unit 都换

如果你希望保留「Wayland parent / X11 各一个进程」的结构，就给两个 unit
分别加 drop-in，**并且必须给 X11 那个显式换一个 socket 名**（§3）：

| unit | 关键参数 |
|---|---|
| `sommelier@0.service` | `--parent --socket=wayland-0 --sd-notify="READY=1" --stable-scaling --enable-linux-dmabuf`，尾随命令原样 |
| `sommelier-x@0.service` | `-X --x-display=0 --socket=wayland-0-x ...`（`wayland-0-x` 只是满足 bind，没有 client 会连它） |

两者都要加 `--transport=auto --virtio-wl /dev/wl0`。代价是多一条 host
连接、多一个无用 socket；好处是拓扑更接近主线，单个进程挂掉影响面更小。
同样先 `systemd-analyze --user verify`，再按 §6.2 的顺序切换（不 mask
`sommelier@0`，而是两个都 start）。

---

## 8. 验收清单

切换后逐项打勾；任何一项失败先记录日志再回滚。
**下面的 bash 检查项已在 `v0.3.6-r` 真机演练中全部自动跑过并 PASS**（数据见 §11），
人工项仍需你自己在 ChromeOS 侧确认。

```bash
# 基本
systemctl --user is-active sommelier-x@0.service
/usr/bin/sommelier-rs-r --version

# Wayland 侧
ls -l "$XDG_RUNTIME_DIR/wayland-0"
WAYLAND_DISPLAY=wayland-0 python3 /tmp/wb_new_globals_probe.py    # RESULT PASS
WAYLAND_DISPLAY=wayland-0 python3 /tmp/wb_wl_probe.py              # errors=0

# X11 侧
DISPLAY=:0 xdpyinfo | grep -E 'dimensions|resolution'
DISPLAY=:0 xrandr | head -3
DISPLAY=:0 xprop -root _NET_SUPPORTING_WM_CHECK

# 日志里不应再出现报错/warning（DEBUG 级 BadWindow 记录属预期，需排除）
journalctl --user -u sommelier-x@0.service --since '5 minutes ago' --no-pager \
  | grep -E "glamor: 'wl_drm' not supported|falling back to sw|WARN.*BadWindow|ERROR.*BadWindow" \
  || echo clean
```

人工确认（**代码无法自证，最影响"能不能长期换"**；标注 ✅ 的已在演练中自动验证）：

- ✅ 真实 X11 窗口：`DISPLAY=:0 xmessage` 显示成功，WM `_NET_SUPPORTING_WM_CHECK` 存在
- ✅ 保留的 `DISPLAY=:1` / `wayland-1` 主线实例全程 `active`，`:1` 几何未变
- ✅ `--parent` 多客户端：隔离 socket 上两个独立 wire 探针先后连接均 PASS
- [ ] 从 ChromeOS 应用启动器打开一个 Linux GUI 应用，窗口正常显示、缩放正确、键盘焦点正常
- [ ] 在 Linux 应用里复制文字 → 粘贴到 ChromeOS 原生应用，再反向来一次
- [ ] 在 Linux 应用输入框里用 ChromeOS 输入法打字（Crostini IME）
- [ ] 注销并重新登录容器后，服务仍 `active`，`WAYLAND_DISPLAY` / `DISPLAY` 正确
- [ ] 连续开多个应用并运行数天（验证 `--parent` accept 循环长期稳定与真实 GPU 加速）

---

## 9. 一键回滚

```bash
#!/bin/sh
set -e
# 方案 B 回滚
systemctl --user stop sommelier-x@0.service 2>/dev/null || true
systemctl --user unmask sommelier@0.service
rm -f ~/.config/systemd/user/sommelier-x@0.service.d/10-rs-r.conf
rmdir  ~/.config/systemd/user/sommelier-x@0.service.d 2>/dev/null || true
# 方案 A 回滚：另删 sommelier@0.service.d/10-rs-r.conf
rm -f ~/.config/systemd/user/sommelier@0.service.d/10-rs-r.conf
systemctl --user daemon-reload
systemctl --user start sommelier@0.service sommelier-x@0.service
systemctl --user is-active sommelier@0.service sommelier-x@0.service || true
DISPLAY=:0 xdpyinfo | grep -E 'dimensions|resolution' > /tmp/dpi.after
diff -u ~/dpi.before /tmp/dpi.after || true
```

回滚只删用户级 drop-in 与 mask，**不改 `/usr/bin/sommelier` 的
alternatives 链接、不改 `/opt/google/cros-containers`、不卸载 `.deb`**。

> **此回滚流程已在真机演练中完整执行并验证**：drop-in 与 mask 清除、
> 5 个单元全 `active`、`:0` 恢复 `406x270mm` / 120 DPI / `Xft.dpi: 120`、
> 四个 `DISPLAY` / `WAYLAND_DISPLAY` 环境变量全部还原。演练脚本用
> `trap rollback EXIT HUP INT TERM` 保证即使中断也一定回滚，见 §11。
极端情况下（systemd user manager 起不来）可从容器外重新登录，或直接：

```bash
sudo dpkg -r sommelier-rs-r        # 我们从不接管主线二进制，卸掉即恢复
```

---

## 10. 已知降级与缓解

| 现象 | 原因 | 缓解 |
|---|---|---|
| `DISPLAY=:0 xdpyinfo` 显示 96 DPI / `508x339mm`（主线 120 DPI） | Xwayland 启动时按 `width × 25.4 / monitorResolution` 定死 X screen mm；真机三项实验（首帧 seeding DSF、`-dpi 120`、`xrandr --dpi 120`）均无效 | `xrandr` 的 RandR monitor 已是 `406x270mm`。`/etc/sommelierrc` 会把 `xdpyinfo` 的 DPI 写成 `Xft.dpi`；在 `~/.sommelierrc` 追加 `echo "Xft.dpi: 120" \| xrdb -merge` 即可让读 Xft 的老式 X11 应用与主线一致 |
| 少数应用的 app_id / 窗口归属与主线略不同 | 每 guest client 独立 host connection，不复用主线 peer pid 关联 | 通常无感；若某应用窗口分组异常，记录到 issue |
| Crostini IME 在 ChromeOS 侧打字未验证 | 非 Xwayland 连接只能创建 placeholder `wl_surface` | 按 §8 人工确认；异常时保留 `sommelier-x@1` 主线对照 |
| stylus-v2 → tablet-v2 转换 / `wl_pointer` 坐标变换 | 主线做协议转换与指针坐标补偿，我们透传 tablet-v2、`wl_pointer` 事件纯转发（`host_to_guest_fixed` 未接入生产路径） | `--viewport-resize` 默认关闭时不影响已验证路径；见 `MAINLINE_PARITY.md` §4 |
| 帧统计不是 Perfetto | `--timing-filename` / `--stats-*` / `--trace-*` 已实现为 CSV / 文本 / 逐消息 trace，而非 Perfetto trace 格式 | 需要 Perfetto 消费端的工具链不适用；其余可观测性需求已覆盖 |

---

## 11. 真机切换演练记录（`v0.3.6-r`）

「整套切换到底行不行」已经不再靠推测：在 Debian 13 / `penguin` /
`/dev/wl0` 上用 §6.1 的精简 drop-in 完整跑过一轮，**并自动回滚到主线**。
演练脚本带 `trap rollback EXIT HUP INT TERM`，无论成功、失败或中断都
无条件恢复；`sommelier@1` / `sommelier-x@1` 全程保持运行作 A/B 与应急锚点。

**切换态结果（全 PASS）**

| 检查 | 实测 |
|---|---|
| `sommelier-x@0.service` | 秒级 `active`，`Type=notify` 正常收到 `READY=1` |
| guest socket | `/run/user/1000/wayland-0` 由我们 bind |
| wire 探针 | `TOTAL_GLOBALS 26`、`wl_drm` 全字段、IME binds OK、`RESULT PASS` |
| Wayland 主链路 | geometry / mode / scale / done / seat capabilities 全 PASS，`errors=0` |
| Xwayland / WM | `X11 forwarding ready on DISPLAY=:0`、`WM initialized`、`_NET_SUPPORTING_WM_CHECK` 存在 |
| 真实 X11 窗口 | `DISPLAY=:0 xmessage` 成功（`rc=0`） |
| **`wl_drm` PRIME** | `:0` 上的 Xwayland 实际调用 `create_prime_buffer 104x54 / 106x56 fmt=0x34325258` |
| RandR | `XWAYLAND0 connected 1920x1280 ... 406mm x 270mm`，与主线一致 |
| 保留的主线 `@1` | 两单元 `active`，`:1` = `960x640 (254x169mm)` |
| 日志 | 无 `glamor` / `wl_drm` 报错，无 `BadWindow` warning（DEBUG 级记录属预期） |
| X screen DPI | `508x339mm` / 96 DPI —— §10 已知差距在替换态同样成立 |

**回滚后复核**：drop-in 文件消失、mask symlink 移除；
`sommelier@0/1`、`sommelier-x@0/1`、共存
`sommelier-rs-r-x@wayland-wbx` **5 个单元全部 `active`**；
`:0` 恢复 `1920x1280 (406x270mm)` / 120 DPI / `Xft.dpi: 120`，
`:9` 仍为 `508x339mm`；`DISPLAY=:0`、`WAYLAND_DISPLAY=wayland-0`、
`DISPLAY_LOW_DENSITY=:1`、`WAYLAND_DISPLAY_LOW_DENSITY=wayland-1`
四个环境变量全部还原。

**本轮新增能力的真机实测**：`--timing-filename --stats-timer=1` 产出
4 行 CSV（8 requests / 69 events / 200 B / 2116 B）、`--stats-summary`
输出实时快照、`--trace-filename --trace-system` 逐消息记录
`C2H/H2C <interface> <opcode> <len>` 与进程头；
`SO_PEERCRED` 在连接日志给出 `peer_pid=Some(20355)`。
`cargo test --workspace --locked` **160 个测试全绿**（156 + 4 + 0）、
`cargo build --release --locked` **0 warning**、`cargo fmt --check` 通过。

### 仍然需要人来做的事

诚实列出，避免把"切换演练通过"误读成"长期生产已验证"：

1. **ChromeOS host 侧人工粘贴 / IME** 必须由人操作确认。
2. **长时间稳定性、真实 GPU 加速路径** 需要数天连续使用。
3. **注销/重登后自启**：演练验证了回滚会恢复环境与单元，但
   「替换态下注销再登录」尚未跑过。
4. **切换会关掉当前 `:0` 上的窗口**——演练已自动恢复主线，
   你正式切换前仍需先保存工作。

完整的功能差距与验证记录见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4 / §4.1 / §5.7 与
[`FEATURE_MATRIX.md`](FEATURE_MATRIX.md) §6 / §7。
