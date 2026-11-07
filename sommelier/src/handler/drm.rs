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

//! Local `wl_drm` v2 emulation — the Rust counterpart of mainline
//! `compositor/sommelier-drm.cc`.
//!
//! Mainline does **not** proxy a host `wl_drm`. It registers a local
//! `wl_drm` global whenever the host advertises `zwp_linux_dmabuf_v1` >= 2,
//! binds a *separate* host dmabuf proxy pinned to v2, and translates the one
//! request Xwayland's glamor actually uses:
//!
//! | guest `wl_drm` | host `zwp_linux_dmabuf_v1` |
//! |---|---|
//! | `authenticate(id)` | nothing; reply `authenticated` immediately |
//! | `create_prime_buffer(id, fd, ...)` | `create_params` → `add(fd, ...)` → `create_immed` → `destroy` |
//!
//! `device` is a render-node **path string** (not an fd) and `authenticated`
//! is unconditional: render nodes have no DRM master, so no real
//! `drmAuthMagic` is needed. The v2 pin matters because dmabuf v4+ rejects
//! format/modifier pairs it did not advertise (`invalid_format`).

use crate::protocols::drm::wl_drm::{
    WlDrmHandler, EVT_AUTHENTICATED, EVT_CAPABILITIES, EVT_DEVICE, EVT_FORMAT,
};
use crate::protocols::linux_dmabuf_v1::zwp_linux_buffer_params_v1::{
    REQ_ADD, REQ_CREATE_IMMED, REQ_DESTROY,
};
use crate::protocols::linux_dmabuf_v1::zwp_linux_dmabuf_v1::REQ_CREATE_PARAMS;
use crate::protocols::wayland::wl_display::EVT_ERROR;
use crate::state::Context;
use crate::wire::{Action, MessageBuilder};
use std::os::fd::BorrowedFd;
use std::os::unix::io::{IntoRawFd, RawFd};

/// `WL_DRM_CAPABILITY_PRIME` (drm.xml: capabilities enum).
const CAPABILITY_PRIME: u32 = 1;
/// `WL_DRM_ERROR_INVALID_NAME` (drm.xml: error enum).
const ERROR_INVALID_NAME: u32 = 2;
/// `DRM_FORMAT_MOD_INVALID`: hi/lo halves sent through dmabuf `add`.
const MOD_INVALID_HI: u32 = 0x00ff_ffff;
const MOD_INVALID_LO: u32 = 0xffff_ffff;

/// The only DRM formats mainline's `sl_drm_format` forwards
/// (`compositor/sommelier-drm.cc:139-156`, `compositor/sommelier-formats.cc:57-61`).
const DRM_FORMATS: [u32; 5] = [
    0x3631_4752, // RGB565
    0x3432_5241, // ARGB8888
    0x3432_4241, // ABGR8888
    0x3432_5258, // XRGB8888
    0x3432_4258, // XBGR8888
];

/// `wl_display` is always object id 1 on a Wayland connection.
const WL_DISPLAY_ID: u32 = 1;

/// Locate a render node for Xwayland's glamor backend. Mainline's
/// `open_virtgpu()` additionally checks `drmGetVersion() == "virtio_gpu"`;
/// Crostini exposes a single virtio-gpu render node, so probing the standard
/// minor range is equivalent there. `--force-drm-device` takes precedence
/// (see `proxy::Client::new`).
pub fn find_drm_device() -> Option<String> {
    (128..=191u32)
        .map(|minor| format!("/dev/dri/renderD{minor}"))
        .find(|path| std::path::Path::new(path).exists())
}

fn emit_event(ctx: &mut Context, guest_id: u32, opcode: u16, b: MessageBuilder) {
    ctx.host_to_client_queue
        .push((b.build_message(guest_id, opcode), Vec::new()));
}

/// Send a `wl_display.error` for an unimplemented / invalid request. Mainline
/// uses `assert(0)` for the two name-based requests, which would kill the
/// whole client task here; a protocol error is the correct Wayland behaviour.
fn post_error(ctx: &mut Context, object_id: u32, code: u32, message: &str) {
    let mut b = MessageBuilder::new();
    b.write_u32(object_id);
    b.write_u32(code);
    b.write_string(message);
    let msg = b.build_message(WL_DISPLAY_ID, EVT_ERROR as u16);
    ctx.host_to_client_queue.push((msg, Vec::new()));
}

pub struct DrmHandler;

impl DrmHandler {
    /// Publish the events a freshly bound `wl_drm` expects before it can
    /// create prime buffers. Called from `registry.rs::on_bind`.
    pub fn send_bind_events(ctx: &mut Context, guest_id: u32) {
        if let Some(device) = ctx.drm_device.clone() {
            let mut b = MessageBuilder::new();
            b.write_string(&device);
            emit_event(ctx, guest_id, EVT_DEVICE, b);
        }
        for format in DRM_FORMATS {
            let mut b = MessageBuilder::new();
            b.write_u32(format);
            emit_event(ctx, guest_id, EVT_FORMAT, b);
        }
        let mut b = MessageBuilder::new();
        b.write_u32(CAPABILITY_PRIME);
        emit_event(ctx, guest_id, EVT_CAPABILITIES, b);
    }
}

impl WlDrmHandler for DrmHandler {
    fn on_authenticate(&mut self, ctx: &mut Context, _id: u32) -> Action {
        // compositor/sommelier-drm.cc:30-35 — unconditional, no DRM master.
        emit_event(
            ctx,
            ctx.last_sender_id,
            EVT_AUTHENTICATED,
            MessageBuilder::new(),
        );
        Action::Drop
    }

    fn on_create_buffer(
        &mut self,
        ctx: &mut Context,
        _id: u32,
        _name: u32,
        _width: i32,
        _height: i32,
        _stride: u32,
        _format: u32,
    ) -> Action {
        // Flink/name based buffers are obsolete; mainline aborts on them.
        log::warn!("wl_drm.create_buffer is not supported (use create_prime_buffer)");
        post_error(
            ctx,
            ctx.last_sender_id,
            ERROR_INVALID_NAME,
            "wl_drm.create_buffer is not supported",
        );
        Action::Drop
    }

    fn on_create_planar_buffer(
        &mut self,
        ctx: &mut Context,
        _id: u32,
        _name: u32,
        _width: i32,
        _height: i32,
        _format: u32,
        _offset0: i32,
        _stride0: i32,
        _offset1: i32,
        _stride1: i32,
        _offset2: i32,
        _stride2: i32,
    ) -> Action {
        log::warn!("wl_drm.create_planar_buffer is not supported (use create_prime_buffer)");
        post_error(
            ctx,
            ctx.last_sender_id,
            ERROR_INVALID_NAME,
            "wl_drm.create_planar_buffer is not supported",
        );
        Action::Drop
    }

    fn on_create_prime_buffer(
        &mut self,
        ctx: &mut Context,
        id: u32,
        name: RawFd,
        width: i32,
        height: i32,
        format: u32,
        offset0: i32,
        stride0: i32,
        offset1: i32,
        stride1: i32,
        offset2: i32,
        stride2: i32,
    ) -> Action {
        let guest_drm_id = ctx.last_sender_id;

        // compositor/sommelier-drm.cc:88-92 — multi-planar is rejected.
        if offset1 != 0 || stride1 != 0 || offset2 != 0 || stride2 != 0 {
            post_error(
                ctx,
                guest_drm_id,
                ERROR_INVALID_NAME,
                "multiplanar buffers not supported",
            );
            return Action::Drop;
        }

        let Some(dmabuf) = ctx.host_drm_dmabuf_id else {
            log::warn!("wl_drm.create_prime_buffer without an internal host dmabuf v2 proxy");
            post_error(
                ctx,
                guest_drm_id,
                ERROR_INVALID_NAME,
                "host dmabuf v2 is unavailable",
            );
            return Action::Drop;
        };

        // The proxy closes every fd consumed from the guest connection after
        // dispatch, so take our own reference before queueing it to the host.
        let dup_fd = match nix::unistd::dup(unsafe { BorrowedFd::borrow_raw(name) }) {
            Ok(d) => d.into_raw_fd(),
            Err(e) => {
                log::error!("wl_drm: dup of prime fd failed: {e}");
                post_error(
                    ctx,
                    guest_drm_id,
                    ERROR_INVALID_NAME,
                    "could not duplicate prime fd",
                );
                return Action::Drop;
            }
        };

        // Create a host `zwp_linux_buffer_params_v1`. It needs a host-side
        // interface registration or host `created` / `failed` events would be
        // treated as coming from an unknown object.
        let params_host = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table
            .track_host_interface(params_host, "zwp_linux_buffer_params_v1".to_string());
        let mut b = MessageBuilder::new();
        b.write_u32(params_host);
        ctx.client_to_host_queue
            .push((b.build_message(dmabuf, REQ_CREATE_PARAMS), Vec::new()));

        // `add` carries the fd out of band (build_message drops builder.fds).
        let mut b = MessageBuilder::new();
        b.write_u32(0); // plane_idx
        b.write_u32(offset0 as u32);
        b.write_u32(stride0 as u32);
        b.write_u32(MOD_INVALID_HI);
        b.write_u32(MOD_INVALID_LO);
        ctx.client_to_host_queue
            .push((b.build_message(params_host, REQ_ADD), vec![dup_fd]));

        // `create_immed`'s new_id is a wl_buffer in the host namespace. Map
        // the guest id manually because we are dropping the original request.
        let host_buffer_id = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table.map_id(id, host_buffer_id);
        ctx.shadow_table
            .track_interface(id, "wl_buffer".to_string());
        let mut b = MessageBuilder::new();
        b.write_u32(host_buffer_id);
        b.write_i32(width);
        b.write_i32(height);
        b.write_u32(format);
        b.write_u32(0); // flags: Y_INVERT is not supported by wl_drm
        ctx.client_to_host_queue
            .push((b.build_message(params_host, REQ_CREATE_IMMED), Vec::new()));

        // The params object is single-use (sommelier-drm.cc:122).
        ctx.client_to_host_queue.push((
            MessageBuilder::new().build_message(params_host, REQ_DESTROY),
            Vec::new(),
        ));

        log::debug!(
            "wl_drm.create_prime_buffer {}x{} fmt={:#x} stride={} -> host buffer {}",
            width,
            height,
            format,
            stride0,
            host_buffer_id
        );
        Action::Drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocols::wayland::wl_display;

    fn opcode(msg: &[u8]) -> u16 {
        u32::from_ne_bytes(msg[4..8].try_into().unwrap()) as u16
    }

    fn sender(msg: &[u8]) -> u32 {
        u32::from_ne_bytes(msg[0..4].try_into().unwrap())
    }

    #[test]
    fn authenticate_replies_with_authenticated_locally() {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.last_sender_id = 42;
        assert!(matches!(
            DrmHandler.on_authenticate(&mut ctx, 7),
            Action::Drop
        ));
        assert!(ctx.client_to_host_queue.is_empty());
        assert_eq!(ctx.host_to_client_queue.len(), 1);
        let (msg, fds) = &ctx.host_to_client_queue[0];
        assert!(fds.is_empty());
        assert_eq!(sender(msg), 42);
        assert_eq!(opcode(msg), EVT_AUTHENTICATED);
    }

    #[test]
    fn bind_events_publish_device_formats_and_prime_capability() {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.drm_device = Some("/dev/dri/renderD128".to_string());
        DrmHandler::send_bind_events(&mut ctx, 55);
        assert_eq!(ctx.host_to_client_queue.len(), 7);
        assert_eq!(opcode(&ctx.host_to_client_queue[0].0), EVT_DEVICE);
        for entry in &ctx.host_to_client_queue[1..6] {
            assert_eq!(opcode(&entry.0), EVT_FORMAT);
        }
        assert_eq!(opcode(&ctx.host_to_client_queue[6].0), EVT_CAPABILITIES);
        assert!(ctx.host_to_client_queue.iter().all(|e| sender(&e.0) == 55));
    }

    #[test]
    fn create_prime_buffer_translates_to_host_dmabuf_v2_requests() {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.last_sender_id = 42;
        ctx.host_drm_dmabuf_id = Some(99);
        // A real fd is needed because the handler dups it.
        let fd = nix::unistd::dup(std::io::stdin()).unwrap().into_raw_fd();

        assert!(matches!(
            DrmHandler.on_create_prime_buffer(
                &mut ctx,
                77,
                fd,
                800,
                600,
                0x3432_5258,
                0,
                3200,
                0,
                0,
                0,
                0
            ),
            Action::Drop
        ));
        assert_eq!(ctx.client_to_host_queue.len(), 4);
        assert_eq!(sender(&ctx.client_to_host_queue[0].0), 99);
        assert_eq!(opcode(&ctx.client_to_host_queue[0].0), REQ_CREATE_PARAMS);
        assert_eq!(opcode(&ctx.client_to_host_queue[1].0), REQ_ADD);
        assert_eq!(ctx.client_to_host_queue[1].1.len(), 1);
        assert_eq!(opcode(&ctx.client_to_host_queue[2].0), REQ_CREATE_IMMED);
        assert_eq!(opcode(&ctx.client_to_host_queue[3].0), REQ_DESTROY);
        assert_eq!(
            ctx.shadow_table.get_host_id(77),
            Some(u32::from_ne_bytes(
                ctx.client_to_host_queue[2].0[8..12].try_into().unwrap()
            ))
        );
        for (_, fds) in &ctx.client_to_host_queue {
            for f in fds {
                let _ = nix::unistd::close(*f);
            }
        }
    }

    #[test]
    fn multiplanar_prime_buffer_is_rejected() {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.last_sender_id = 42;
        ctx.host_drm_dmabuf_id = Some(99);
        assert!(matches!(
            DrmHandler.on_create_prime_buffer(&mut ctx, 77, -1, 8, 8, 0, 0, 32, 0, 32, 0, 0),
            Action::Drop
        ));
        assert!(ctx.client_to_host_queue.is_empty());
        assert_eq!(ctx.host_to_client_queue.len(), 1);
        assert_eq!(
            opcode(&ctx.host_to_client_queue[0].0),
            wl_display::EVT_ERROR as u16
        );
    }
}
