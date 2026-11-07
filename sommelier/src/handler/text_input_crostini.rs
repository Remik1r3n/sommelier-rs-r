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

//! Local Crostini IME protocols — the Rust counterpart of
//! `sommelier-text-input.cc:548-697`.
//!
//! These two interfaces exist **only** between sommelier and `cros_im`; the
//! host compositor never sees them (mainline binds them with a null proxy).
//! Their entire purpose is one request: an X11 window id arrives from
//! `cros_im`, which has no way to name the Xwayland `wl_surface`, so sommelier
//! resolves the XID and sends `zwp_text_input_v1.activate(seat, surface)` on
//! its behalf. All the real IME state (preedit, commit, surrounding text) is
//! forwarded on the ordinary `zwp_text_input_v1` / `zcr_extended_text_input_v1`
//! path handled by `handler/text_input.rs`.
//!
//! `zcr_text_input_x11_v1` is deprecated upstream (M118 replaced it with
//! `zcr_text_input_crostini_v1`) but is still advertised for older cros_im.

use std::collections::HashMap;

use crate::protocols::text_input_crostini_unstable_v1::zcr_text_input_crostini_manager_v1::ZcrTextInputCrostiniManagerV1Handler;
use crate::protocols::text_input_crostini_unstable_v1::zcr_text_input_crostini_v1::ZcrTextInputCrostiniV1Handler;
use crate::protocols::text_input_unstable_v1::zwp_text_input_v1::REQ_ACTIVATE;
use crate::protocols::text_input_x11_unstable_v1::zcr_text_input_x11_v1::ZcrTextInputX11V1Handler;
use crate::protocols::wayland::wl_compositor::REQ_CREATE_SURFACE;
use crate::protocols::wayland::wl_display::EVT_ERROR;
use crate::state::Context;
use crate::wire::{Action, MessageBuilder};

const WL_DISPLAY_ID: u32 = 1;
/// `zcr_text_input_crostini_manager_v1.error.text_input_crostini_exists`
/// (text-input-crostini-unstable-v1.xml:14-17).
const ERROR_TEXT_INPUT_CROSTINI_EXISTS: u32 = 0;

#[derive(Debug, Default)]
pub struct TextInputCrostiniHandler {
    /// guest `zcr_text_input_crostini_v1` id → guest `zwp_text_input_v1` id.
    crostini_to_text_input: HashMap<u32, u32>,
    /// guest `zwp_text_input_v1` id → its crostini object (uniqueness check).
    text_input_to_crostini: HashMap<u32, u32>,
    /// XID → host `wl_surface` created on *this* connection for non-Xwayland
    /// clients (see `resolve_x11_window`).
    local_surface_for_xid: HashMap<u32, u32>,
}

impl TextInputCrostiniHandler {
    pub fn new() -> Self {
        Self::default()
    }

    fn post_error(ctx: &mut Context, object_id: u32, code: u32, message: &str) {
        let mut b = MessageBuilder::new();
        b.write_u32(object_id);
        b.write_u32(code);
        b.write_string(message);
        ctx.host_to_client_queue
            .push((b.build_message(WL_DISPLAY_ID, EVT_ERROR), Vec::new()));
    }

    /// Translate an X11 XID into a host `wl_surface` usable on the current
    /// host connection.
    ///
    /// Mainline shares one host `wl_display` across every client, so the
    /// Xwayland surface id is directly valid. This proxy gives each guest
    /// connection its own host connection; on the Xwayland connection the id
    /// is valid as-is, while other clients must create their own placeholder
    /// surface (wire-legal, but it cannot represent the real X11 window).
    fn resolve_x11_window(&mut self, ctx: &mut Context, xid: u32) -> Option<u32> {
        let bridge = ctx.x11_bridge.clone()?;
        let host_surface = bridge.host_surface_for_xid(xid)?;
        if ctx.is_xwayland_client {
            return Some(host_surface);
        }
        if let Some(id) = self.local_surface_for_xid.get(&xid) {
            return Some(*id);
        }
        let compositor = ctx.host_compositor_id?;
        let id = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table
            .track_host_interface(id, "wl_surface".to_string());
        let mut b = MessageBuilder::new();
        b.write_u32(id);
        ctx.client_to_host_queue
            .push((b.build_message(compositor, REQ_CREATE_SURFACE), Vec::new()));
        self.local_surface_for_xid.insert(xid, id);
        Some(id)
    }

    /// `sommelier-text-input.cc:558-591` / `:666-673`.
    fn activate_x11(
        &mut self,
        ctx: &mut Context,
        guest_text_input: u32,
        guest_seat: u32,
        xid: u32,
    ) -> Action {
        let host_text_input = ctx
            .shadow_table
            .get_host_id(guest_text_input)
            .or_else(|| ctx.text_inputs.get(&guest_text_input).map(|s| s.host_v1_id));
        let Some(host_text_input) = host_text_input else {
            log::debug!("crostini IME: text_input {guest_text_input} has no host proxy");
            return Action::Drop;
        };
        let host_seat = ctx.shadow_table.get_host_id(guest_seat).unwrap_or(0);
        let Some(host_surface) = self.resolve_x11_window(ctx, xid) else {
            log::debug!("crostini IME: no host surface for XID {xid:#x}");
            return Action::Drop;
        };

        if let Some(state) = ctx.text_inputs.get_mut(&guest_text_input) {
            state.active_surface = Some(guest_seat);
        }

        let mut b = MessageBuilder::new();
        b.write_u32(host_seat);
        b.write_u32(host_surface);
        ctx.client_to_host_queue
            .push((b.build_message(host_text_input, REQ_ACTIVATE), Vec::new()));
        log::debug!(
            "crostini IME: activate_x11 xid={:#x} -> host surface {} on text_input {}",
            xid,
            host_surface,
            host_text_input
        );
        Action::Drop
    }
}

impl ZcrTextInputCrostiniManagerV1Handler for TextInputCrostiniHandler {
    fn on_get_text_input_crostini(
        &mut self,
        ctx: &mut Context,
        id: u32,
        text_input: u32,
    ) -> Action {
        // sommelier-text-input.cc:617-623 — one crostini object per text_input.
        if self.text_input_to_crostini.contains_key(&text_input) {
            Self::post_error(
                ctx,
                ctx.last_sender_id,
                ERROR_TEXT_INPUT_CROSTINI_EXISTS,
                "text_input has already been associated with a text_input_crostini",
            );
            return Action::Drop;
        }
        ctx.shadow_table
            .track_interface(id, "zcr_text_input_crostini_v1".to_string());
        self.crostini_to_text_input.insert(id, text_input);
        self.text_input_to_crostini.insert(text_input, id);
        // No host proxy is created: this object is entirely local.
        Action::Drop
    }
}

impl ZcrTextInputCrostiniV1Handler for TextInputCrostiniHandler {
    fn on_destroy(&mut self, ctx: &mut Context) -> Action {
        let id = ctx.last_sender_id;
        if let Some(text_input) = self.crostini_to_text_input.remove(&id) {
            self.text_input_to_crostini.remove(&text_input);
        }
        ctx.shadow_table.remove_id(id);
        Action::Drop
    }

    fn on_activate_x11(&mut self, ctx: &mut Context, seat: u32, x11_window_id: u32) -> Action {
        let Some(text_input) = self
            .crostini_to_text_input
            .get(&ctx.last_sender_id)
            .copied()
        else {
            log::debug!("crostini IME: activate_x11 on an unassociated object");
            return Action::Drop;
        };
        self.activate_x11(ctx, text_input, seat, x11_window_id)
    }
}

impl ZcrTextInputX11V1Handler for TextInputCrostiniHandler {
    fn on_activate(
        &mut self,
        ctx: &mut Context,
        text_input: u32,
        seat: u32,
        x11_window_id: u32,
    ) -> Action {
        // Deprecated path: `text_input` is passed explicitly instead of being
        // recovered from the crostini object's user data.
        self.activate_x11(ctx, text_input, seat, x11_window_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Context;

    fn opcode(msg: &[u8]) -> u16 {
        u32::from_ne_bytes(msg[4..8].try_into().unwrap()) as u16
    }

    #[test]
    fn get_text_input_crostini_is_local_and_enforces_uniqueness() {
        let mut h = TextInputCrostiniHandler::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.last_sender_id = 10; // manager object
        assert!(matches!(
            h.on_get_text_input_crostini(&mut ctx, 20, 30),
            Action::Drop
        ));
        assert!(ctx.client_to_host_queue.is_empty());
        assert_eq!(
            ctx.shadow_table.get_interface(20).map(|s| s.as_str()),
            Some("zcr_text_input_crostini_v1")
        );

        // A second association is a protocol error on the manager.
        assert!(matches!(
            h.on_get_text_input_crostini(&mut ctx, 21, 30),
            Action::Drop
        ));
        assert_eq!(ctx.host_to_client_queue.len(), 1);
        assert_eq!(opcode(&ctx.host_to_client_queue[0].0), EVT_ERROR);
    }

    #[test]
    fn activate_x11_sends_host_text_input_activate() {
        let mut h = TextInputCrostiniHandler::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.is_xwayland_client = true;
        ctx.last_sender_id = 10;
        h.on_get_text_input_crostini(&mut ctx, 20, 30);
        ctx.shadow_table.map_id(30, 130);
        ctx.shadow_table.map_id(40, 140);
        let (bridge, _rx) = X11Bridge::new();
        bridge.register_window(0x0040_0001, 999);
        ctx.x11_bridge = Some(bridge);

        ctx.last_sender_id = 20; // crostini object
        assert!(matches!(
            h.on_activate_x11(&mut ctx, 40, 0x0040_0001),
            Action::Drop
        ));
        let (msg, fds) = ctx.client_to_host_queue.last().unwrap();
        assert!(fds.is_empty());
        assert_eq!(u32::from_ne_bytes(msg[0..4].try_into().unwrap()), 130);
        assert_eq!(opcode(msg), REQ_ACTIVATE);
        assert_eq!(u32::from_ne_bytes(msg[8..12].try_into().unwrap()), 140);
        assert_eq!(u32::from_ne_bytes(msg[12..16].try_into().unwrap()), 999);
    }

    #[test]
    fn deprecated_x11_activate_uses_the_explicit_text_input() {
        let mut h = TextInputCrostiniHandler::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.is_xwayland_client = true;
        ctx.shadow_table.map_id(30, 130);
        ctx.shadow_table.map_id(40, 140);
        let (bridge, _rx) = X11Bridge::new();
        bridge.register_window(7, 999);
        ctx.x11_bridge = Some(bridge);
        assert!(matches!(h.on_activate(&mut ctx, 30, 40, 7), Action::Drop));
        assert_eq!(ctx.client_to_host_queue.len(), 1);
        assert_eq!(opcode(&ctx.client_to_host_queue[0].0), REQ_ACTIVATE);
    }

    use crate::x11::bridge::X11Bridge;
}
