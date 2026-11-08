# Wayland 协议 XML 定义（third_party/protocols/）

本目录存放加强版 Wayland proxy 使用的 Wayland 协议 XML。`wayland_codegen`
（基于 `quick-xml` + `serde`，见 `wayland_codegen/src/protocol.rs`）在构建期解析
这些文件并生成 Rust 代码；`<protocol name="...">` 即生成的 Rust 模块名。

## 构建接入

`sommelier/build.rs` 在构建期自动枚举本目录下全部 `.xml`（当前 33 个），
生成同名 Rust 模块与 `protocols::all` 聚合分发。新增协议只需放入 XML，
无需在 `build.rs` 或 `proxy.rs` 手工登记。

## 文件清单

表头说明：「来源」为 XML 的出处；「interfaces」为
`name@version`（version 取自 `<interface version>`）；「许可」由各文件
`<copyright>` 文本判定。

| 文件 | 来源 | protocol name（模块名） | interfaces (name@version) | 许可 |
|---|---|---|---|---|
| `aura-shell.xml` | 项目自带的协议定义 | `aura_shell` | zaura_shell@38, zaura_surface@38, zaura_output@38, zaura_toplevel@38, zaura_popup@38 | MIT/X11-like |
| `content-type-v1.xml` | wayland-protocols `staging/content-type/content-type-v1.xml` | `content_type_v1` | wp_content_type_manager_v1@1, wp_content_type_v1@1 | MIT/X11-like |
| `cursor-shape-v1.xml` | wayland-protocols `staging/cursor-shape/cursor-shape-v1.xml` | `cursor_shape_v1` | wp_cursor_shape_manager_v1@2, wp_cursor_shape_device_v1@2 | MIT/X11-like |
| `drm.xml` | ChromeOS 主线 `sommelier/protocol/drm.xml` | `drm` | wl_drm@2 | MIT/HPND-like |
| `fractional-scale-v1.xml` | 项目自带的协议定义 | `fractional_scale_v1` | wp_fractional_scale_manager_v1@1, wp_fractional_scale_v1@1 | MIT/X11-like |
| `gaming-input-unstable-v2.xml` | ChromeOS 主线 `sommelier/protocol/gaming-input-unstable-v2.xml` | `gaming_input_unstable_v2` | zcr_gaming_input_v2@2, zcr_gaming_seat_v2@1, zcr_gamepad_v2@2, zcr_gamepad_vibrator_v2@2 | MIT/X11-like |
| `gtk-shell.xml` | ChromeOS 主线 `sommelier/protocol/gtk-shell.xml` | `gtk` | gtk_shell1@1, gtk_surface1@1 | 无 `<copyright>` 块 |
| `idle-inhibit-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/idle-inhibit-unstable-v1.xml` | `idle_inhibit_unstable_v1` | zwp_idle_inhibit_manager_v1@1, zwp_idle_inhibitor_v1@1 | MIT/X11-like |
| `keyboard-extension-unstable-v1.xml` | 项目自带的协议定义 | `keyboard_extension_unstable_v1` | zcr_keyboard_extension_v1@2, zcr_extended_keyboard_v1@2 | MIT/X11-like |
| `keyboard-shortcuts-inhibit-unstable-v1.xml` | wayland-protocols `unstable/keyboard-shortcuts-inhibit/keyboard-shortcuts-inhibit-unstable-v1.xml` | `keyboard_shortcuts_inhibit_unstable_v1` | zwp_keyboard_shortcuts_inhibit_manager_v1@1, zwp_keyboard_shortcuts_inhibitor_v1@1 | MIT/X11-like |
| `linux-dmabuf-v1.xml` | 项目自带的协议定义 | `linux_dmabuf_v1` | zwp_linux_dmabuf_v1@5, zwp_linux_buffer_params_v1@5, zwp_linux_dmabuf_feedback_v1@5 | MIT/X11-like |
| `linux-drm-syncobj-v1.xml` | wayland-protocols `staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml` | `linux_drm_syncobj_v1` | wp_linux_drm_syncobj_manager_v1@1, wp_linux_drm_syncobj_timeline_v1@1, wp_linux_drm_syncobj_surface_v1@1 | MIT/X11-like |
| `linux-explicit-synchronization-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/linux-explicit-synchronization-unstable-v1.xml` | `zwp_linux_explicit_synchronization_unstable_v1` | zwp_linux_explicit_synchronization_v1@2, zwp_linux_surface_synchronization_v1@2, zwp_linux_buffer_release_v1@1 | MIT/X11-like |
| `pointer-constraints-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/pointer-constraints-unstable-v1.xml` | `pointer_constraints_unstable_v1` | zwp_pointer_constraints_v1@1, zwp_locked_pointer_v1@1, zwp_confined_pointer_v1@1 | MIT/X11-like |
| `presentation-time.xml` | wayland-protocols `stable/presentation-time/presentation-time.xml` | `presentation_time` | wp_presentation@3, wp_presentation_feedback@3 | MIT/X11-like |
| `primary-selection-unstable-v1.xml` | wayland-protocols `unstable/primary-selection/primary-selection-unstable-v1.xml` | `wp_primary_selection_unstable_v1` | zwp_primary_selection_device_manager_v1@1, zwp_primary_selection_device_v1@1, zwp_primary_selection_offer_v1@1, zwp_primary_selection_source_v1@1 | MIT/X11-like |
| `relative-pointer-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/relative-pointer-unstable-v1.xml` | `relative_pointer_unstable_v1` | zwp_relative_pointer_manager_v1@1, zwp_relative_pointer_v1@1 | MIT/X11-like |
| `single-pixel-buffer-v1.xml` | wayland-protocols `staging/single-pixel-buffer/single-pixel-buffer-v1.xml` | `single_pixel_buffer_v1` | wp_single_pixel_buffer_manager_v1@1 | MIT/X11-like |
| `stylus-unstable-v2.xml` | ChromeOS 主线 `sommelier/protocol/stylus-unstable-v2.xml` | `stylus_unstable_v2` | zcr_stylus_v2@2, zcr_touch_stylus_v2@1, zcr_pointer_stylus_v2@2 | MIT/X11-like |
| `tablet-unstable-v2.xml` | ChromeOS 主线 `sommelier/protocol/tablet-unstable-v2.xml` | `tablet_unstable_v2` | zwp_tablet_manager_v2@1, zwp_tablet_seat_v2@1, zwp_tablet_tool_v2@1, zwp_tablet_v2@1, zwp_tablet_pad_ring_v2@1, zwp_tablet_pad_strip_v2@1, zwp_tablet_pad_group_v2@1, zwp_tablet_pad_v2@1 | MIT/X11-like |
| `tearing-control-v1.xml` | wayland-protocols `staging/tearing-control/tearing-control-v1.xml` | `tearing_control_v1` | wp_tearing_control_manager_v1@1, wp_tearing_control_v1@1 | MIT/X11-like |
| `text-input-crostini-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/text-input-crostini-unstable-v1.xml` | `text_input_crostini_unstable_v1` | zcr_text_input_crostini_manager_v1@1, zcr_text_input_crostini_v1@1 | BSD-style (ChromiumOS) |
| `text-input-extension-unstable-v1.xml` | 项目自带的协议定义 | `text_input_extension_unstable_v1` | zcr_text_input_extension_v1@11, zcr_extended_text_input_v1@11 | MIT/X11-like |
| `text-input-unstable-v1.xml` | 项目自带的协议定义 | `text_input_unstable_v1` | zwp_text_input_v1@1, zwp_text_input_manager_v1@1 | MIT/X11-like |
| `text-input-unstable-v3.xml` | 项目自带的协议定义 | `text_input_unstable_v3` | zwp_text_input_v3@1, zwp_text_input_manager_v3@1 | MIT/HPND-like |
| `text-input-x11-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/text-input-x11-unstable-v1.xml` | `text_input_x11_unstable_v1` | zcr_text_input_x11_v1@1 | BSD-style (ChromiumOS) |
| `viewporter.xml` | 项目自带的协议定义 | `viewporter` | wp_viewporter@1, wp_viewport@1 | MIT/X11-like |
| `wayland.xml` | 项目自带的协议定义 | `wayland` | wl_display@1, wl_registry@1, wl_callback@1, wl_compositor@6, wl_shm_pool@2, wl_shm@2, wl_buffer@1, wl_data_offer@3, wl_data_source@3, wl_data_device@3, wl_data_device_manager@3, wl_shell@1, wl_shell_surface@1, wl_surface@6, wl_seat@10, wl_pointer@10, wl_keyboard@10, wl_touch@10, wl_output@4, wl_region@1, wl_subcompositor@1, wl_subsurface@1, wl_fixes@1 | MIT/X11-like |
| `xdg-activation-v1.xml` | wayland-protocols `staging/xdg-activation/xdg-activation-v1.xml` | `xdg_activation_v1` | xdg_activation_v1@1, xdg_activation_token_v1@1 | MIT/X11-like |
| `xdg-decoration-unstable-v1.xml` | 项目自带的协议定义 | `xdg_decoration_unstable_v1` | zxdg_decoration_manager_v1@1, zxdg_toplevel_decoration_v1@1 | MIT/X11-like |
| `xdg-dialog-v1.xml` | wayland-protocols `staging/xdg-dialog/xdg-dialog-v1.xml` | `xdg_dialog_v1` | xdg_wm_dialog_v1@1, xdg_dialog_v1@1 | MIT/X11-like |
| `xdg-output-unstable-v1.xml` | ChromeOS 主线 `sommelier/protocol/xdg-output-unstable-v1.xml` | `xdg_output_unstable_v1` | zxdg_output_manager_v1@3, zxdg_output_v1@3 | MIT/X11-like |
| `xdg-shell.xml` | 项目自带的协议定义 | `xdg_shell` | xdg_wm_base@7, xdg_positioner@7, xdg_surface@7, xdg_toplevel@7, xdg_popup@7 | MIT/X11-like |

上游文件完整 URL 前缀：
`https://gitlab.freedesktop.org/wayland/wayland-protocols/-/raw/main/`。

## 许可 / 出处汇总

- **wayland-protocols 10 个**：来自
  [wayland-protocols](https://gitlab.freedesktop.org/wayland/wayland-protocols)
  `main` 分支，各文件 `<copyright>` 为 MIT/X11-like
  （"Permission is hereby granted, free of charge..."）。
- **ChromeOS 主线 12 个**：来自 `sommelier/protocol/`。
  `text-input-crostini-unstable-v1.xml` / `text-input-x11-unstable-v1.xml` 为
  ChromiumOS Authors BSD-style；`drm.xml` 为 MIT/HPND-like；
  `gtk-shell.xml` 文件内无 `<copyright>` 块；其余为 MIT/X11-like。
- **项目自带 11 个**：见各文件 `<copyright>`，多为 MIT/X11-like / MIT/HPND-like。

各 XML 内部均保留原始 `<copyright>` 文本，许可详情以文件原文为准。
