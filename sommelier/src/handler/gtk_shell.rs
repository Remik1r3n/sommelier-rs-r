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

//! `gtk_shell1` bridge — the Rust counterpart of mainline
//! `sommelier-gtk-shell.cc`.
//!
//! Mainline never exposes `zaura_shell` to guests. Instead it advertises
//! `gtk_shell1` v1 (as soon as the host aura shell is >= v6) and translates
//! the handful of requests GTK clients actually use:
//!
//! | guest request | host request |
//! |---|---|
//! | `gtk_shell1.get_gtk_surface(id, surface)` | `zaura_shell.get_aura_surface(new_id, surface)` |
//! | `gtk_shell1.set_startup_id(id)` | `zaura_surface.set_startup_id(id)` on every surface |
//! | `gtk_surface1.set_dbus_properties(app_id, ...)` | `zaura_surface.set_application_id("org.chromium.guest_os.<vm>.wayland.<app_id>")` |
//! | `system_bell` / `set_modal` / `unset_modal` / `present` | DoNothing, like mainline |
//!
//! Everything stays local to the guest connection: the host only ever sees
//! `zaura_shell` / `zaura_surface` objects.

use crate::protocols::aura_shell::zaura_shell::REQ_GET_AURA_SURFACE;
use crate::protocols::aura_shell::zaura_surface::{REQ_SET_APPLICATION_ID, REQ_SET_STARTUP_ID};
use crate::protocols::gtk::gtk_shell1::GtkShell1Handler;
use crate::protocols::gtk::gtk_surface1::GtkSurface1Handler;
use crate::state::Context;
use crate::wire::{Action, MessageBuilder};

/// Cached startup id and the host `zaura_surface` objects created for this
/// guest's `gtk_surface1` objects.
#[derive(Debug, Default)]
pub struct GtkShellHandler {
    startup_id: Option<String>,
    host_surfaces: Vec<u32>,
}

impl GtkShellHandler {
    pub fn new() -> Self {
        Self::default()
    }

    fn send_startup_id(&self, ctx: &mut Context, host_surface: u32, id: &str) {
        let mut b = MessageBuilder::new();
        b.write_string(id);
        ctx.client_to_host_queue.push((
            b.build_message(host_surface, REQ_SET_STARTUP_ID),
            Vec::new(),
        ));
    }
}

impl GtkShell1Handler for GtkShellHandler {
    fn on_get_gtk_surface(&mut self, ctx: &mut Context, gtk_surface: u32, surface: u32) -> Action {
        let Some(zaura_shell) = ctx.host_zaura_shell_id else {
            log::debug!("gtk_shell1.get_gtk_surface without host zaura_shell");
            return Action::Drop;
        };
        let Some(host_surface) = ctx.shadow_table.get_host_id(surface) else {
            log::debug!(
                "gtk_shell1.get_gtk_surface on unmapped wl_surface {}",
                surface
            );
            return Action::Drop;
        };

        let host_id = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table.map_id(gtk_surface, host_id);
        ctx.shadow_table
            .track_interface(gtk_surface, "gtk_surface1".to_string());
        ctx.shadow_table
            .track_host_interface(host_id, "zaura_surface".to_string());

        let mut b = MessageBuilder::new();
        b.write_u32(host_id);
        b.write_u32(host_surface);
        ctx.client_to_host_queue.push((
            b.build_message(zaura_shell, REQ_GET_AURA_SURFACE),
            Vec::new(),
        ));

        // Mainline applies an already-known startup id to newly created
        // surfaces (sl_gtk_shell_get_gtk_surface, :83).
        if let Some(id) = self.startup_id.clone() {
            self.send_startup_id(ctx, host_id, &id);
        }
        self.host_surfaces.push(host_id);
        Action::Drop
    }

    fn on_set_startup_id(&mut self, ctx: &mut Context, startup_id: &str) -> Action {
        self.startup_id = if startup_id.is_empty() {
            None
        } else {
            Some(startup_id.to_string())
        };
        if let Some(id) = self.startup_id.clone() {
            for host_surface in self.host_surfaces.clone() {
                self.send_startup_id(ctx, host_surface, &id);
            }
        }
        Action::Drop
    }

    fn on_system_bell(&mut self, _ctx: &mut Context, _surface: u32) -> Action {
        // Mainline's DoNothing.
        Action::Drop
    }
}

#[derive(Debug, Default)]
pub struct GtkSurfaceHandler;

impl GtkSurface1Handler for GtkSurfaceHandler {
    fn on_set_dbus_properties(
        &mut self,
        ctx: &mut Context,
        application_id: &str,
        _app_menu_path: &str,
        _menubar_path: &str,
        _window_object_path: &str,
        _application_object_path: &str,
        _unique_bus_name: &str,
    ) -> Action {
        // Native Wayland clients use the `.wayland.` form
        // (NATIVE_WAYLAND_APPLICATION_ID_FORMAT, sommelier.h:26).
        if application_id.is_empty() || ctx.host_zaura_shell_version < 5 {
            return Action::Drop;
        }
        let Some(host_id) = ctx.shadow_table.get_host_id(ctx.last_sender_id) else {
            return Action::Drop;
        };
        let formatted = format!(
            "org.chromium.guest_os.{}.wayland.{}",
            ctx.vm_identifier, application_id
        );
        let mut b = MessageBuilder::new();
        b.write_string(&formatted);
        ctx.client_to_host_queue
            .push((b.build_message(host_id, REQ_SET_APPLICATION_ID), Vec::new()));
        Action::Drop
    }

    fn on_set_modal(&mut self, _ctx: &mut Context) -> Action {
        Action::Drop
    }

    fn on_unset_modal(&mut self, _ctx: &mut Context) -> Action {
        Action::Drop
    }

    fn on_present(&mut self, _ctx: &mut Context, _time: u32) -> Action {
        Action::Drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.host_zaura_shell_id = Some(11);
        ctx.host_zaura_shell_version = 38;
        ctx.vm_identifier = "termina".to_string();
        ctx.shadow_table.map_id(20, 21); // guest wl_surface 20 -> host 21
        ctx
    }

    #[test]
    fn get_gtk_surface_creates_a_host_aura_surface() {
        let mut ctx = ctx();
        let mut handler = GtkShellHandler::new();
        assert_eq!(handler.on_get_gtk_surface(&mut ctx, 30, 20), Action::Drop);
        assert_eq!(ctx.client_to_host_queue.len(), 1);
        let (msg, _) = &ctx.client_to_host_queue[0];
        assert_eq!(u32::from_ne_bytes(msg[0..4].try_into().unwrap()), 11);
        assert_eq!(
            (u32::from_ne_bytes(msg[4..8].try_into().unwrap()) & 0xffff) as u16,
            REQ_GET_AURA_SURFACE
        );
        assert_eq!(u32::from_ne_bytes(msg[12..16].try_into().unwrap()), 21);
        assert_eq!(
            ctx.shadow_table.get_host_id(30),
            Some(handler.host_surfaces[0])
        );
    }

    #[test]
    fn startup_id_is_replayed_on_new_surfaces() {
        let mut ctx = ctx();
        let mut handler = GtkShellHandler::new();
        handler.on_set_startup_id(&mut ctx, "abc123");
        assert!(ctx.client_to_host_queue.is_empty());
        handler.on_get_gtk_surface(&mut ctx, 30, 20);
        // get_aura_surface + set_startup_id
        assert_eq!(ctx.client_to_host_queue.len(), 2);
    }

    #[test]
    fn dbus_app_id_uses_mainline_format() {
        let mut ctx = ctx();
        let mut shell = GtkShellHandler::new();
        shell.on_get_gtk_surface(&mut ctx, 30, 20);
        ctx.client_to_host_queue.clear();
        ctx.last_sender_id = 30;
        let mut surface = GtkSurfaceHandler;
        assert_eq!(
            surface.on_set_dbus_properties(&mut ctx, "org.gnome.gedit", "", "", "", "", ""),
            Action::Drop
        );
        assert_eq!(ctx.client_to_host_queue.len(), 1);
        let msg = &ctx.client_to_host_queue[0].0;
        let text = String::from_utf8_lossy(&msg[12..]);
        assert!(text.contains("org.chromium.guest_os.termina.wayland.org.gnome.gedit"));
    }
}
