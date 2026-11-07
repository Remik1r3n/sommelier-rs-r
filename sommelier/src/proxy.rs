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

use crate::config::{Config, TransportKind};
use crate::connection::WaylandConnection;
use crate::protocols;
use crate::state::Context;
use crate::transform::Transform;
use crate::virtgpu_channel::VirtGpuChannel;
use crate::virtwl_channel::VirtWaylandChannel;
use crate::wire::{MessageBuilder, ProtocolError, WireMessage};
use crate::x11::{WmCommand, WmEvent, X11Bridge, X11WindowInfo};
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use std::collections::HashMap;
use std::os::fd::BorrowedFd;
use std::os::unix::io::{IntoRawFd, RawFd};
use std::sync::{Arc, Mutex};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedReceiver;

type DispatchResult = Result<Option<(Vec<u8>, Vec<RawFd>)>, ProtocolError>;

#[derive(Debug, Clone, Copy)]
enum Direction {
    ClientToHost,
    HostToClient,
}

struct SommelierHandler {
    display: crate::handler::display::DisplayHandler,
    registry: crate::handler::registry::RegistryHandler,
    compositor: crate::handler::compositor::CompositorHandler,
    callback: crate::handler::callback::CallbackHandler,
    shm: crate::handler::shm::ShmHandler,
    linux_dmabuf: crate::handler::linux_dmabuf::LinuxDmabufHandler,
    data_device: crate::handler::data_device::DataDeviceHandler,
    text_input_manager_v1: crate::handler::text_input::TextInputManagerV1Handler,
    text_input_v1: crate::handler::text_input::TextInputV1Handler,
    text_input_extension_v1: crate::handler::text_input::TextInputExtensionV1Handler,
    extended_text_input_v1: crate::handler::text_input::ExtendedTextInputV1Handler,
    text_input_manager_v3: crate::handler::text_input::TextInputManagerV3Handler,
    text_input_v3: crate::handler::text_input::TextInputV3Handler,
    keyboard: crate::handler::keyboard::KeyboardHandler,
    seat: crate::handler::seat::SeatHandler,
    output: crate::handler::output::OutputHandler,
    viewport: crate::handler::viewport::ViewportHandler,
    gtk_shell1: crate::handler::gtk_shell::GtkShellHandler,
    gtk_surface1: crate::handler::gtk_shell::GtkSurfaceHandler,
}

impl SommelierHandler {
    fn new() -> Self {
        Self {
            display: crate::handler::display::DisplayHandler,
            registry: crate::handler::registry::RegistryHandler,
            compositor: crate::handler::compositor::CompositorHandler,
            callback: crate::handler::callback::CallbackHandler,
            shm: crate::handler::shm::ShmHandler,
            linux_dmabuf: crate::handler::linux_dmabuf::LinuxDmabufHandler,
            data_device: crate::handler::data_device::DataDeviceHandler,
            text_input_manager_v1: crate::handler::text_input::TextInputManagerV1Handler,
            text_input_v1: crate::handler::text_input::TextInputV1Handler,
            text_input_extension_v1: crate::handler::text_input::TextInputExtensionV1Handler,
            extended_text_input_v1: crate::handler::text_input::ExtendedTextInputV1Handler,
            text_input_manager_v3: crate::handler::text_input::TextInputManagerV3Handler,
            text_input_v3: crate::handler::text_input::TextInputV3Handler,
            keyboard: crate::handler::keyboard::KeyboardHandler::new(),
            seat: crate::handler::seat::SeatHandler,
            output: crate::handler::output::OutputHandler::new(),
            viewport: crate::handler::viewport::ViewportHandler,
            gtk_shell1: crate::handler::gtk_shell::GtkShellHandler::new(),
            gtk_surface1: crate::handler::gtk_shell::GtkSurfaceHandler,
        }
    }
}

/// Host-side objects created by the X11 WM for one X11 top-level window.
#[derive(Debug, Clone, Copy)]
struct HostToplevel {
    host_surface: u32,
    xdg_surface: u32,
    xdg_toplevel: u32,
    aura_surface: u32,
}

struct Client {
    client_conn: WaylandConnection,
    host_conn: WaylandConnection,
    ctx: Context,
    handler: SommelierHandler,
    /// Only the dedicated Xwayland client owns the WM command receiver.
    wm_commands: Option<UnboundedReceiver<WmCommand>>,
    x11: Option<Arc<X11Bridge>>,
    toplevels: HashMap<u32, HostToplevel>,
}

impl Client {
    fn new(
        client_conn: WaylandConnection,
        host_conn: WaylandConnection,
        cfg: &Config,
        bridge: Option<Arc<X11Bridge>>,
        wm_commands: Option<UnboundedReceiver<WmCommand>>,
        is_xwayland_client: bool,
    ) -> Self {
        let mut ctx = Context::new(cfg.gpu_accel, cfg.xdg_decoration);
        ctx.apply_runtime(
            cfg.transform(),
            cfg.quirks.clone(),
            bridge.clone(),
            is_xwayland_client,
        );
        ctx.vm_identifier = cfg.vm_identifier.clone();
        ctx.damage_buffer = cfg.damage_buffer;
        ctx.viewport_resize = cfg.viewport_resize;
        ctx.desired_scale = cfg.desired_scale;
        // `--force-drm-device` also selects the GBM node used by the local
        // SHM/memfd fallback allocator.
        if let Some(path) = &cfg.force_drm_device {
            match crate::allocator::Allocator::with_device(Some(path)) {
                Ok(dev) => ctx.allocator = Some(dev),
                Err(e) => log::warn!("Forced DRM device {} unusable: {}", path, e),
            }
        }
        Self {
            client_conn,
            host_conn,
            ctx,
            handler: SommelierHandler::new(),
            wm_commands,
            x11: bridge,
            toplevels: HashMap::new(),
        }
    }

    async fn run(mut self) {
        self.ctx.shadow_table.map_id(1, 1);
        self.ctx
            .shadow_table
            .track_interface(1, "wl_display".to_string());

        // Take the receiver out of `self` so the select! futures borrow
        // disjoint locals rather than `&mut self` twice.
        let mut wm_commands = self.wm_commands.take();
        // X11 clipboard changes do not generate Wayland traffic of their own;
        // poll them on a timer even when the guest connection is idle.
        let mut clipboard_tick = tokio::time::interval(std::time::Duration::from_millis(50));
        clipboard_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                res = self.client_conn.recv() => {
                    match res {
                        Ok(bytes) => {
                            // Restored from `main`: an EOF with pending fds is
                            // not a disconnect (SCM_RIGHTS can outlive data).
                            if bytes == 0 && self.client_conn.read_fds.is_empty() { break; }
                            if !self.handle_msgs(Direction::ClientToHost).await { break; }
                        }
                        Err(_) => break,
                    }
                }
                res = self.host_conn.recv() => {
                    match res {
                        Ok(bytes) => {
                            if bytes == 0 && self.host_conn.read_fds.is_empty() { break; }
                            if !self.handle_msgs(Direction::HostToClient).await { break; }
                        }
                        Err(_) => break,
                    }
                }
                cmd = async {
                    match wm_commands.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match cmd {
                        Some(command) => self.handle_wm_command(command).await,
                        None => {
                            // WM thread went away; keep proxying Wayland clients.
                            wm_commands = None;
                        }
                    }
                }
                _ = clipboard_tick.tick() => {}
            }
            self.poll_selection().await;
        }
    }

    /// Drive the X11 <-> Wayland clipboard state machine and flush any
    /// synthetic Wayland messages it produced. `handle_msgs` already drains
    /// the two queues for traffic-driven handlers; this path covers changes
    /// that originate in the X11 WM (owner changes, ConvertSelection results).
    async fn poll_selection(&mut self) {
        if self.x11.is_none() {
            return;
        }
        {
            let selection = self.ctx.selection.clone();
            let mut guard = selection
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.poll(&mut self.ctx);
        }

        let mut client_data = Vec::new();
        let mut client_fds = Vec::new();
        for (msg, fds) in self.ctx.host_to_client_queue.drain(..) {
            client_data.extend_from_slice(&msg);
            client_fds.extend(fds);
        }
        if !client_data.is_empty()
            && self
                .client_conn
                .send(&client_data, &client_fds)
                .await
                .is_err()
        {
            log::debug!("x11: failed to deliver clipboard events to guest");
        }
        for fd in client_fds {
            let _ = nix::unistd::close(fd);
        }

        let mut host_data = Vec::new();
        let mut host_fds = Vec::new();
        for (msg, fds) in self.ctx.client_to_host_queue.drain(..) {
            host_data.extend_from_slice(&msg);
            host_fds.extend(fds);
        }
        if !host_data.is_empty() && self.host_conn.send(&host_data, &host_fds).await.is_err() {
            log::debug!("x11: failed to deliver clipboard requests to host");
        }
        for fd in host_fds {
            let _ = nix::unistd::close(fd);
        }
    }

    fn dispatch_request(
        handler: &mut SommelierHandler,
        ctx: &mut Context,
        interface: &str,
        msg: &mut WireMessage,
    ) -> DispatchResult {
        protocols::all::dispatch_request(interface, msg, handler, ctx)
    }

    fn dispatch_event(
        handler: &mut SommelierHandler,
        ctx: &mut Context,
        interface: &str,
        msg: &mut WireMessage,
    ) -> DispatchResult {
        protocols::all::dispatch_event(interface, msg, handler, ctx)
    }

    async fn handle_msgs(&mut self, direction: Direction) -> bool {
        let (conn, other_conn) = match direction {
            Direction::ClientToHost => (&mut self.client_conn, &mut self.host_conn),
            Direction::HostToClient => (&mut self.host_conn, &mut self.client_conn),
        };

        let mut out_buffer = Vec::new();
        let mut out_fds = Vec::new();
        let mut offset = 0;
        let mut fd_offset = 0;

        while offset + 8 <= conn.read_buf.len() {
            let sender_id =
                u32::from_ne_bytes(conn.read_buf[offset..offset + 4].try_into().unwrap());
            let word2 =
                u32::from_ne_bytes(conn.read_buf[offset + 4..offset + 8].try_into().unwrap());
            let len = (word2 >> 16) as usize;
            let opcode = (word2 & 0xFFFF) as u16;

            if len < 8 {
                log::error!("Invalid message length: {}", len);
                return false;
            }
            if offset + len > conn.read_buf.len() {
                break;
            }

            let packet = &conn.read_buf[offset..offset + len];

            let guest_id = match direction {
                Direction::ClientToHost => Some(sender_id),
                Direction::HostToClient => self.ctx.shadow_table.get_guest_id(sender_id),
            };

            let interface = match direction {
                Direction::ClientToHost => self.ctx.shadow_table.get_interface(sender_id).cloned(),
                Direction::HostToClient => self
                    .ctx
                    .shadow_table
                    .get_host_interface(sender_id)
                    .or_else(|| guest_id.and_then(|gid| self.ctx.shadow_table.get_interface(gid)))
                    .cloned(),
            };

            // X11 WM needs host configure/close events for toplevels it created.
            let is_x11_toplevel_event = self.ctx.is_xwayland_client
                && matches!(direction, Direction::HostToClient)
                && interface.as_deref() == Some("xdg_toplevel");

            let mut consumed_fds = 0;
            let result = if let Some(interface) = interface {
                let mut msg =
                    WireMessage::new(sender_id, opcode, &packet[8..], &conn.read_fds[fd_offset..]);

                log::trace!("[{:?}] {}:{} (len={})", direction, interface, opcode, len);
                self.ctx.last_sender_id = sender_id;
                match direction {
                    Direction::ClientToHost => self.ctx.last_guest_sender_id = sender_id,
                    Direction::HostToClient => self.ctx.last_host_sender_id = sender_id,
                }

                if matches!(direction, Direction::HostToClient) {
                    maybe_update_host_serial(&mut self.ctx, &interface, opcode, &packet[8..]);
                }

                // Objects we created solely to export the X11 CLIPBOARD into
                // the host compositor have no guest-side counterpart. Their
                // events must be consumed here, never forwarded or rewritten.
                let internal_selection = matches!(direction, Direction::HostToClient)
                    && self.ctx.is_internal_selection_id(sender_id);

                let res = if internal_selection {
                    handle_internal_selection_event(&self.ctx, &interface, opcode, &mut msg);
                    Ok(None)
                } else {
                    match direction {
                        Direction::ClientToHost => Self::dispatch_request(
                            &mut self.handler,
                            &mut self.ctx,
                            &interface,
                            &mut msg,
                        ),
                        Direction::HostToClient => Self::dispatch_event(
                            &mut self.handler,
                            &mut self.ctx,
                            &interface,
                            &mut msg,
                        ),
                    }
                };
                consumed_fds = msg.fd_offset;
                res
            } else {
                if guest_id.is_some() {
                    log::warn!(
                        "[{:?}] unknown id {} opcode {} (len={})",
                        direction,
                        sender_id,
                        opcode,
                        len
                    );
                }
                Ok(None)
            };

            let fatal = match &result {
                Err(ProtocolError::UnknownOpcode(_)) => {
                    // A single malformed message should not kill every app on
                    // this connection. Drop just this message and resync at
                    // the next 8-byte boundary (we already know `len`).
                    log::warn!("Unknown opcode {}; dropping one message", opcode);
                    false
                }
                Err(e) => {
                    log::error!(
                        "Protocol error: {} (direction={:?}, id={}, opcode={})",
                        e,
                        direction,
                        sender_id,
                        opcode
                    );
                    true
                }
                Ok(_) => false,
            };
            if fatal {
                return false;
            }

            if let Ok(Some((mut data, fds))) = result {
                if let Some(gid) = guest_id {
                    let target_id = match direction {
                        Direction::ClientToHost => self.ctx.shadow_table.get_host_id(gid),
                        Direction::HostToClient => Some(gid),
                    };
                    if let Some(tid) = target_id {
                        if data.len() >= 4 {
                            data[0..4].copy_from_slice(&tid.to_ne_bytes());
                        }
                    }
                }
                out_buffer.extend_from_slice(&data);
                out_fds.extend(fds);
            }

            if is_x11_toplevel_event {
                forward_x11_toplevel_event(
                    &self.ctx.transform,
                    &self.toplevels,
                    self.x11.as_deref(),
                    sender_id,
                    opcode,
                    &packet[8..],
                );
            }

            fd_offset += consumed_fds;

            let queue = match direction {
                Direction::ClientToHost => &mut self.ctx.client_to_host_queue,
                Direction::HostToClient => &mut self.ctx.host_to_client_queue,
            };
            for (p_data, p_fds) in queue.drain(..) {
                out_buffer.extend_from_slice(&p_data);
                out_fds.extend(p_fds);
            }

            offset += len;
        }

        let mut success = true;
        if !out_buffer.is_empty() && other_conn.send(&out_buffer, &out_fds).await.is_err() {
            success = false;
        }

        let mut fds_to_close: std::collections::HashSet<RawFd> = std::collections::HashSet::new();
        fds_to_close.extend(out_fds.iter());

        // Bidirectional queue: a host→client key event can queue an ack_key
        // back to the host. Flush it through the source connection.
        let mut reverse_out_fds = Vec::new();
        let reverse_queue = match direction {
            Direction::ClientToHost => &mut self.ctx.host_to_client_queue,
            Direction::HostToClient => &mut self.ctx.client_to_host_queue,
        };
        let mut reverse_out_buf = Vec::new();
        for (p_data, p_fds) in reverse_queue.drain(..) {
            reverse_out_buf.extend_from_slice(&p_data);
            reverse_out_fds.extend(p_fds);
        }
        if !reverse_out_buf.is_empty()
            && conn.send(&reverse_out_buf, &reverse_out_fds).await.is_err()
        {
            success = false;
        }
        fds_to_close.extend(reverse_out_fds.iter());
        fds_to_close.extend(conn.read_fds.iter().take(fd_offset));

        for fd in fds_to_close {
            let _ = nix::unistd::close(fd);
        }

        conn.read_fds.drain(..fd_offset);
        conn.read_buf.drain(..offset);

        publish_x11_surfaces(&self.ctx, self.x11.as_deref());
        success
    }

    /// Handle a command from the X11 WM by synthesizing Wayland requests on
    /// Xwayland's host connection. Equivalent to mainline `sl_window_update()`
    /// creating `xdg_surface` / `xdg_toplevel` / `zaura_surface`.
    async fn handle_wm_command(&mut self, command: WmCommand) {
        match command {
            WmCommand::CreateToplevel(info) => self.create_toplevel(&info),
            WmCommand::DestroyToplevel { xid } => self.destroy_toplevel(xid),
            WmCommand::UpdateToplevel { xid, info } => self.update_toplevel(xid, &info),
            WmCommand::Activate { xid } => {
                if let Some(t) = self.toplevels.get(&xid) {
                    let b = MessageBuilder::new();
                    self.ctx.client_to_host_queue.push((
                        b.build_message(
                            t.aura_surface,
                            protocols::aura_shell::zaura_surface::REQ_ACTIVATE as u16,
                        ),
                        Vec::new(),
                    ));
                }
            }
            WmCommand::MoveResize { xid, edges } => {
                log::debug!(
                    "X11 _NET_WM_MOVERESIZE xid={} edges={} (host move/resize needs a seat serial)",
                    xid,
                    edges
                );
            }
            WmCommand::Commit { xid } => {
                if let Some(t) = self.toplevels.get(&xid) {
                    let b = MessageBuilder::new();
                    self.ctx.client_to_host_queue.push((
                        b.build_message(
                            t.host_surface,
                            protocols::wayland::wl_surface::REQ_COMMIT as u16,
                        ),
                        Vec::new(),
                    ));
                }
            }
        }

        let mut data = Vec::new();
        let mut fds = Vec::new();
        for (msg, msg_fds) in self.ctx.client_to_host_queue.drain(..) {
            data.extend_from_slice(&msg);
            fds.extend(msg_fds);
        }
        if !data.is_empty() && self.host_conn.send(&data, &fds).await.is_err() {
            log::error!("Failed to send X11 WM commands to host");
        }
        for fd in fds {
            let _ = nix::unistd::close(fd);
        }
    }

    fn create_toplevel(&mut self, info: &X11WindowInfo) {
        let Some(bridge) = &self.x11 else { return };
        let Some(host_surface) = bridge.host_surface(info.surface_guest_id) else {
            log::warn!(
                "X11 window {:x} paired with unpublished surface {}",
                info.xid,
                info.surface_guest_id
            );
            return;
        };
        let Some(xdg_wm_base) = self.ctx.host_xdg_wm_base_id else {
            log::warn!("X11 window {:x}: xdg_wm_base not bound yet", info.xid);
            return;
        };

        let xdg_surface = self.ctx.shadow_table.allocate_host_id();
        let xdg_toplevel = self.ctx.shadow_table.allocate_host_id();
        let aura_surface = self.ctx.shadow_table.allocate_host_id();
        self.ctx
            .shadow_table
            .track_host_interface(xdg_surface, "xdg_surface".to_string());
        self.ctx
            .shadow_table
            .track_host_interface(xdg_toplevel, "xdg_toplevel".to_string());
        self.ctx
            .shadow_table
            .track_host_interface(aura_surface, "zaura_surface".to_string());

        let mut b = MessageBuilder::new();
        b.write_u32(xdg_surface);
        b.write_u32(host_surface);
        self.ctx.client_to_host_queue.push((
            b.build_message(
                xdg_wm_base,
                protocols::xdg_shell::xdg_wm_base::REQ_GET_XDG_SURFACE as u16,
            ),
            Vec::new(),
        ));

        let mut b = MessageBuilder::new();
        b.write_u32(xdg_toplevel);
        self.ctx.client_to_host_queue.push((
            b.build_message(
                xdg_surface,
                protocols::xdg_shell::xdg_surface::REQ_GET_TOPLEVEL as u16,
            ),
            Vec::new(),
        ));

        if let Some(aura_shell) = self.ctx.host_zaura_shell_id {
            let mut b = MessageBuilder::new();
            b.write_u32(aura_surface);
            b.write_u32(host_surface);
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    aura_shell,
                    protocols::aura_shell::zaura_shell::REQ_GET_AURA_SURFACE as u16,
                ),
                Vec::new(),
            ));
            if self.ctx.host_zaura_shell_version >= 5 && !info.app_id.is_empty() {
                let mut b = MessageBuilder::new();
                b.write_string(&info.app_id);
                self.ctx.client_to_host_queue.push((
                    b.build_message(
                        aura_surface,
                        protocols::aura_shell::zaura_surface::REQ_SET_APPLICATION_ID as u16,
                    ),
                    Vec::new(),
                ));
            }
        }

        self.toplevels.insert(
            info.xid,
            HostToplevel {
                host_surface,
                xdg_surface,
                xdg_toplevel,
                aura_surface,
            },
        );
        bridge.register_window(info.xid, host_surface);
        self.update_toplevel(info.xid, info);

        let b = MessageBuilder::new();
        self.ctx.client_to_host_queue.push((
            b.build_message(
                host_surface,
                protocols::wayland::wl_surface::REQ_COMMIT as u16,
            ),
            Vec::new(),
        ));
        log::info!(
            "X11 window {:x} -> host surface {} / xdg_toplevel {}",
            info.xid,
            host_surface,
            xdg_toplevel
        );
    }

    fn update_toplevel(&mut self, xid: u32, info: &X11WindowInfo) {
        let Some(t) = self.toplevels.get(&xid).copied() else {
            return;
        };
        let transform = self.ctx.transform.clone();

        if !info.title.is_empty() {
            let mut b = MessageBuilder::new();
            b.write_string(&info.title);
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_TITLE as u16,
                ),
                Vec::new(),
            ));
        }
        let (min_w, min_h) = transform.guest_to_host_size(info.min_size.0, info.min_size.1);
        if min_w > 0 || min_h > 0 {
            let mut b = MessageBuilder::new();
            b.write_i32(min_w);
            b.write_i32(min_h);
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_MIN_SIZE as u16,
                ),
                Vec::new(),
            ));
        }
        let (max_w, max_h) = transform.guest_to_host_size(info.max_size.0, info.max_size.1);
        if max_w > 0 || max_h > 0 {
            let mut b = MessageBuilder::new();
            b.write_i32(max_w);
            b.write_i32(max_h);
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_MAX_SIZE as u16,
                ),
                Vec::new(),
            ));
        }
        if info.fullscreen {
            let b = MessageBuilder::new();
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_FULLSCREEN as u16,
                ),
                Vec::new(),
            ));
        }
        if info.maximized {
            let b = MessageBuilder::new();
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_MAXIMIZED as u16,
                ),
                Vec::new(),
            ));
        }
        if info.minimized {
            let b = MessageBuilder::new();
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.xdg_toplevel,
                    protocols::xdg_shell::xdg_toplevel::REQ_SET_MINIMIZED as u16,
                ),
                Vec::new(),
            ));
        }
        if info.urgent {
            let b = MessageBuilder::new();
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.aura_surface,
                    protocols::aura_shell::zaura_surface::REQ_DRAW_ATTENTION as u16,
                ),
                Vec::new(),
            ));
        }
    }

    fn destroy_toplevel(&mut self, xid: u32) {
        let Some(t) = self.toplevels.remove(&xid) else {
            return;
        };
        let b = MessageBuilder::new();
        self.ctx.client_to_host_queue.push((
            b.build_message(
                t.xdg_toplevel,
                protocols::xdg_shell::xdg_toplevel::REQ_DESTROY as u16,
            ),
            Vec::new(),
        ));
        let b = MessageBuilder::new();
        self.ctx.client_to_host_queue.push((
            b.build_message(
                t.xdg_surface,
                protocols::xdg_shell::xdg_surface::REQ_DESTROY as u16,
            ),
            Vec::new(),
        ));
        if self.ctx.host_zaura_shell_version >= 38 {
            let b = MessageBuilder::new();
            self.ctx.client_to_host_queue.push((
                b.build_message(
                    t.aura_surface,
                    protocols::aura_shell::zaura_surface::REQ_RELEASE as u16,
                ),
                Vec::new(),
            ));
        }
        if let Some(bridge) = &self.x11 {
            bridge.unregister_window(xid);
        }
    }
}

/// Publish Xwayland's `wl_surface` guest→host id mapping into the shared
/// bridge so the WM can pair `WL_SURFACE_ID` ClientMessages with host
/// surfaces. Free function (not a method) because `handle_msgs` already holds
/// disjoint mutable borrows of two other `Client` fields.
fn publish_x11_surfaces(ctx: &Context, bridge: Option<&X11Bridge>) {
    let Some(bridge) = bridge else { return };
    if !ctx.is_xwayland_client {
        return;
    }
    for guest_id in ctx.shadow_table.find_by_interface("wl_surface") {
        if let Some(host_id) = ctx.shadow_table.get_host_id(guest_id) {
            bridge.publish_surface(guest_id, host_id);
        }
    }
    bridge.set_host_globals(ctx.host_xdg_wm_base_id, ctx.host_zaura_shell_id);
}

/// Translate host `xdg_toplevel.configure` / `close` into `WmEvent`s for the
/// X11 WM (which turns them into ConfigureNotify / WM_DELETE_WINDOW).
/// `xdg_surface.configure` is forwarded to Xwayland by the normal generator
/// path, so Xwayland sends `ack_configure` itself — no synthetic ack needed.
/// `wl_data_device.set_selection` needs a valid input serial (mainline uses
/// `ctx->default_seat->seat->last_serial`). Harvest it from the host events
/// that carry one at the front of their payload.
fn maybe_update_host_serial(ctx: &mut Context, interface: &str, opcode: u16, payload: &[u8]) {
    if payload.len() < 4 {
        return;
    }
    let carries_serial = match interface {
        // enter(0) / button(3), enter(0) / key(3), down(0), enter(1)
        "wl_pointer" => matches!(opcode, 0 | 3),
        "wl_keyboard" => matches!(opcode, 0 | 3),
        "wl_touch" => opcode == 0,
        "wl_data_device" => opcode == 1,
        _ => false,
    };
    if carries_serial {
        ctx.host_serial = u32::from_ne_bytes(payload[..4].try_into().unwrap());
    }
}

/// Consume events addressed at the host objects we created for the X11
/// CLIPBOARD export. Only `wl_data_source.send` carries data we need; every
/// other event (`target`, `cancelled`, `action`, internal data_device offers)
/// is deliberately swallowed so it cannot leak host object ids to the guest.
fn handle_internal_selection_event(
    ctx: &Context,
    interface: &str,
    opcode: u16,
    msg: &mut WireMessage<'_>,
) {
    if interface != "wl_data_source" {
        return;
    }
    if opcode == protocols::wayland::wl_data_source::EVT_SEND as u16 {
        let mime = msg.read_string().unwrap_or_default();
        let fd = match msg.read_fd() {
            Ok(fd) if fd >= 0 => fd,
            _ => {
                log::warn!("x11: host wl_data_source.send arrived without an fd");
                return;
            }
        };
        let selection = ctx.selection.clone();
        let mut guard = selection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.on_host_source_send(ctx, &mime, fd);
    } else if opcode == protocols::wayland::wl_data_source::EVT_CANCELLED as u16 {
        log::debug!("x11: internal host wl_data_source cancelled");
    }
}

fn forward_x11_toplevel_event(
    transform: &Transform,
    toplevels: &HashMap<u32, HostToplevel>,
    bridge: Option<&X11Bridge>,
    sender_id: u32,
    opcode: u16,
    payload: &[u8],
) {
    let Some(bridge) = bridge else { return };
    if toplevels.is_empty() {
        return;
    }
    let found = toplevels
        .iter()
        .find(|(_, t)| t.xdg_toplevel == sender_id)
        .map(|(xid, _)| *xid);
    let Some(xid) = found else { return };

    if opcode == protocols::xdg_shell::xdg_toplevel::EVT_CLOSE as u16 {
        bridge.push_event(WmEvent::Close { xid });
    } else if opcode == protocols::xdg_shell::xdg_toplevel::EVT_CONFIGURE as u16
        && payload.len() >= 8
    {
        let width = i32::from_ne_bytes(payload[0..4].try_into().unwrap());
        let height = i32::from_ne_bytes(payload[4..8].try_into().unwrap());
        let (guest_w, guest_h) = transform.host_to_guest_size(width, height);
        bridge.push_event(WmEvent::Configure {
            xid,
            width: guest_w,
            height: guest_h,
            activated: false,
            fullscreen: false,
            maximized: false,
        });
    }
}

// ---------------------------------------------------------------------------
// Handler delegation. The 11 protocols already present upstream keep their
// hand-written mappings; every newly added protocol gets a pure-forward
// implementation from the generated aggregate macro.
// ---------------------------------------------------------------------------

protocols::wayland::impl_sommelier_delegates!(SommelierHandler, {
    wl_display: display,
    wl_registry: registry,
    wl_callback: callback,
    wl_compositor: compositor,
    wl_surface: compositor,
    wl_subcompositor: compositor,
    wl_subsurface: compositor,
    wl_region: compositor,
    wl_shm: shm,
    wl_shm_pool: shm,
    wl_buffer: shm,
    wl_data_device_manager: data_device,
    wl_data_device: data_device,
    wl_data_source: data_device,
    wl_data_offer: data_device,
    wl_keyboard: keyboard,
    wl_seat: seat,
    wl_output: output
});
impl protocols::wayland::ProtocolHandler for SommelierHandler {}

protocols::xdg_shell::impl_sommelier_delegates!(SommelierHandler, {
    xdg_wm_base: compositor,
    xdg_surface: compositor,
    xdg_toplevel: compositor
});
impl protocols::xdg_shell::ProtocolHandler for SommelierHandler {}

protocols::linux_dmabuf_v1::impl_sommelier_delegates!(SommelierHandler, {
    zwp_linux_dmabuf_v1: linux_dmabuf,
    zwp_linux_buffer_params_v1: linux_dmabuf,
    zwp_linux_dmabuf_feedback_v1: linux_dmabuf
});
impl protocols::linux_dmabuf_v1::ProtocolHandler for SommelierHandler {}

protocols::text_input_unstable_v1::impl_sommelier_delegates!(SommelierHandler, {
    zwp_text_input_manager_v1: text_input_manager_v1,
    zwp_text_input_v1: text_input_v1
});
impl protocols::text_input_unstable_v1::ProtocolHandler for SommelierHandler {}

protocols::text_input_extension_unstable_v1::impl_sommelier_delegates!(SommelierHandler, {
    zcr_text_input_extension_v1: text_input_extension_v1,
    zcr_extended_text_input_v1: extended_text_input_v1
});
impl protocols::text_input_extension_unstable_v1::ProtocolHandler for SommelierHandler {}

protocols::text_input_unstable_v3::impl_sommelier_delegates!(SommelierHandler, {
    zwp_text_input_manager_v3: text_input_manager_v3,
    zwp_text_input_v3: text_input_v3
});
impl protocols::text_input_unstable_v3::ProtocolHandler for SommelierHandler {}

protocols::viewporter::impl_sommelier_delegates!(SommelierHandler, {
    wp_viewport: viewport
});
impl protocols::viewporter::ProtocolHandler for SommelierHandler {}

protocols::xdg_decoration_unstable_v1::impl_sommelier_delegates!(SommelierHandler, {});
impl protocols::xdg_decoration_unstable_v1::ProtocolHandler for SommelierHandler {}

protocols::fractional_scale_v1::impl_sommelier_delegates!(SommelierHandler, {
    wp_fractional_scale_v1: output
});
impl protocols::fractional_scale_v1::ProtocolHandler for SommelierHandler {}

protocols::xdg_output_unstable_v1::impl_sommelier_delegates!(SommelierHandler, {
    zxdg_output_v1: output
});
impl protocols::xdg_output_unstable_v1::ProtocolHandler for SommelierHandler {}

protocols::keyboard_extension_unstable_v1::impl_sommelier_delegates!(SommelierHandler, {
    zcr_keyboard_extension_v1: keyboard,
    zcr_extended_keyboard_v1: keyboard
});
impl protocols::keyboard_extension_unstable_v1::ProtocolHandler for SommelierHandler {}

// zaura_shell is bound internally; its events must never reach the guest.
protocols::aura_shell::impl_sommelier_delegates!(SommelierHandler, {});
impl protocols::aura_shell::ProtocolHandler for SommelierHandler {}

// gtk_shell1 is implemented locally (mainline sommelier-gtk-shell.cc) and is
// deliberately excluded from the generated "pure forward" delegates.
protocols::gtk::impl_sommelier_delegates!(SommelierHandler, {
    gtk_shell1: gtk_shell1,
    gtk_surface1: gtk_surface1
});
impl protocols::gtk::ProtocolHandler for SommelierHandler {}

protocols::all::impl_generated_delegates!(SommelierHandler);

// ---------------------------------------------------------------------------
// Host connection construction (merged from both branches)
// ---------------------------------------------------------------------------

type HostConnection = (
    WaylandConnection,
    Option<Arc<VirtWaylandChannel>>,
    Option<Arc<Mutex<VirtGpuChannel>>>,
);

async fn connect_host(cfg: &Config) -> Option<HostConnection> {
    match cfg.transport {
        TransportKind::VirtWl => match VirtWaylandChannel::new(&cfg.virtwl_device) {
            Ok(channel) => {
                let arc = Arc::new(channel);
                Some((
                    WaylandConnection::new_virtwayland(arc.clone()),
                    Some(arc),
                    None,
                ))
            }
            Err(e) => {
                log::error!("Failed to open virtio-wl {}: {}", cfg.virtwl_device, e);
                None
            }
        },
        TransportKind::VirtGpu => {
            match VirtGpuChannel::new_with_device(cfg.force_drm_device.as_deref()) {
                Ok(mut channel) => match channel.init_context() {
                    Ok(fence) => {
                        let arc = Arc::new(Mutex::new(channel));
                        Some((
                            WaylandConnection::new_virtgpu(arc.clone(), fence),
                            None,
                            Some(arc),
                        ))
                    }
                    Err(e) => {
                        log::error!("Failed to init virtgpu context: {}", e);
                        None
                    }
                },
                Err(e) => {
                    log::error!("Failed to connect to virtgpu: {}", e);
                    None
                }
            }
        }
        TransportKind::Unix => {
            let path = cfg.local_compositor.as_ref()?;
            match UnixStream::connect(path).await {
                Ok(stream) => {
                    let fd = stream.into_std().unwrap().into_raw_fd();
                    set_nonblocking(fd);
                    Some((WaylandConnection::new(fd), None, None))
                }
                Err(e) => {
                    log::error!("Failed to connect to host compositor {}: {}", path, e);
                    None
                }
            }
        }
    }
}

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let _ = fcntl(
            BorrowedFd::borrow_raw(fd),
            FcntlArg::F_SETFL(OFlag::O_NONBLOCK),
        );
    }
}

fn spawn_client(
    client_fd: RawFd,
    cfg: Config,
    bridge: Option<Arc<X11Bridge>>,
    wm_commands: Option<UnboundedReceiver<WmCommand>>,
    is_xwayland_client: bool,
) {
    set_nonblocking(client_fd);
    let client_conn = WaylandConnection::new(client_fd);
    tokio::spawn(async move {
        let Some((host_conn, virtwl, virtgpu)) = connect_host(&cfg).await else {
            log::error!("Failed to establish host connection, closing client");
            return;
        };
        log::info!(
            "Client connected via {} (x11={})",
            host_conn.transport_name(),
            is_xwayland_client
        );
        let mut client = Client::new(
            client_conn,
            host_conn,
            &cfg,
            bridge,
            wm_commands,
            is_xwayland_client,
        );
        client.ctx.virtwayland_channel = virtwl;
        client.ctx.virtgpu_channel = virtgpu;
        client.run().await;
        log::info!("Client disconnected");
    });
}

/// Main proxy entry point.
///
/// `wm_commands` and `xwayland_client_fd` come from `x11::start_x11()`; when
/// X11 forwarding is disabled both are `None` and the listener behaves exactly
/// like upstream sommelier-rs.
pub async fn run(
    cfg: Config,
    bridge: Option<Arc<X11Bridge>>,
    wm_commands: Option<UnboundedReceiver<WmCommand>>,
    xwayland_client_fd: Option<RawFd>,
) {
    let _ = std::fs::remove_file(&cfg.socket_path);

    // Pre-flight check: fail early rather than accepting clients we cannot
    // serve (this is what made `main` unusable on ChromeOS).
    match cfg.transport {
        TransportKind::VirtWl => {
            if let Err(e) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&cfg.virtwl_device)
            {
                log::error!("virtio-wl device {} unavailable: {}", cfg.virtwl_device, e);
            }
        }
        TransportKind::Unix => {
            if let Some(path) = &cfg.local_compositor {
                if std::os::unix::net::UnixStream::connect(path).is_err() {
                    log::warn!("Local compositor {} is not reachable yet", path);
                }
            }
        }
        TransportKind::VirtGpu => {}
    }

    let listener = UnixListener::bind(&cfg.socket_path)
        .unwrap_or_else(|e| panic!("Failed to bind {}: {}", cfg.socket_path.display(), e));
    log::info!(
        "sommelier-rs-r listening on {} (transport={}, X11={})",
        cfg.socket_path.display(),
        cfg.transport.as_str(),
        cfg.x11_enabled
    );

    if cfg.sd_notify {
        crate::notify_systemd();
    }

    // Xwayland is a Wayland client of ours, but it must be identifiable so
    // that its wl_surface IDs can be published to the X11 WM. It arrives on a
    // dedicated socketpair passed as WAYLAND_SOCKET, not via the listener.
    if let Some(fd) = xwayland_client_fd {
        log::info!("Starting dedicated Xwayland client (fd={})", fd);
        spawn_client(fd, cfg.clone(), bridge.clone(), wm_commands, true);
    }

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let fd = stream.into_std().unwrap().into_raw_fd();
                spawn_client(fd, cfg.clone(), bridge.clone(), None, false);
            }
            Err(e) => log::error!("Accept error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_serial_is_harvested_from_input_events() {
        let mut ctx = crate::state::Context::new_for_test(false, false, vec![]);
        maybe_update_host_serial(&mut ctx, "wl_pointer", 0, &7u32.to_ne_bytes());
        assert_eq!(ctx.host_serial, 7);
        maybe_update_host_serial(&mut ctx, "wl_keyboard", 3, &9u32.to_ne_bytes());
        assert_eq!(ctx.host_serial, 9);
        maybe_update_host_serial(&mut ctx, "wl_data_device", 1, &11u32.to_ne_bytes());
        assert_eq!(ctx.host_serial, 11);
    }

    #[test]
    fn host_serial_ignores_events_without_a_serial() {
        let mut ctx = crate::state::Context::new_for_test(false, false, vec![]);
        maybe_update_host_serial(&mut ctx, "wl_pointer", 5, &99u32.to_ne_bytes());
        maybe_update_host_serial(&mut ctx, "wl_output", 0, &99u32.to_ne_bytes());
        maybe_update_host_serial(&mut ctx, "wl_keyboard", 0, &[0u8; 2]);
        assert_eq!(ctx.host_serial, 0);
    }

    #[test]
    fn internal_selection_ids_are_isolated() {
        let mut ctx = crate::state::Context::new_for_test(false, false, vec![]);
        assert!(!ctx.is_internal_selection_id(55));
        ctx.internal_selection_ids.insert(55);
        assert!(ctx.is_internal_selection_id(55));
        assert!(!ctx.is_internal_selection_id(56));
    }
}
