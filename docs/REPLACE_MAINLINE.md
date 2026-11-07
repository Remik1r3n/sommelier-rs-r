# 把 Crostini 的主线 sommelier 换成 sommelier-rs-r

> 适用版本：`v0.3.6-r`。本文是**可选的进阶操作**；默认安装方式见
> [`CROSTINI_INSTALL.md`](CROSTINI_INSTALL.md) §2，只新增共存实例，不动主线。
>
> **状态声明（请先读）**：本文给出的启动参数映射、CLI 兼容性、
> `--parent` 多客户端能力、单实例同时提供 Wayland + X11、sd-notify、
> 尾随命令执行链，均已在 Debian 13 / `penguin` / `/dev/wl0` 真机逐项验证。
> 但**「把 `wayland-0` / `:0` 整套切过去并长期运行」这一完整演练尚未执行**——
> 它会中断当前桌面上的 Linux 应用，需要你确认后再动手。本文因此按
> 「可灰度、可一键回滚」编写，不是"生产无风险"承诺。

---

## 1. 现在到底能不能换？

先给结论：**协议与功能面差得不多了，剩下的主要不是"再补几个功能"，而是
"做一次生产切换演练 + 几项必须由人在 ChromeOS 侧确认的验证"。**

| 层次 | 状态 | 说明 |
|---|---|---|
| Wayland / X11 主链路 | ✅ 已验证 | 154 个测试、release 0 warning、wire 探针 PASS、真实 X11 窗口映射到 host `xdg_toplevel`、剪贴板往返、`wl_drm.create_prime_buffer` 被真实 Xwayland 调用 |
| 主线 CLI / 环境变量 / global 版本 | ✅ 已对齐 | 30 余处语义偏差已修正，见 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §1 |
| 主线 unit 的 `ExecStart` 参数 | ✅ 已验证可解析 | `--parent`、`--sd-notify="READY=1"`、`--no-exit-with-child`、`--x-auth`、`--enable-xshape`、`--stable-scaling`、`--enable-linux-dmabuf`、尾随 `/bin/sh -c ...` 全部能吃下 |
| `--parent` 多客户端 | ✅ 已验证 | `UnixListener` + `accept()` 循环 + 每 client 一个 task；真机隔离 socket 上两个独立探针先后连接均 PASS |
| X11 screen DPI | ⚠️ 有条件 | `xrandr` monitor 已是主线同款 `406x270mm`，但 `xdpyinfo` X screen 仍 96 DPI（主线 120）。可用 `Xft.dpi` 兜底，见 §9 |
| Crostini IME host 侧交互 | ⚠️ 有条件 | 协议与 XID→surface 解析已实现，但「在 ChromeOS 界面里真正打字」需人工确认 |
| 由 client pid 推断 app_id / 窗口归属 | ⚠️ 有条件 | 这是我们「只解析不实现 `--peer-pid`」的实际后果，见 §2 末尾 |
| 完整 drop-in 切换一整轮 | ❌ 未执行 | 会中断当前桌面；需你点头后我们在真机跑，见 §10 |
| ChromeOS host 侧人工粘贴 / 长时间稳定性 / 真实 GPU 加速 | ❌ 未覆盖 | 需要人或长时间运行，代码侧无法自证 |

按 [`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4 的差距表数：`v0.3.6-r` 剩 10 项，
其中 4 项是"实现路径不同但效果接近"、2 项是 Perfetto/stats 等可观测性、
1 项是 INCR 驱动策略；真正影响日常体验的只有上面 3 个 ⚠️。

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
- 实际代价是：主线用 peer pid 做 client 身份 / app_id / 窗口归属推断；
  我们每个 guest client 一条独立 host connection，不做这层关联，
  所以少数依赖精确 app_id 的场景可能与主线略有差异。

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
/usr/bin/sommelier-rs-r --version          # 应为 0.3.6-r
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

# 日志里不应再出现
journalctl --user -u sommelier-x@0.service --since '5 minutes ago' --no-pager \
  | grep -E "glamor: 'wl_drm' not supported|falling back to sw|BadWindow" || echo clean
```

人工确认（**这三项代码无法自证，最影响"能不能长期换"**）：

- [ ] 从 ChromeOS 应用启动器打开一个 Linux GUI 应用，窗口正常显示、缩放正确、键盘焦点正常
- [ ] 在 Linux 应用里复制文字 → 粘贴到 ChromeOS 原生应用，再反向来一次
- [ ] 在 Linux 应用输入框里用 ChromeOS 输入法打字（Crostini IME）
- [ ] 保留的 `DISPLAY=:1` / `wayland-1` 主线实例仍可正常使用
- [ ] 注销并重新登录容器后，服务仍 `active`，`WAYLAND_DISPLAY` / `DISPLAY` 正确
- [ ] 连续开多个应用（验证 `--parent` accept 循环在真实负载下稳定）

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
| stylus / viewport 指针补偿 / Perfetto 帧统计 | 实现路径不同或未复刻，见 `MAINLINE_PARITY.md` §4 | 不影响协议兼容与日常使用 |

---

## 11. 我们还没替你做的事

诚实列出，避免把"能力具备"误读成"已经替你验证过生产切换"：

1. **完整 drop-in 切换演练未在真机执行**——它会关掉你当前 `:0` 上的窗口。
   需要你确认后，我们可以按 §6 → §8 → 长期观察 → §9 在真机跑一整轮，
   并把结果写回本文与 `MAINLINE_PARITY.md` §5。
2. **ChromeOS host 侧人工粘贴 / IME** 必须由人操作确认。
3. **长时间稳定性、真实 GPU 加速路径** 需要数天连续使用。
4. **注销/重启后自启** 依赖 §6.3 的 `set-environment` 与 systemd 单元启用状态，
   尚未在完整切换形态下验证。

完整的功能差距与验证记录见
[`MAINLINE_PARITY.md`](MAINLINE_PARITY.md) §4 / §5 与
[`FEATURE_MATRIX.md`](FEATURE_MATRIX.md) §6。
