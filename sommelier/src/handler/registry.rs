/*
Copyright 2026 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

use crate::protocols::aura_shell::zaura_shell::REQ_GET_AURA_OUTPUT;
use crate::protocols::wayland::wl_registry;
use crate::protocols::wayland::wl_shm;
use crate::protocols::xdg_output_unstable_v1::zxdg_output_manager_v1::REQ_GET_XDG_OUTPUT;
use crate::state::{Context, HostId};
use crate::transform::ScalingMode;
use crate::wire::{Action, MessageBuilder};
use log::error;

pub struct RegistryHandler;

/// Host globals that the proxy binds/translates internally and must never be
/// advertised to guest clients. Everything else in
/// `protocols::all::KNOWN_INTERFACES` is safe to forward; keeping a deny list
/// (instead of an allow list) means adding a protocol XML automatically makes
/// it available without touching this file.
/// Mainline `sommelier-compositor.cc:1170`: advertise v4 (we implement
/// `wl_surface.damage_buffer` by rewriting it to `wl_surface.damage`), or v3
/// when `--no-support-damage-buffer` is in effect.
const COMPOSITOR_VERSION: u32 = 4;
const COMPOSITOR_VERSION_NO_DAMAGE_BUFFER: u32 = 3;
/// Mainline always binds the host compositor at v3
/// (`kMinHostWlCompositorVersion`, sommelier.h:80).
const HOST_COMPOSITOR_VERSION: u32 = 3;
/// Version clamps from `sl_registry_handler` (sommelier.cc:546-671).
const MAX_OUTPUT_VERSION: u32 = 3;
const MAX_SEAT_VERSION: u32 = 5;
const MAX_DATA_DEVICE_MANAGER_VERSION: u32 = 3;
const MAX_XDG_WM_BASE_VERSION: u32 = 3;
const MAX_LINUX_DMABUF_VERSION: u32 = 4;
const MAX_ZAURA_SHELL_VERSION: u32 = 38;
const MIN_ZAURA_SHELL_VERSION: u32 = 6;
const MAX_TEXT_INPUT_EXTENSION_VERSION: u32 = 11;
const MAX_WL_DRM_VERSION: u32 = 2;
/// Synthetic registry names for globals that have no host counterpart.
/// Host global names are small integers, so these cannot collide.
pub const SYNTHETIC_TEXT_INPUT_V3_GLOBAL: u32 = 0x7fff_0000;
pub const SYNTHETIC_GTK_SHELL1_GLOBAL: u32 = 0x7fff_0001;
pub const SYNTHETIC_WL_DRM_GLOBAL: u32 = 0x7fff_0002;
pub const SYNTHETIC_TEXT_INPUT_CROSTINI_MANAGER_GLOBAL: u32 = 0x7fff_0003;
pub const SYNTHETIC_TEXT_INPUT_X11_GLOBAL: u32 = 0x7fff_0004;

/// Version we advertise to the guest for a host global, mirroring the
/// `MIN(...)` clamps in mainline's `sl_registry_handler`.
fn advertised_version(interface: &str, host_version: u32, ctx: &Context) -> u32 {
    match interface {
        "wl_compositor" => {
            if ctx.damage_buffer {
                COMPOSITOR_VERSION
            } else {
                COMPOSITOR_VERSION_NO_DAMAGE_BUFFER
            }
        }
        "wl_output" => host_version.min(MAX_OUTPUT_VERSION),
        "wl_seat" => host_version.min(MAX_SEAT_VERSION),
        "wl_data_device_manager" => host_version.min(MAX_DATA_DEVICE_MANAGER_VERSION),
        "xdg_wm_base" => host_version.min(MAX_XDG_WM_BASE_VERSION),
        "zwp_linux_dmabuf_v1" => host_version.min(MAX_LINUX_DMABUF_VERSION),
        "zcr_text_input_extension_v1" => host_version.min(MAX_TEXT_INPUT_EXTENSION_VERSION),
        "zwp_text_input_manager_v1" => 1,
        "wl_drm" => host_version.min(MAX_WL_DRM_VERSION),
        _ => host_version,
    }
}

/// Version used when binding the corresponding object on the host.
fn host_bind_version(interface: &str, requested: u32) -> u32 {
    match interface {
        // sommelier-compositor.cc:1187 — always v3, damage_buffer is handled
        // locally and never forwarded to the host.
        "wl_compositor" => HOST_COMPOSITOR_VERSION,
        "wl_output" => requested.min(MAX_OUTPUT_VERSION),
        "wl_seat" => requested.min(MAX_SEAT_VERSION),
        "wl_data_device_manager" => requested.min(MAX_DATA_DEVICE_MANAGER_VERSION),
        "xdg_wm_base" => requested.min(MAX_XDG_WM_BASE_VERSION),
        "zwp_linux_dmabuf_v1" => requested.min(MAX_LINUX_DMABUF_VERSION),
        "zcr_text_input_extension_v1" => requested.min(MAX_TEXT_INPUT_EXTENSION_VERSION),
        "zwp_text_input_manager_v1" => 1,
        "zaura_shell" => requested.min(MAX_ZAURA_SHELL_VERSION),
        "wl_drm" => requested.min(MAX_WL_DRM_VERSION),
        _ => requested,
    }
}

const INTERNAL_GLOBALS: &[&str] = &[
    // ChromeOS aura-shell: bound internally for shelf app-ids.
    "zaura_shell",
    "zaura_surface",
    "zaura_output",
    "zaura_toplevel",
    "zaura_popup",
    // Keyboard shortcut ack/peek protocol (internal).
    "zcr_keyboard_extension_v1",
    "zcr_extended_keyboard_v1",
    // cros-im: host speaks text-input v1 + zcr extension, guest sees v3.
    "zwp_text_input_manager_v1",
    "zwp_text_input_v1",
    "zcr_text_input_extension_v1",
    "zcr_extended_text_input_v1",
    "zcr_text_input_crostini_manager_v1",
    "zcr_text_input_crostini_v1",
    "zcr_text_input_x11_v1",
    // Host stylus-v2 is translated into guest tablet-v2.
    "zcr_stylus_v2",
    "zcr_touch_stylus_v2",
    "zcr_pointer_stylus_v2",
    // Gamepad is injected into the guest via uinput, not via Wayland.
    "zcr_gaming_input_v2",
    "zcr_gaming_seat_v2",
    "zcr_gamepad_v2",
    "zcr_gamepad_vibrator_v2",
    // Mainline binds zxdg_output_manager_v1 internally (only with
    // --direct-scale) and never advertises it to guests: the translated
    // wl_output geometry already describes the virtual layout.
    "zxdg_output_manager_v1",
    // wl_drm is emulated locally from an internal host dmabuf v2 proxy
    // (compositor/sommelier-drm.cc); never forward a host-provided wl_drm.
    "wl_drm",
    // Legacy / explicit-sync globals handled by the proxy itself.
    "gtk_shell1",
    "gtk_surface1",
    "zwp_linux_explicit_synchronization_v1",
    "zwp_linux_surface_synchronization_v1",
    "zwp_linux_buffer_release_v1",
    "wp_linux_drm_syncobj_manager_v1",
    "wp_linux_drm_syncobj_timeline_v1",
    "wp_linux_drm_syncobj_surface_v1",
];

fn is_advertisable(interface: &str, ctx: &Context) -> bool {
    if INTERNAL_GLOBALS.contains(&interface) {
        return false;
    }
    if !crate::protocols::all::is_known(interface) {
        return false;
    }
    if interface == "zxdg_decoration_manager_v1" && !ctx.xdg_decoration {
        return false;
    }
    if interface == "zwp_linux_dmabuf_v1" && !ctx.gpu_accel {
        return false;
    }
    // Mainline sommelier.cc:589-628: in X11 mode the data device manager and
    // xdg_wm_base are bound internally for the WM and are NOT advertised to
    // the Xwayland client (which is the only client on that connection).
    if ctx.is_xwayland_client && matches!(interface, "wl_data_device_manager" | "xdg_wm_base") {
        return false;
    }
    true
}

/// Re-emit a `wl_registry.global` event with a rewritten version. The generic
/// forward path cannot change payload fields, so mainline-style version
/// clamps need their own event.
/// Bind the host-side output extension objects mainline creates in
/// `sl_bind_host_output` (sommelier-output.cc:653-668). `zaura_output` gives
/// us the real (possibly fractional) device scale factor; `zxdg_output_v1`
/// is only needed for `--direct-scale` coordinate conversion.
fn create_output_extensions(ctx: &mut Context, host_output_id: u32) {
    if ctx.host_zaura_shell_version >= 6 {
        if let Some(aura_shell) = ctx.host_zaura_shell_id {
            let id = ctx.shadow_table.allocate_host_id();
            ctx.shadow_table
                .track_host_interface(id, "zaura_output".to_string());
            ctx.output_extension_to_host.insert(id, host_output_id);
            let mut b = MessageBuilder::new();
            b.write_u32(id);
            b.write_u32(host_output_id);
            ctx.client_to_host_queue
                .push((b.build_message(aura_shell, REQ_GET_AURA_OUTPUT), Vec::new()));
        }
    }
    if let Some(manager) = ctx.host_xdg_output_manager_id {
        let id = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table
            .track_host_interface(id, "zxdg_output_v1".to_string());
        ctx.output_extension_to_host.insert(id, host_output_id);
        let mut b = MessageBuilder::new();
        b.write_u32(id);
        b.write_u32(host_output_id);
        ctx.client_to_host_queue
            .push((b.build_message(manager, REQ_GET_XDG_OUTPUT), Vec::new()));
    }
}

fn emit_global(ctx: &mut Context, name: u32, interface: &str, version: u32) {
    let registry_guest_id = ctx
        .shadow_table
        .get_guest_id(ctx.last_sender_id)
        .unwrap_or(ctx.last_sender_id);
    let mut builder = MessageBuilder::new();
    builder.write_u32(name);
    builder.write_string(interface);
    builder.write_u32(version);
    let msg = builder.build_message(registry_guest_id, wl_registry::EVT_GLOBAL as u16);
    ctx.host_to_client_queue.push((msg, Vec::new()));
}

impl wl_registry::WlRegistryHandler for RegistryHandler {
    fn on_global(&mut self, ctx: &mut Context, name: u32, interface: &str, version: u32) -> Action {
        // Track host globals
        ctx.host_globals.insert(interface.to_string(), name);

        // sommelier.cc:644-657 — wp_viewporter means fractional scaling is
        // available, so drop the integer start-up scale in favour of the
        // exact --scale value.
        if interface == "wp_viewporter" {
            let desired = ctx.desired_scale;
            ctx.transform.set_scale(desired);
        }

        // Xwayland never binds xdg_wm_base itself (rootless Xwayland only
        // creates wl_surfaces; the WM is expected to create the xdg roles).
        // Bind it internally so the X11 WM can synthesize xdg_toplevel on the
        // same host connection as those surfaces.
        if interface == "xdg_wm_base" && ctx.is_xwayland_client {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.shadow_table
                .track_host_interface(host_id, "xdg_wm_base".to_string());
            ctx.host_xdg_wm_base_id = Some(host_id);
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(version.min(3));
            builder.write_u32(host_id);
            let msg = builder.build_message(ctx.last_sender_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((msg, Vec::new()));
            log::debug!(
                "Bound xdg_wm_base internally for Xwayland (host_id={})",
                host_id
            );
        }

        // X11 CLIPBOARD -> host selection export. Mainline binds
        // wl_data_device_manager internally and uses its own wl_data_device
        // for set_selection; we do the same on the dedicated Xwayland
        // connection. is_advertisable() below keeps this global off the
        // Xwayland client's registry (sommelier.cc:589-628), so no guest
        // bind is expected on this connection.
        if ctx.is_xwayland_client
            && interface == "wl_data_device_manager"
            && ctx.host_data_device_manager_id.is_none()
        {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.shadow_table
                .track_host_interface(host_id, interface.to_string());
            ctx.internal_selection_ids.insert(host_id);
            ctx.host_data_device_manager_id = Some(host_id);
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(version.min(3));
            builder.write_u32(host_id);
            ctx.client_to_host_queue.push((
                builder.build_message(ctx.last_sender_id, wl_registry::REQ_BIND as u16),
                Vec::new(),
            ));
            crate::x11::selection::ensure_host_selection_device(ctx);
            log::debug!(
                "x11: bound wl_data_device_manager internally (host_id={})",
                host_id
            );
        }

        if interface == "wl_compositor" {
            // Mainline advertises a fixed version (4 normally, 3 with
            // --no-support-damage-buffer) regardless of the host version and
            // binds the host at v3. `wl_surface.damage_buffer` is translated
            // to `wl_surface.damage` by handler/compositor.rs.
            let want = advertised_version(interface, version, ctx);
            emit_global(ctx, name, interface, want);
            log::debug!(
                "Advertising wl_compositor v{} (host v{}, damage_buffer={})",
                want,
                version,
                ctx.damage_buffer
            );
            return Action::Drop;
        }

        if interface == "zwp_linux_dmabuf_v1" {
            // Mainline sommelier.cc:658-678 creates the local wl_drm global
            // unconditionally (only the guest-visible zwp_linux_dmabuf_v1 is
            // gated by --enable-linux-dmabuf). It needs create_immed, which
            // exists since dmabuf v2; bind a separate proxy pinned to v2 so
            // v4+ format/modifier validation cannot reject implicit buffers.
            if version >= 2 && ctx.drm_device.is_some() && ctx.host_drm_dmabuf_id.is_none() {
                let host_id = ctx.shadow_table.allocate_host_id();
                ctx.shadow_table
                    .track_host_interface(host_id, "zwp_linux_dmabuf_v1".to_string());
                ctx.host_drm_dmabuf_id = Some(host_id);
                let mut b = MessageBuilder::new();
                b.write_u32(name);
                b.write_string(interface);
                b.write_u32(2);
                b.write_u32(host_id);
                ctx.client_to_host_queue.push((
                    b.build_message(ctx.last_sender_id, wl_registry::REQ_BIND as u16),
                    Vec::new(),
                ));
                emit_global(ctx, SYNTHETIC_WL_DRM_GLOBAL, "wl_drm", 2);
                log::debug!(
                    "wl_drm v2 emulation enabled (internal dmabuf v2 host_id={})",
                    host_id
                );
            }

            if !ctx.gpu_accel {
                return Action::Drop;
            }
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_dmabuf_id = Some(host_id);
            // Register for event dispatch: the host compositor sends format/modifier
            // events to the dmabuf factory object after we bind it.
            ctx.shadow_table
                .track_host_interface(host_id, "zwp_linux_dmabuf_v1".to_string());

            let client_version = advertised_version(interface, version, ctx);
            emit_global(ctx, name, interface, client_version);

            // 2. Bind internally
            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(client_version);
            builder.write_u32(host_id); // new_id

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));

            // Drop the original global event so we don't send the v4 advertisement
            return Action::Drop;
        } else if interface == "zwp_text_input_manager_v1" {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_text_input_manager_v1_id = Some(host_id);
            // The host does not send events to the text_input_manager factory; no
            // shadow table entry needed.

            // Mainline advertises zwp_text_input_manager_v1 v1. We additionally
            // advertise v3 (translated to the host's v1 manager) so modern
            // toolkits keep working; both are part of the compatibility goal.
            emit_global(ctx, name, "zwp_text_input_manager_v1", 1);
            emit_global(
                ctx,
                SYNTHETIC_TEXT_INPUT_V3_GLOBAL,
                "zwp_text_input_manager_v3",
                1,
            );
            // Mainline sommelier.cc:711-714 creates all three globals
            // together. The crostini/x11 ones are local to sommelier and
            // have no host counterpart (sommelier-text-input.cc:641-653).
            emit_global(
                ctx,
                SYNTHETIC_TEXT_INPUT_CROSTINI_MANAGER_GLOBAL,
                "zcr_text_input_crostini_manager_v1",
                1,
            );
            emit_global(
                ctx,
                SYNTHETIC_TEXT_INPUT_X11_GLOBAL,
                "zcr_text_input_x11_v1",
                1,
            );

            // 2. Bind internally at mainline's kTextInputManagerVersion = 1.
            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(1);
            builder.write_u32(host_id); // new_id

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));

            return Action::Drop;
        } else if interface == "zcr_text_input_extension_v1" {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_text_input_extension_v1_id = Some(host_id);
            // The host does not send events to the text_input_extension factory; no
            // shadow table entry needed.
            let client_version = advertised_version(interface, version, ctx);

            // Mainline advertises this global at min(host, 11) and binds it
            // both internally and on behalf of the guest.
            emit_global(ctx, name, interface, client_version);

            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(client_version);
            builder.write_u32(host_id); // new_id

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));

            return Action::Drop;
        } else if interface == "zcr_keyboard_extension_v1" {
            // Bind zcr_keyboard_extension_v1 internally. This is a ChromeOS-
            // specific protocol that enables the ack-key mechanism for
            // controlling host accelerator processing. Not exposed to the guest.
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_keyboard_extension_id = Some(HostId::from_allocated(host_id));
            // The host does not send events to the keyboard_extension factory; no
            // shadow table entry needed.

            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            // Bind at v1: we only need ack_key. peek_key (added in v2) is
            // intentionally not used.
            builder.write_u32(1);
            builder.write_u32(host_id);

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));
            log::debug!("Bound zcr_keyboard_extension_v1 (host_id={})", host_id);

            return Action::Drop;
        } else if interface == "zaura_shell" {
            // Bind zaura_shell internally for ChromeOS shelf integration.
            // We use this to create zaura_surface objects and set application
            // IDs so the shelf can match windows to .desktop entries.
            // Not exposed to the guest; capped at v38 (need v5 for
            // set_application_id, v38 for release destructor).
            let host_id = ctx.shadow_table.allocate_host_id();
            let bound_version = std::cmp::min(version, 38);
            ctx.host_zaura_shell_id = Some(host_id);
            ctx.host_zaura_shell_version = bound_version;
            ctx.shadow_table
                .track_host_interface(host_id, "zaura_shell".to_string());

            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(bound_version);
            builder.write_u32(host_id);

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));
            log::debug!("Bound zaura_shell internally (host_id={})", host_id);

            // Mainline sommelier.cc:629-643 exposes `gtk_shell1` v1 (not
            // zaura_shell) to guests whenever the host aura shell is >= v6.
            // handler/gtk_shell.rs translates its requests onto zaura_shell.
            if version >= MIN_ZAURA_SHELL_VERSION {
                emit_global(ctx, SYNTHETIC_GTK_SHELL1_GLOBAL, "gtk_shell1", 1);
            }

            return Action::Drop;
        } else if interface == "zxdg_output_manager_v1" {
            // sommelier.cc:759-775: only --direct-scale needs xdg-output, and
            // mainline warns that binding it unconditionally breaks Crostini.
            if ctx.transform.mode == ScalingMode::Direct && ctx.host_xdg_output_manager_id.is_none()
            {
                let host_id = ctx.shadow_table.allocate_host_id();
                ctx.shadow_table
                    .track_host_interface(host_id, "zxdg_output_manager_v1".to_string());
                ctx.host_xdg_output_manager_id = Some(host_id);
                let mut b = MessageBuilder::new();
                b.write_u32(name);
                b.write_string(interface);
                b.write_u32(version.min(3));
                b.write_u32(host_id);
                ctx.client_to_host_queue.push((
                    b.build_message(ctx.last_sender_id, wl_registry::REQ_BIND as u16),
                    Vec::new(),
                ));
                log::debug!("Bound zxdg_output_manager_v1 internally for --direct-scale");
            }
            return Action::Drop;
        } else if interface == "wl_shm" {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_shm_id = Some(host_id);
            // wl_shm is emulated: on_bind drops the guest request and sends synthetic
            // format events, so the host never sends wl_shm events to us. No shadow
            // table entry is needed.

            // Bind to wl_shm
            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(1); // Bind version 1
            builder.write_u32(host_id); // new_id

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));
        }

        if !is_advertisable(interface, ctx) {
            log::debug!("Filtering host global {} v{}", interface, version);
            return Action::Drop;
        }
        // Apply mainline's version clamps to everything else. Rewriting the
        // event (rather than forwarding it) is what makes wl_output v3 /
        // wl_seat v5 / xdg_wm_base v3 deterministic regardless of host.
        let want = advertised_version(interface, version, ctx);
        if want != version {
            emit_global(ctx, name, interface, want);
            return Action::Drop;
        }
        Action::Forward
    }

    fn on_bind(&mut self, ctx: &mut Context, _name: u32, id: &(String, u32, u32)) -> Action {
        // id is (interface, version, new_id)
        let (interface, _version, guest_new_id) = id;

        // Mainline clamps both the guest-advertised and host-bound versions
        // (`sl_registry_handler`, `sl_bind_host_*`). In particular the host
        // wl_compositor is always bound at v3 while guests see v4.
        let bound_version = host_bind_version(interface, *_version);

        // `gtk_shell1` has no host counterpart: it is implemented locally on
        // top of the internally-bound zaura_shell (sommelier-gtk-shell.cc).
        if interface == "gtk_shell1" {
            if let Some(host_id) = ctx.host_zaura_shell_id {
                ctx.shadow_table.map_id(*guest_new_id, host_id);
                ctx.shadow_table
                    .track_interface(*guest_new_id, "gtk_shell1".to_string());
            } else {
                error!("gtk_shell1 bound but host zaura_shell is missing");
            }
            return Action::Drop;
        }

        if matches!(
            interface.as_str(),
            "zcr_text_input_crostini_manager_v1" | "zcr_text_input_x11_v1"
        ) {
            // Purely local: mainline binds these with a null host proxy.
            ctx.shadow_table
                .track_interface(*guest_new_id, interface.clone());
            return Action::Drop;
        }

        if interface == "wl_drm" {
            // Purely local: mainline never binds a host wl_drm. The handler
            // answers authenticate and turns create_prime_buffer into host
            // dmabuf v2 requests, so no host object id is needed here.
            ctx.shadow_table
                .track_interface(*guest_new_id, "wl_drm".to_string());
            crate::handler::drm::DrmHandler::send_bind_events(ctx, *guest_new_id);
            return Action::Drop;
        }

        if interface == "wl_shm" {
            // Do NOT forward wl_shm to host. We emulate it.
            // Just track it so we know this guest ID is wl_shm.
            ctx.shadow_table
                .track_interface(*guest_new_id, "wl_shm".to_string());

            // Send standard formats to client: ARGB8888 (0) and XRGB8888 (1)
            for format in [0u32, 1u32] {
                let mut builder = MessageBuilder::new();
                builder.write_u32(format);
                let msg = builder.build_message(*guest_new_id, wl_shm::EVT_FORMAT as u16);
                ctx.host_to_client_queue.push((msg, Vec::new()));
            }

            return Action::Drop;
        } else if interface == "zwp_text_input_manager_v3" {
            // Map the client's v3 ID to the host's v1 ID we already bound.
            if let Some(host_id) = ctx.host_text_input_manager_v1_id {
                ctx.shadow_table.map_id(*guest_new_id, host_id);
                ctx.shadow_table
                    .track_interface(*guest_new_id, "zwp_text_input_manager_v3".to_string());
            } else {
                error!("zwp_text_input_manager_v3 bound but host v1 manager not found");
            }
            return Action::Drop;
        }

        // Translation logic for other interfaces
        let host_new_id = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table.map_id(*guest_new_id, host_new_id);
        ctx.shadow_table
            .track_interface(*guest_new_id, interface.clone());

        // Remember the host objects the X11 WM needs in order to synthesize
        // xdg_toplevel / zaura_surface for X11 windows.
        match interface.as_str() {
            "xdg_wm_base" => ctx.host_xdg_wm_base_id = Some(host_new_id),
            "wl_compositor" => ctx.host_compositor_id = Some(host_new_id),
            _ => {}
        }

        // We need to send the bind request to the host.
        // The sender is the registry object.
        let registry_guest_id = ctx.last_sender_id;

        if let Some(registry_host_id) = ctx.shadow_table.get_host_id(registry_guest_id) {
            let mut builder = MessageBuilder::new();
            builder.write_u32(_name);
            builder.write_string(interface);
            builder.write_u32(bound_version);
            builder.write_u32(host_new_id);

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));
        } else {
            error!("Registry not mapped! Guest ID: {}", registry_guest_id);
        }

        if interface == "wl_output" {
            create_output_extensions(ctx, host_new_id);
        }

        Action::Drop
    }
}
