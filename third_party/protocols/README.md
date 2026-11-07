# Wayland 协议 XML 定义（third_party/protocols/）

本目录存放加强版 Wayland proxy 使用的 Wayland 协议 XML。`wayland_codegen`
（基于 `quick-xml` + `serde`，见 `wayland_codegen/src/protocol.rs`）在构建期解析
这些文件并生成 Rust 代码；`<protocol name="...">` 即生成的 Rust 模块名。

## 本次补齐内容

- **本地复制 12 个**：直接取自 ChromeOS 主线 sommelier 的
  `sommelier/protocol/`，保持原文件名不变。
- **上游下载 10 个**：通过本地代理 `127.0.0.1:1088` 从 wayland-protocols 上游
  `main` 分支 raw 文件下载（gitlab.freedesktop.org）。
- **原有 11 个**：按要求保留不动。
- 合计 **33 个 `.xml`**，另加本 `README.md`（本目录唯一允许新建的非 XML 文件）。

> **重要提示**：`sommelier/build.rs` 目前以**显式数组**只登记了原有 11 个协议，
> 并不扫描本目录。因此本次新增的 22 个文件已就位，但**尚未接入构建**；如需启用，
> 需在 `build.rs` 中登记（本次任务禁止修改 build.rs / main.rs / *.rs / Cargo.toml）。

## 文件清单

表头说明：「来源」区分 原有 / 本地复制 / 上游下载；「interfaces」为
`name@version`（version 取自 `<interface version>`）；「许可」由各文件
`<copyright>` 文本判定。

| 文件 | 来源 | protocol name（模块名） | interfaces (name@version) | 许可 |
|---|---|---|---|---|
| `aura-shell.xml` | 原有（保留不动） | `aura_shell` | zaura_shell@38, zaura_surface@38, zaura_output@38, zaura_toplevel@38, zaura_popup@38 | MIT/X11-like |
| `content-type-v1.xml` | 上游下载 `staging/content-type/content-type-v1.xml` | `content_type_v1` | wp_content_type_manager_v1@1, wp_content_type_v1@1 | MIT/X11-like |
| `cursor-shape-v1.xml` | 上游下载 `staging/cursor-shape/cursor-shape-v1.xml` | `cursor_shape_v1` | wp_cursor_shape_manager_v1@2, wp_cursor_shape_device_v1@2 | MIT/X11-like |
| `drm.xml` | 本地复制 `sommelier/protocol/drm.xml` | `drm` | wl_drm@2 | MIT/HPND-like |
| `fractional-scale-v1.xml` | 原有（保留不动） | `fractional_scale_v1` | wp_fractional_scale_manager_v1@1, wp_fractional_scale_v1@1 | MIT/X11-like |
| `gaming-input-unstable-v2.xml` | 本地复制 `sommelier/protocol/gaming-input-unstable-v2.xml` | `gaming_input_unstable_v2` | zcr_gaming_input_v2@2, zcr_gaming_seat_v2@1, zcr_gamepad_v2@2, zcr_gamepad_vibrator_v2@2 | MIT/X11-like |
| `gtk-shell.xml` | 本地复制 `sommelier/protocol/gtk-shell.xml` | `gtk` | gtk_shell1@1, gtk_surface1@1 | 无 `<copyright>` 块 |
| `idle-inhibit-unstable-v1.xml` | 本地复制 `sommelier/protocol/idle-inhibit-unstable-v1.xml` | `idle_inhibit_unstable_v1` | zwp_idle_inhibit_manager_v1@1, zwp_idle_inhibitor_v1@1 | MIT/X11-like |
| `keyboard-extension-unstable-v1.xml` | 原有（保留不动） | `keyboard_extension_unstable_v1` | zcr_keyboard_extension_v1@2, zcr_extended_keyboard_v1@2 | MIT/X11-like |
| `keyboard-shortcuts-inhibit-unstable-v1.xml` | 上游下载 `unstable/keyboard-shortcuts-inhibit/keyboard-shortcuts-inhibit-unstable-v1.xml` | `keyboard_shortcuts_inhibit_unstable_v1` | zwp_keyboard_shortcuts_inhibit_manager_v1@1, zwp_keyboard_shortcuts_inhibitor_v1@1 | MIT/X11-like |
| `linux-dmabuf-v1.xml` | 原有（保留不动） | `linux_dmabuf_v1` | zwp_linux_dmabuf_v1@5, zwp_linux_buffer_params_v1@5, zwp_linux_dmabuf_feedback_v1@5 | MIT/X11-like |
| `linux-drm-syncobj-v1.xml` | 上游下载 `staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml` | `linux_drm_syncobj_v1` | wp_linux_drm_syncobj_manager_v1@1, wp_linux_drm_syncobj_timeline_v1@1, wp_linux_drm_syncobj_surface_v1@1 | MIT/X11-like |
| `linux-explicit-synchronization-unstable-v1.xml` | 本地复制 `sommelier/protocol/linux-explicit-synchronization-unstable-v1.xml` | `zwp_linux_explicit_synchronization_unstable_v1` | zwp_linux_explicit_synchronization_v1@2, zwp_linux_surface_synchronization_v1@2, zwp_linux_buffer_release_v1@1 | MIT/X11-like |
| `pointer-constraints-unstable-v1.xml` | 本地复制 `sommelier/protocol/pointer-constraints-unstable-v1.xml` | `pointer_constraints_unstable_v1` | zwp_pointer_constraints_v1@1, zwp_locked_pointer_v1@1, zwp_confined_pointer_v1@1 | MIT/X11-like |
| `presentation-time.xml` | 上游下载 `stable/presentation-time/presentation-time.xml` | `presentation_time` | wp_presentation@3, wp_presentation_feedback@3 | MIT/X11-like |
| `primary-selection-unstable-v1.xml` | 上游下载 `unstable/primary-selection/primary-selection-unstable-v1.xml` | `wp_primary_selection_unstable_v1` | zwp_primary_selection_device_manager_v1@1, zwp_primary_selection_device_v1@1, zwp_primary_selection_offer_v1@1, zwp_primary_selection_source_v1@1 | MIT/X11-like |
| `relative-pointer-unstable-v1.xml` | 本地复制 `sommelier/protocol/relative-pointer-unstable-v1.xml` | `relative_pointer_unstable_v1` | zwp_relative_pointer_manager_v1@1, zwp_relative_pointer_v1@1 | MIT/X11-like |
| `single-pixel-buffer-v1.xml` | 上游下载 `staging/single-pixel-buffer/single-pixel-buffer-v1.xml` | `single_pixel_buffer_v1` | wp_single_pixel_buffer_manager_v1@1 | MIT/X11-like |
| `stylus-unstable-v2.xml` | 本地复制 `sommelier/protocol/stylus-unstable-v2.xml` | `stylus_unstable_v2` | zcr_stylus_v2@2, zcr_touch_stylus_v2@1, zcr_pointer_stylus_v2@2 | MIT/X11-like |
| `tablet-unstable-v2.xml` | 本地复制 `sommelier/protocol/tablet-unstable-v2.xml` | `tablet_unstable_v2` | zwp_tablet_manager_v2@1, zwp_tablet_seat_v2@1, zwp_tablet_tool_v2@1, zwp_tablet_v2@1, zwp_tablet_pad_ring_v2@1, zwp_tablet_pad_strip_v2@1, zwp_tablet_pad_group_v2@1, zwp_tablet_pad_v2@1 | MIT/X11-like |
| `tearing-control-v1.xml` | 上游下载 `staging/tearing-control/tearing-control-v1.xml` | `tearing_control_v1` | wp_tearing_control_manager_v1@1, wp_tearing_control_v1@1 | MIT/X11-like |
| `text-input-crostini-unstable-v1.xml` | 本地复制 `sommelier/protocol/text-input-crostini-unstable-v1.xml` | `text_input_crostini_unstable_v1` | zcr_text_input_crostini_manager_v1@1, zcr_text_input_crostini_v1@1 | BSD-style (ChromiumOS) |
| `text-input-extension-unstable-v1.xml` | 原有（保留不动） | `text_input_extension_unstable_v1` | zcr_text_input_extension_v1@11, zcr_extended_text_input_v1@11 | MIT/X11-like |
| `text-input-unstable-v1.xml` | 原有（保留不动） | `text_input_unstable_v1` | zwp_text_input_v1@1, zwp_text_input_manager_v1@1 | MIT/X11-like |
| `text-input-unstable-v3.xml` | 原有（保留不动） | `text_input_unstable_v3` | zwp_text_input_v3@1, zwp_text_input_manager_v3@1 | MIT/HPND-like |
| `text-input-x11-unstable-v1.xml` | 本地复制 `sommelier/protocol/text-input-x11-unstable-v1.xml` | `text_input_x11_unstable_v1` | zcr_text_input_x11_v1@1 | BSD-style (ChromiumOS) |
| `viewporter.xml` | 原有（保留不动） | `viewporter` | wp_viewporter@1, wp_viewport@1 | MIT/X11-like |
| `wayland.xml` | 原有（保留不动） | `wayland` | wl_display@1, wl_registry@1, wl_callback@1, wl_compositor@6, wl_shm_pool@2, wl_shm@2, wl_buffer@1, wl_data_offer@3, wl_data_source@3, wl_data_device@3, wl_data_device_manager@3, wl_shell@1, wl_shell_surface@1, wl_surface@6, wl_seat@10, wl_pointer@10, wl_keyboard@10, wl_touch@10, wl_output@4, wl_region@1, wl_subcompositor@1, wl_subsurface@1, wl_fixes@1 | MIT/X11-like |
| `xdg-activation-v1.xml` | 上游下载 `staging/xdg-activation/xdg-activation-v1.xml` | `xdg_activation_v1` | xdg_activation_v1@1, xdg_activation_token_v1@1 | MIT/X11-like |
| `xdg-decoration-unstable-v1.xml` | 原有（保留不动） | `xdg_decoration_unstable_v1` | zxdg_decoration_manager_v1@1, zxdg_toplevel_decoration_v1@1 | MIT/X11-like |
| `xdg-dialog-v1.xml` | 上游下载 `staging/xdg-dialog/xdg-dialog-v1.xml` | `xdg_dialog_v1` | xdg_wm_dialog_v1@1, xdg_dialog_v1@1 | MIT/X11-like |
| `xdg-output-unstable-v1.xml` | 本地复制 `sommelier/protocol/xdg-output-unstable-v1.xml` | `xdg_output_unstable_v1` | zxdg_output_manager_v1@3, zxdg_output_v1@3 | MIT/X11-like |
| `xdg-shell.xml` | 原有（保留不动） | `xdg_shell` | xdg_wm_base@7, xdg_positioner@7, xdg_surface@7, xdg_toplevel@7, xdg_popup@7 | MIT/X11-like |

上游文件完整 URL 前缀：
`https://gitlab.freedesktop.org/wayland/wayland-protocols/-/raw/main/`。

## 校验结果

校验依据：`wayland_codegen/src/protocol.rs` 中 `serde` 结构体可识别的元素/属性，
以及任务给出的 schema 清单。工具：`python3` + `xml.etree.ElementTree`。

### 1. well-formed
全部 **33/33** 通过 `ET.fromstring()`。所有文件均**无 DOCTYPE**（不会触发外部 DTD 读取）。

### 2. protocol name / interface / version
- 所有 `<protocol name>` 与 `<interface name>` 均为合法 Rust 标识符（`snake_case`，
  无连字符、无点号），可直接作为模块名 / `format_ident!` 使用。
- 所有 `<interface version>`、`<request/event/entry since>` 均为整数（`u32` 可解析）。
- 每个文件的 protocol name 与 interface 列表见上表。

### 3. 元素 / 属性是否超出可识别集合
`protocol.rs` 识别的元素仅 9 种：`protocol`、`copyright`、`description`、
`interface`、`request`、`event`、`arg`、`enum`、`entry`。

- 33 个文件均**只使用这 9 种元素名**，无未知元素名；层级关系均合法
  （`protocol>copyright/description/interface`、`interface>description/request/event/enum`、
  `request/event>description/arg`、`enum>description/entry`）。
- 新增 22 个文件中**无**未知属性、无非法值、无 `<arg type="new_id">` 缺 `interface`。
- 因此 **0 个文件会导致 quick-xml 解析报错**。

### 4. 超出「任务列出 schema」但被解析器容忍的构造（不报错，文件仍安全）

| 文件 | 构造 | 说明 |
|---|---|---|
| `content-type-v1.xml` | `<entry><description>` 嵌套 | `<entry>` 内带说明文本，超出严格 schema；但原有 `xdg-shell.xml`（build.rs 明确引用、已编译）就有 20 处同类结构，证明解析器容忍 |
| `pointer-constraints-unstable-v1.xml` | `<entry><description>` 嵌套 | 同上 |
| `presentation-time.xml` | `<entry><description>` 嵌套；`<event type="destructor">` | `event@type` 超出任务列出的 `<event name since>`；但 codegen 的 `Message` 结构为 request/event 共用，带 `@type` 字段，不会报错 |
| `tearing-control-v1.xml` | `<entry><description>` 嵌套 | 同上 |
| `drm.xml` | `<enum since="...">` | `since` 在任务列出的 `<enum name since bitfield>` 内；codegen 的 `Enum` 结构无 `@since` 字段，serde 默认静默忽略，无害 |

补充：原有 `wayland.xml` / `linux-dmabuf-v1.xml` 还包含 `<event deprecated-since>`
属性，同样被 serde 静默忽略，进一步印证「未知属性不报错」。

### 5. `<arg type="new_id">` 缺 `interface`
- 新增 22 个文件：**0 处**。
- 唯一一处出现在**原有** `wayland.xml`（`wl_registry.bind` 的 `id` 参数）。
  `generator.rs::map_type` 已将其映射为 `(String, u32, u32)` 元组，不会报错。

### 6. interface 名重复（会导致同名模块）
- 新增 22 个文件 **vs** 原有 11 个文件：**无重复**。
- 新增文件彼此之间：**无重复**。
- 易混淆但**不冲突**的例子：`xdg-dialog-v1.xml` 定义 `xdg_wm_dialog_v1`，
  与 `xdg-shell.xml` 的 `xdg_wm_base` 不同；`xdg-output-unstable-v1.xml` 的
  `zxdg_output_manager_v1` / `zxdg_output_v1` 与 `xdg-decoration-unstable-v1.xml`
  的 `zxdg_decoration_manager_v1` / `zxdg_toplevel_decoration_v1` 不同。

### 7. 安全判定
22 个新增文件**全部安全**：well-formed、元素/属性在可解析范围内、模块名合法、
无 interface 重名、无 `new_id` 缺 interface。

## 下载失败清单

**无失败**。10 个上游文件全部经代理下载成功（HTTP 200，内容为真实 XML，
已校验 `<protocol>` 根元素），未创建任何占位/伪造 XML。

- `fractional-scale-v1.xml`：按要求跳过（已存在）。
- `xdg-dialog-v1.xml`、`tearing-control-v1.xml`、`linux-drm-syncobj-v1.xml`：
  上游 staging 路径下均存在，已成功下载，无需跳过。

## 许可 / 出处汇总

- **上游 10 个**：来自
  [wayland-protocols](https://gitlab.freedesktop.org/wayland/wayland-protocols)
  `main` 分支，各文件 `<copyright>` 为 MIT/X11-like
  （"Permission is hereby granted, free of charge..."）。
- **本地复制 12 个**：来自本仓库 `sommelier/protocol/`（ChromeOS 主线 sommelier）。
  `text-input-crostini-unstable-v1.xml` / `text-input-x11-unstable-v1.xml` 为
  ChromiumOS Authors BSD-style；`drm.xml` 为 MIT/HPND-like；
  `gtk-shell.xml` 文件内无 `<copyright>` 块；其余为 MIT/X11-like。
- **原有 11 个**：见各文件 `<copyright>`，多为 MIT/X11-like / MIT/HPND-like。

各 XML 内部均保留原始 `<copyright>` 文本，许可详情以文件原文为准。
