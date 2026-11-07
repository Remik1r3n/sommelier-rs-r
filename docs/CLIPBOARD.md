# 剪贴板：主线实现与 sommelier-rs-r 的对照

## 1. 结论先行

**主线 C++ `sommelier` 本来就有完整的 X11 ↔ host 双向剪贴板。** 它不是「缺失功能」，
也不需要先在真机上验证才有资格实现——主线源码本身就是可照抄的规格。

`sommelier-rs`（`main` 与 `virtwl` 两个分支）都没有 X11，因此也没有这条链路；
它们只做 guest `wl_data_device` ↔ host `wl_data_device` 的代理转发。
`sommelier-rs-r` 补 X11 时，第一版只做到「X11 ↔ guest Wayland」，把 X11 内容
合成成了 **guest** 侧的 `wl_data_offer`。这与主线的方向不同：主线是把 X11 内容
合成 **host** 侧的 `wl_data_source` 并 `set_selection`，从而让 ChromeOS host 和
所有 guest 应用同时看到它。

## 2. 主线代码位置

| 主线位置 | 作用 |
|---|---|
| `sommelier/sommelier.cc:336` `sl_set_selection()` | host selection 变化时，把 `wl_data_offer` 的 MIME 转成 X11 atom，调用 `xcb_set_selection_owner(CLIPBOARD)`，让 WM 成为 X11 owner |
| `sommelier/sommelier.cc:2786` `sl_get_selection_targets()` | X11 owner 变化时读 `_WL_SELECTION` 的 TARGETS，`xcb_get_atom_name` 批量转字符串 → `wl_data_device_manager_create_data_source()` → 每个名字 `wl_data_source_offer()` → `wl_data_device_set_selection()` |
| `sommelier/sommelier.cc:3024` `sl_handle_xfixes_selection_notify()` | XFixes 检测 CLIPBOARD owner 变化；owner 是自己时只记 timestamp 后返回，避免回环 |
| `sommelier/sommelier.cc:2087` `sl_begin_data_source_send()` | host 请求数据时，把 MIME 转回 atom，`xcb_convert_selection()`，并把 host 给的 fd 设为非阻塞写端 |
| `sommelier/sommelier.cc:2211` `sl_handle_selection_fd_readable()` | 从 host `wl_data_offer.receive` 的管道读数据，用 INCR 或一次性 `xcb_change_property` 回答 X11 |
| `sommelier/sommelier.cc:2990` `sl_handle_selection_request()` | 以 WM 身份回答 X11 `SelectionRequest`（TARGETS / TIMESTAMP / 数据） |
| `sommelier/sommelier-data-device-manager.cc` | guest ↔ host 的 `wl_data_device_manager` / `wl_data_offer` / `wl_data_source` 代理，含 virtio_wl 跨 VM 管道 |

关键点：主线在 `sl_connect()` 里**内部绑定** `wl_data_device_manager` 并用默认 seat
创建自己的 `wl_data_device`。它不复用 guest 的 data device，因为 guest 随时可能
`set_selection(NULL)` 把导出冲掉。

## 3. 本项目的等价实现

| 主线 | sommelier-rs-r |
|---|---|
| 内部 bind `wl_data_device_manager` | `handler/registry.rs::on_global()`（仅 Xwayland 连接） |
| 默认 seat 的 `wl_data_device` | `x11/selection.rs::ensure_host_selection_device()`，seat id 从 guest 的 `get_data_device` 请求反查（`handler/data_device.rs`） |
| `sl_get_selection_targets()` | `SelectionBridge::export_x11_targets_to_host()`：`create_data_source` → `offer(mime)`* → `set_selection(source, host_serial)` |
| `sl_begin_data_source_send()` | `SelectionBridge::on_host_source_send()`：`ConvertSelection` → 异步写回 host fd |
| `sl_set_selection()` | `wm.rs::handle_wm_event(WmEvent::ClipboardTargets)` → `ClipboardBridge::claim_clipboard()` |
| `sl_handle_xfixes_selection_notify()` | `clipboard.rs::handle_xfixes_selection_notify()`；owner 为自身窗口时忽略 |
| `last_serial` | `Context::host_serial`，由 `proxy.rs::maybe_update_host_serial()` 从 host 输入事件首字段提取 |

### 3.1 三个方向

```
X11 → ChromeOS host
  X11 app 复制 → XFixes owner 变化
    → WM: ConvertSelection(CLIPBOARD, TARGETS, _WL_SELECTION)
    → X11Bridge::set_clipboard_targets(names)   [generation++]
    → SelectionBridge::poll() 发现 generation 变化
    → host: create_data_source / offer(mime)* / set_selection(source, serial)
  ChromeOS host 粘贴
    → host: wl_data_source.send(mime, fd)  [内部对象事件，proxy 直接消费]
    → WmEvent::RequestClipboardData
    → WM: ConvertSelection(CLIPBOARD, atom(mime), _WL_SELECTION)
    → X11Bridge::set_clipboard_data(atom, bytes)
    → proxy: 写回 host 提供的 fd 并关闭

X11 → guest Wayland
  XFixes owner 变化 → TARGETS → 合成 guest wl_data_device.data_offer /
  offer / selection；wl_data_offer.receive → ConvertSelection → 写回 guest fd

host / guest Wayland → X11
  host offer 或 guest data_source 的 MIME / 文本载荷
    → WmEvent::ClipboardTargets / ClipboardData
    → ClipboardBridge::note_targets / note_data + claim_clipboard()
    → X11 SelectionRequest 由 WM 以 TARGETS / TIMESTAMP / 数据回答
```

### 3.2 防止回环与污染的三个措施

1. **内部对象隔离**：`Context::internal_selection_ids` 记录内部 bind 的 manager /
   data device / 所有 source 的 host id。`proxy.rs::handle_msgs()` 命中后走
   `handle_internal_selection_event()`，只消费 `wl_data_source.send`，一律返回
   `Ok(None)`，绝不把 host 命名空间的对象 id 转发给 guest。旧 source 退役后仍留在
   集合里，host 迟到的 `send` / `cancelled` 也会被引擎吞掉。
2. **selection 回声过滤**：我们导出的 selection 会被 host 广播回内部 data device，
   `SelectionBridge::on_host_selection()` 检测到 sender 是内部对象时直接返回，
   不会让 WM 把 CLIPBOARD owner 从刚复制的 X11 应用手里抢走。
3. **缓存失效**：`set_clipboard_targets()` 清空 payload 缓存（新 owner 不能用旧
   owner 的字节应答）；`on_host_source_send()` 再对单个 target 调
   `invalidate_clipboard_data()`；WM 侧 owner 清空时发布空 TARGETS，proxy 因此
   发出 `wl_data_device.set_selection(0, serial)`。

### 3.3 MIME ↔ X11 target：与主线一致的 1:1 透传

主线**没有**任何 MIME 映射表：`sl_get_selection_targets()` 把 X11 atom 名
（`UTF8_STRING`、`image/png`…）原样作为 `wl_data_source.offer()` 的 MIME，
`sl_send_data()` 再用 `xcb_get_atom_name()` 把 X11 请求的 target 原样作为
`wl_data_offer.receive()` 的 MIME。X11 侧唯一"新增"的 target 是
`sl_set_selection()` 在数组开头插入的 `TARGETS` 与 `TIMESTAMP`。

实现与主线保持一致：

- `mime_for_x11_target()` / `x11_target_for_mime()` 恒等，控制 atom
  （`TARGETS`/`TIMESTAMP`/`MULTIPLE`/`SAVE_TARGETS`/`_WL_SELECTION`）过滤；
- `normalize_targets()` 先发主线的 `TARGETS`, `TIMESTAMP`，再发 MIME 原名，
  最后追加 `UTF8_STRING`/`STRING`/`TEXT` 作为**额外别名**（不改写 MIME）；
- `preferred_x11_target()` 仅在向 X11 owner **预取**文本数据时优先用
  `UTF8_STRING`（X11 owner 很少 intern 字面 MIME 名）；
- `x11_target_candidates()` 让 `SelectionRequest` 无论请求 MIME 原名还是经典
  atom，都能解析到同一个 Wayland offer。

INCR 也按主线对齐：阈值与 chunk 都是 64 KiB（`sommelier.cc:2214`），
INCR property 写的是 **chunk size** 而非总长度（`:2250-2257`）。

## 4. 已知取舍

- **serial 可能为 0**：`Context::host_serial` 来自真实 host 输入事件（
  `maybe_update_host_serial()` 覆盖 pointer / keyboard / touch / data_device 的
  输入 opcode）。进程刚启动、还未收到任何输入时复制，主线同样会用
  `last_serial == 0` 发送 `set_selection`；`v0.3.6-r` 起这条日志降为 debug，
  不再打 warning。ChromeOS compositor 若严格校验 serial 仍可能拒绝这一次
  `set_selection`，下一次输入事件后恢复正常。
- **X11 owner 会被 WM 接管**：host / Wayland 侧复制时，`claim_clipboard()` 让 WM
  成为 X11 owner（主线 `sl_set_selection` 同款行为）。之后 X11 应用粘贴走 WM，
  WM 用预取或延迟 `SelectionRequest` 从 Wayland 侧取数据；这与主线的绕行方式一致。
- **大载荷**：Wayland → X11 方向只预取文本类与常见图片 MIME，上限 4 MiB；
  其余由延迟 `SelectionRequest` 按需获取，1.5 s 超时按 ICCCM 回 property NONE。
  X11 → host 方向由 `ConvertSelection` 直接写 fd，不受预取上限约束。
- **真机端到端结论（已验证）**：在 Debian 13 / `penguin` / `/dev/wl0` 的
  Crostini 真机上，X11 侧 `printf 'wb-e2e-clip-7788' | xclip -selection clipboard`
  后 `xclip -o` 能原样读回；`TARGETS` 返回 `TARGETS` + `UTF8_STRING`
  （主线 1:1 原名，无 `text/plain` 改写）。同一时刻，连到 guest socket 的
  Wayland 客户端收到 `wl_data_device.data_offer` 与 `selection`，offer 的
  MIME 就是 `UTF8_STRING` 原名，证明 **X11 → guest → host 剪贴板桥接链路
  已打通**。**仍未人工确认**的是 ChromeOS host 侧实际「粘贴」动作（容器
  内无法驱动 host UI），以及 `host_serial == 0` 时首次导出是否被 host 接受。
  详见 `MAINLINE_PARITY.md` §5。

详见 `FEATURE_MATRIX.md` §6。
