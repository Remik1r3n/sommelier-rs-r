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

use crate::protocols::wayland::wl_registry;
use crate::protocols::wayland::wl_shm;
use crate::state::{Context, HostId};
use crate::wire::{Action, MessageBuilder};
use log::error;

pub struct RegistryHandler;

/// Host globals that the proxy binds/translates internally and must never be
/// advertised to guest clients. Everything else in
/// `protocols::all::KNOWN_INTERFACES` is safe to forward; keeping a deny list
/// (instead of an allow list) means adding a protocol XML automatically makes
/// it available without touching this file.
/// Highest `wl_compositor` version we can expose without supporting
/// `wl_surface.damage_buffer` (introduced in v4).
const MAX_COMPOSITOR_VERSION: u32 = 3;

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
    // Legacy / explicit-sync globals handled by the proxy itself.
    "wl_drm",
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
    true
}

impl wl_registry::WlRegistryHandler for RegistryHandler {
    fn on_global(&mut self, ctx: &mut Context, name: u32, interface: &str, version: u32) -> Action {
        // Track host globals
        ctx.host_globals.insert(interface.to_string(), name);

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

        if interface == "wl_compositor" && !ctx.damage_buffer && version > MAX_COMPOSITOR_VERSION {
            // `wl_surface.damage_buffer` was added in wl_compositor v4 and we
            // cannot translate it for the host. Advertise (and later bind) v3
            // so a conforming guest never emits the request at all.
            let registry_guest_id = ctx
                .shadow_table
                .get_guest_id(ctx.last_sender_id)
                .unwrap_or(ctx.last_sender_id);
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(MAX_COMPOSITOR_VERSION);
            let msg = builder.build_message(registry_guest_id, wl_registry::EVT_GLOBAL as u16);
            ctx.host_to_client_queue.push((msg, Vec::new()));
            log::debug!(
                "Capping wl_compositor global at v{} (--no-support-damage-buffer)",
                MAX_COMPOSITOR_VERSION
            );
            return Action::Drop;
        }

        if interface == "zwp_linux_dmabuf_v1" {
            if !ctx.gpu_accel {
                return Action::Drop;
            }
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_dmabuf_id = Some(host_id);
            // Register for event dispatch: the host compositor sends format/modifier
            // events to the dmabuf factory object after we bind it.
            ctx.shadow_table
                .track_host_interface(host_id, "zwp_linux_dmabuf_v1".to_string());

            let client_version = version;
            let mut global_builder = MessageBuilder::new();
            global_builder.write_u32(name);
            global_builder.write_string(interface);
            global_builder.write_u32(client_version);

            // Translate host registry ID to guest registry ID
            let registry_guest_id = ctx
                .shadow_table
                .get_guest_id(ctx.last_sender_id)
                .unwrap_or(ctx.last_sender_id);

            let global_msg =
                global_builder.build_message(registry_guest_id, wl_registry::EVT_GLOBAL as u16);
            ctx.host_to_client_queue.push((global_msg, Vec::new()));

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

            let client_version = 1;
            let mut global_builder = MessageBuilder::new();
            global_builder.write_u32(name);
            global_builder.write_string("zwp_text_input_manager_v3");
            global_builder.write_u32(client_version);

            // Translate host registry ID to guest registry ID
            let registry_guest_id = ctx
                .shadow_table
                .get_guest_id(ctx.last_sender_id)
                .unwrap_or(ctx.last_sender_id);

            let global_msg =
                global_builder.build_message(registry_guest_id, wl_registry::EVT_GLOBAL as u16);
            ctx.host_to_client_queue.push((global_msg, Vec::new()));

            // 2. Bind internally
            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(version);
            builder.write_u32(host_id); // new_id

            let full_msg = builder.build_message(registry_host_id, wl_registry::REQ_BIND as u16);
            ctx.client_to_host_queue.push((full_msg, Vec::new()));

            return Action::Drop;
        } else if interface == "zcr_text_input_extension_v1" {
            let host_id = ctx.shadow_table.allocate_host_id();
            ctx.host_text_input_extension_v1_id = Some(host_id);
            // The host does not send events to the text_input_extension factory; no
            // shadow table entry needed.

            // 2. Bind internally
            let registry_host_id = ctx.last_sender_id;
            let mut builder = MessageBuilder::new();
            builder.write_u32(name);
            builder.write_string(interface);
            builder.write_u32(version);
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
        Action::Forward
    }

    fn on_bind(&mut self, ctx: &mut Context, _name: u32, id: &(String, u32, u32)) -> Action {
        // id is (interface, version, new_id)
        let (interface, _version, guest_new_id) = id;

        // Keep the host binding in sync with the version we advertised: a
        // guest that (incorrectly) requests v4+ must not make the host believe
        // damage_buffer is supported.
        let bound_version = if interface == "wl_compositor" && !ctx.damage_buffer {
            (*_version).min(MAX_COMPOSITOR_VERSION)
        } else {
            *_version
        };

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

        Action::Drop
    }
}
