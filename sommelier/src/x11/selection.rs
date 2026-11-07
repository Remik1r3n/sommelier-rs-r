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

//! Wayland-side half of the X11 <-> Wayland selection bridge.
//!
//! Mainline sommelier owns both the X11 `CLIPBOARD` selection and a Wayland
//! `wl_data_device_manager`, so it can translate offers in either direction.
//! In this proxy the host compositor is a *real* Wayland compositor: guest
//! requests are forwarded to it. To make X11 applications share the clipboard
//! we therefore have to do three things on top of plain forwarding:
//!
//! 1. **X11 -> guest Wayland**: when the WM reports a new X11 CLIPBOARD owner
//!    (`set_clipboard_targets`), synthesize a `wl_data_device.data_offer` +
//!    `offer` + `selection` on every guest data device. A subsequent
//!    `wl_data_offer.receive` is turned into a `ConvertSelection` request to
//!    the X11 owner; its bytes are written back into the guest-provided fd.
//! 2. **host Wayland -> X11**: track host `wl_data_offer`s and prefetch their
//!    small/text payloads, publishing them as `WmEvent::ClipboardData` so the
//!    WM can answer X11 `SelectionRequest`s without a synchronous round trip.
//! 3. **guest Wayland -> X11**: track guest `wl_data_source`s, publish their
//!    MIME list as TARGETS and prefetch their payload with a compositor-style
//!    `wl_data_source.send` event.
//!
//! All transfers are asynchronous. Large or non-text payloads are deliberately
//! not prefetched; they are fetched on demand from the X11 owner in direction
//! (1), while directions (2)/(3) fall back to a deferred `SelectionRequest`
//! handled by `ClipboardBridge::flush_pending`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::fd::{IntoRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocols::wayland::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
};
use crate::state::Context;
use crate::wire::MessageBuilder;
use crate::x11::bridge::{WmEvent, X11Bridge};

/// Upper bound for a prefetched payload. Bigger blobs are left to the
/// on-demand X11 ConvertSelection path / INCR transfer.
const PREFETCH_LIMIT: usize = 4 * 1024 * 1024;
/// How long a `wl_data_offer.receive` waits for ConvertSelection data.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Keep at most this many synthetic offers alive per connection.
const MAX_X11_OFFERS: usize = 8;

/// X11 target name for a Wayland MIME type when asking an *X11 selection
/// owner* for data.
///
/// Mainline (`sl_begin_data_source_send`, `sl_send_data`) interns the MIME
/// string verbatim as the X11 target. We keep that 1:1 identity and only
/// add the classic text atoms as extra aliases when *fetching*, because many
/// X11 owners serve `UTF8_STRING` but never intern a literal MIME name.
pub fn x11_target_for_mime(mime: &str) -> String {
    mime.trim().to_string()
}

/// Extra X11 atoms worth trying for a Wayland MIME type. This is a
/// compatibility superset of mainline: the exported MIME names themselves
/// are never rewritten.
pub fn x11_target_aliases(mime: &str) -> Vec<String> {
    let lower = mime.trim().to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "utf8_string"
            | "string"
            | "text"
            | "compound_text"
            | "text/plain"
            | "text/plain;charset=utf-8"
            | "text/plain;charset=utf-8;"
            | "text/plain;charset=us-ascii"
    ) {
        vec![
            "UTF8_STRING".to_string(),
            "STRING".to_string(),
            "TEXT".to_string(),
            "text/plain;charset=utf-8".to_string(),
        ]
    } else {
        Vec::new()
    }
}

/// Best X11 atom to use when *prefetching* a Wayland text MIME from an X11
/// owner. Mainline would use the literal MIME name; UTF8_STRING is a far more
/// widely served target, so it is tried first.
pub fn preferred_x11_target(mime: &str) -> String {
    x11_target_aliases(mime)
        .into_iter()
        .next()
        .unwrap_or_else(|| x11_target_for_mime(mime))
}

/// All X11 atom names an X11 requestor might ask for that should resolve to
/// the same Wayland MIME (1:1 name first, aliases afterwards).
pub fn x11_target_candidates(target: &str) -> Vec<String> {
    let mut out = vec![target.to_string()];
    let lower = target.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "utf8_string" | "string" | "text" | "compound_text"
    ) {
        out.push("text/plain;charset=utf-8".to_string());
        out.push("text/plain".to_string());
    } else if lower.starts_with("text/plain") {
        out.push("UTF8_STRING".to_string());
        out.push("STRING".to_string());
        out.push("TEXT".to_string());
    }
    out
}

/// Map an X11 TARGETS atom back to a Wayland MIME type.
///
/// Mainline `sl_get_selection_targets` (sommelier.cc:2819-2841) uses the atom
/// name verbatim as the `wl_data_source.offer` MIME. Control atoms are never
/// offered. There is deliberately NO UTF8_STRING -> text/plain conversion:
/// that would change what the ChromeOS host sees.
pub fn mime_for_x11_target(target: &str) -> String {
    match target {
        "TARGETS" | "TIMESTAMP" | "MULTIPLE" | "SAVE_TARGETS" | "WL_SELECTION"
        | "_WL_SELECTION" => String::new(),
        _ => target.to_string(),
    }
}

/// X11 targets an X11 requestor should see in `TARGETS`.
///
/// Mainline `sl_set_selection` (sommelier.cc:354-371) puts `TARGETS` and
/// `TIMESTAMP` first, then the MIME names verbatim. We append the classic
/// text atoms as additional aliases so GTK/Qt/Xt clients keep working; they
/// never affect the MIME list exported to the host.
pub fn normalize_targets(mimes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec!["TARGETS".into(), "TIMESTAMP".into()];
    for mime in mimes {
        let name = mime.trim();
        if !name.is_empty() && !out.contains(&name.to_string()) {
            out.push(name.to_string());
        }
        if name.eq_ignore_ascii_case("UTF8_STRING")
            || name.eq_ignore_ascii_case("STRING")
            || name.eq_ignore_ascii_case("TEXT")
            || name.to_ascii_lowercase().starts_with("text/plain")
        {
            for alias in ["UTF8_STRING", "STRING", "TEXT"] {
                if !out.iter().any(|t| t == alias) {
                    out.push(alias.to_string());
                }
            }
        }
    }
    out
}

fn is_prefetchable(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.starts_with("text/")
        || matches!(
            m.as_str(),
            "utf8_string"
                | "string"
                | "text"
                | "compound_text"
                | "image/png"
                | "image/jpeg"
                | "image/bmp"
                | "application/json"
        )
}

#[derive(Debug, Clone)]
struct X11Offer {
    targets: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct HostOffer {
    host_id: u32,
    mimes: Vec<String>,
}

struct PendingReceive {
    /// X11 atom name requested from the WM.
    target: String,
    fd: OwnedFd,
    deadline: Instant,
}

/// A host `wl_data_source.send(mime, fd)` waiting for the bytes of an X11
/// `ConvertSelection`. This is the Rust equivalent of mainline
/// `sl_begin_data_source_send()` + `sl_handle_selection_fd_writable()`.
struct PendingHostSend {
    target: String,
    fd: OwnedFd,
    deadline: Instant,
}

/// Per-Wayland-connection clipboard state. Stored behind
/// `Arc<Mutex<..>>` in `Context` so handlers can hold `&mut Context` at the
/// same time (see `Context::selection`).
pub struct SelectionBridge {
    bridge: Option<Arc<X11Bridge>>,
    /// Guest `wl_data_device` object ids created by `get_data_device`.
    data_devices: HashSet<u32>,
    /// Guest `wl_data_source` id -> offered MIME types.
    sources: HashMap<u32, Vec<String>>,
    active_source: Option<u32>,
    /// Guest offer ids we synthesized for the X11 CLIPBOARD selection.
    x11_offers: VecDeque<(u32, X11Offer)>,
    /// Guest offer id -> host offer state (from host `data_offer` events).
    host_offers: HashMap<u32, HostOffer>,
    active_host_offer: Option<u32>,
    pending_receives: Vec<PendingReceive>,
    last_targets_generation: u64,
    /// (source kind, MIME) pairs already prefetched in this selection epoch,
    /// used to avoid re-issuing `receive`/`send` on every poll.
    prefetched: HashSet<String>,
    /// host `wl_data_source.send` requests awaiting ConvertSelection bytes.
    host_sends: Vec<PendingHostSend>,
    /// TARGETS generation that was last exported to the host compositor.
    exported_targets_generation: u64,
    /// MIME list currently offered by the exported host source.
    exported_mimes: Vec<String>,
}

impl Default for SelectionBridge {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Mainline `sl_connect()` binds `wl_data_device_manager` internally and
/// creates a `wl_data_device` for the default seat, so it can install a
/// `wl_data_source` that represents the X11 CLIPBOARD owner
/// (`sommelier.cc:sl_get_selection_targets`). We do the same, but only on the
/// dedicated Xwayland connection: the host has one selection per seat, so a
/// single exporter is enough for the whole VM.
///
/// The manager is bound in `handler/registry.rs`; the host seat id is learned
/// from the guest's own `get_data_device` request in `handler/data_device.rs`.
/// This function is called from both places and is idempotent.
fn mimes_from_x11_targets(targets: &[String]) -> Vec<String> {
    let mut mimes: Vec<String> = Vec::new();
    for target in targets {
        let mime = mime_for_x11_target(target);
        if !mime.is_empty() && !mimes.contains(&mime) {
            mimes.push(mime);
        }
    }
    mimes
}

pub fn ensure_host_selection_device(ctx: &mut Context) {
    if !ctx.is_xwayland_client || ctx.host_selection_device_id.is_some() {
        return;
    }
    let (Some(manager), Some(seat)) = (ctx.host_data_device_manager_id, ctx.host_seat_id) else {
        return;
    };
    let id = ctx.shadow_table.allocate_host_id();
    ctx.shadow_table
        .track_host_interface(id, "wl_data_device".to_string());
    ctx.internal_selection_ids.insert(id);
    ctx.host_selection_device_id = Some(id);

    let mut b = MessageBuilder::new();
    b.write_u32(id);
    b.write_u32(seat);
    ctx.client_to_host_queue.push((
        b.build_message(manager, wl_data_device_manager::REQ_GET_DATA_DEVICE as u16),
        Vec::new(),
    ));
    log::debug!(
        "x11: created internal host wl_data_device id={} (manager={}, seat={})",
        id,
        manager,
        seat
    );
}

impl SelectionBridge {
    pub fn new(bridge: Option<Arc<X11Bridge>>) -> Self {
        Self {
            bridge,
            data_devices: HashSet::new(),
            sources: HashMap::new(),
            active_source: None,
            x11_offers: VecDeque::new(),
            host_offers: HashMap::new(),
            active_host_offer: None,
            pending_receives: Vec::new(),
            last_targets_generation: 0,
            prefetched: HashSet::new(),
            host_sends: Vec::new(),
            exported_targets_generation: u64::MAX,
            exported_mimes: Vec::new(),
        }
    }

    /// Attach the shared X11 bridge once `Context::apply_runtime` knows it.
    pub fn set_bridge(&mut self, bridge: Option<Arc<X11Bridge>>) {
        self.bridge = bridge;
    }

    // ----- guest request tracking (client -> host direction) -----

    /// `wl_data_device_manager.get_data_device(id, ..)`.
    pub fn track_data_device(&mut self, id: u32) {
        self.data_devices.insert(id);
    }

    /// `wl_data_device_manager.create_data_source(id)`.
    pub fn track_source(&mut self, id: u32) {
        self.sources.entry(id).or_default();
    }

    /// `wl_data_source.offer(mime_type)`.
    pub fn source_offer(&mut self, sender: u32, mime: &str) {
        self.sources
            .entry(sender)
            .or_default()
            .push(mime.to_string());
    }

    /// `wl_data_device.set_selection(source, serial)`.
    pub fn set_selection(&mut self, ctx: &mut Context, source: u32, _serial: u32) {
        self.active_source = (source != 0).then_some(source);
        let mimes = self
            .active_source
            .and_then(|s| self.sources.get(&s))
            .cloned()
            .unwrap_or_default();
        self.prefetched.clear();
        let Some(bridge) = self.bridge.clone() else {
            return;
        };
        bridge.push_event(WmEvent::ClipboardTargets {
            targets: normalize_targets(&mimes),
        });
        if let Some(source) = self.active_source {
            for mime in &mimes {
                if is_prefetchable(mime) && self.prefetched.insert(format!("source:{mime}")) {
                    prefetch_from_guest_source(
                        ctx,
                        source,
                        mime,
                        &preferred_x11_target(mime),
                        bridge.clone(),
                    );
                }
            }
        }
    }

    /// `wl_data_source.destroy(source)`.
    pub fn source_destroyed(&mut self, sender: u32) {
        self.sources.remove(&sender);
        if self.active_source == Some(sender) {
            self.active_source = None;
        }
    }

    // ----- host event tracking (host -> client direction) -----

    /// A host `wl_data_device.data_offer` was remapped by
    /// `handler/data_device.rs`; remember the guest<->host pair.
    pub fn on_host_data_offer(&mut self, guest_offer: u32, host_offer: u32) {
        self.host_offers.insert(
            guest_offer,
            HostOffer {
                host_id: host_offer,
                mimes: Vec::new(),
            },
        );
    }

    /// A host `wl_data_offer.offer(mime)` event arrived.
    pub fn on_host_offer_mime(&mut self, ctx: &Context, mime: &str) {
        let host_sender = ctx.last_sender_id;
        let Some(guest_offer) = ctx.shadow_table.get_guest_id(host_sender) else {
            return;
        };
        if let Some(offer) = self.host_offers.get_mut(&guest_offer) {
            if !offer.mimes.contains(&mime.to_string()) {
                offer.mimes.push(mime.to_string());
            }
        }
    }

    /// A host `wl_data_device.selection(id)` event arrived. `id` is a host
    /// object id (or 0); the generated forward path rewrites it for the guest,
    /// so this handler only records state and returns `Action::Forward`.
    pub fn on_host_selection(&mut self, ctx: &mut Context, host_offer: u32) {
        // The selection we just exported comes back to our *internal* data
        // device as a host event. Feeding it into the X11 WM would make the
        // WM take ownership of CLIPBOARD from the very X11 app that copied the
        // data (mainline avoids this via its `selection_owner` check).
        if ctx.is_internal_selection_id(ctx.last_sender_id) {
            log::trace!("x11: ignoring selection echo on internal data device");
            return;
        }
        if host_offer == 0 {
            self.active_host_offer = None;
            return;
        }
        let guest_offer = ctx
            .shadow_table
            .get_guest_id(host_offer)
            .unwrap_or(host_offer);
        self.active_host_offer = Some(guest_offer);
        let mimes = self
            .host_offers
            .get(&guest_offer)
            .map(|o| o.mimes.clone())
            .unwrap_or_default();
        self.prefetched.clear();
        let Some(bridge) = self.bridge.clone() else {
            return;
        };
        bridge.push_event(WmEvent::ClipboardTargets {
            targets: normalize_targets(&mimes),
        });
        for mime in &mimes {
            if is_prefetchable(mime) && self.prefetched.insert(format!("host:{mime}")) {
                let host_id = self
                    .host_offers
                    .get(&guest_offer)
                    .map(|o| o.host_id)
                    .unwrap_or(host_offer);
                prefetch_from_host_offer(
                    ctx,
                    host_id,
                    mime,
                    &preferred_x11_target(mime),
                    bridge.clone(),
                );
            }
        }
    }

    // ----- X11 -> host Wayland selection export -----

    /// Translate an X11 TARGETS list into a host `wl_data_source` and install
    /// it as the seat's selection. Equivalent to mainline
    /// `sommelier.cc:sl_get_selection_targets()`.
    fn export_x11_targets_to_host(&mut self, ctx: &mut Context, mimes: &[String], generation: u64) {
        if !ctx.is_xwayland_client || generation == self.exported_targets_generation {
            return;
        }
        self.exported_targets_generation = generation;
        // Mainline's seat->last_serial also starts at 0 (sommelier.cc:563)
        // and is passed through unchanged, so this is not an error state.
        log::debug!("x11: exporting CLIPBOARD with serial {}", ctx.host_serial);
        ensure_host_selection_device(ctx);
        let Some(device) = ctx.host_selection_device_id else {
            return;
        };

        if mimes.is_empty() {
            // Owner disappeared: wl_data_device.set_selection(NULL, serial).
            let mut b = MessageBuilder::new();
            b.write_u32(0);
            b.write_u32(ctx.host_serial);
            ctx.client_to_host_queue.push((
                b.build_message(device, wl_data_device::REQ_SET_SELECTION as u16),
                Vec::new(),
            ));
            self.exported_mimes.clear();
            return;
        }

        let Some(manager) = ctx.host_data_device_manager_id else {
            return;
        };
        let source = ctx.shadow_table.allocate_host_id();
        ctx.shadow_table
            .track_host_interface(source, "wl_data_source".to_string());
        // Retired sources stay in this set: the host may still deliver a late
        // `send`/`cancelled` for a source it was reading when we replaced it.
        ctx.internal_selection_ids.insert(source);
        ctx.host_selection_source_id = Some(source);

        let mut b = MessageBuilder::new();
        b.write_u32(source);
        ctx.client_to_host_queue.push((
            b.build_message(
                manager,
                wl_data_device_manager::REQ_CREATE_DATA_SOURCE as u16,
            ),
            Vec::new(),
        ));
        for mime in mimes {
            let mut ob = MessageBuilder::new();
            ob.write_string(mime);
            ctx.client_to_host_queue.push((
                ob.build_message(source, wl_data_source::REQ_OFFER as u16),
                Vec::new(),
            ));
        }
        let mut sb = MessageBuilder::new();
        sb.write_u32(source);
        sb.write_u32(ctx.host_serial);
        ctx.client_to_host_queue.push((
            sb.build_message(device, wl_data_device::REQ_SET_SELECTION as u16),
            Vec::new(),
        ));
        self.exported_mimes = mimes.to_vec();
        log::debug!(
            "x11: exported {} MIME type(s) to host as wl_data_source id={}",
            mimes.len(),
            source
        );
    }

    /// Host compositor asked our exported source for data. Equivalent to
    /// mainline `sl_internal_data_source_send()` /
    /// `sl_begin_data_source_send()`: run `ConvertSelection` against the X11
    /// CLIPBOARD owner and write the resulting bytes into the supplied fd.
    ///
    /// Returns true when the event was consumed (the caller must not forward
    /// it to any guest).
    pub fn on_host_source_send(&mut self, ctx: &Context, mime: &str, fd: RawFd) -> bool {
        if ctx.host_selection_source_id != Some(ctx.last_sender_id) {
            return false;
        }
        let Some(bridge) = &self.bridge else {
            return false;
        };
        let Ok(owned) = nix::unistd::dup(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }) else {
            log::warn!("x11: dup host data_source.send fd failed");
            return true;
        };
        let target = x11_target_for_mime(mime);
        // Never satisfy this request with bytes cached for a previous owner.
        bridge.invalidate_clipboard_data(&target);
        self.host_sends.push(PendingHostSend {
            target: target.clone(),
            fd: owned,
            deadline: Instant::now() + RECEIVE_TIMEOUT,
        });
        bridge.push_event(WmEvent::RequestClipboardData { target });
        true
    }

    /// Complete host `send` requests once ConvertSelection has delivered the
    /// bytes (or the timeout expires, in which case an empty payload still
    /// closes the fd so the host does not block forever).
    fn poll_host_sends(&mut self) {
        let Some(bridge) = self.bridge.clone() else {
            self.host_sends.clear();
            return;
        };
        let now = Instant::now();
        let mut i = 0;
        while i < self.host_sends.len() {
            let target = self.host_sends[i].target.clone();
            let data = bridge.clipboard_data_for(&target);
            let timed_out = self.host_sends[i].deadline <= now;
            if data.is_some() || timed_out {
                let pending = self.host_sends.remove(i);
                spawn_write(pending.fd, data.unwrap_or_default());
            } else {
                i += 1;
            }
        }
    }

    // ----- X11 -> guest Wayland transfer -----

    /// `wl_data_offer.receive(mime, fd)` on one of our synthetic X11 offers.
    /// Returns `true` when the request was consumed (the caller must Drop it).
    pub fn receive_x11_offer(
        &mut self,
        _ctx: &Context,
        guest_offer: u32,
        mime: &str,
        fd: RawFd,
    ) -> bool {
        let Some((_, offer)) = self.x11_offers.iter().find(|(id, _)| *id == guest_offer) else {
            return false;
        };
        if !offer.targets.iter().any(|t| t == mime) {
            return false;
        }
        let Some(bridge) = &self.bridge else {
            return false;
        };
        // The proxy closes the original SCM_RIGHTS fd after dispatch, so keep
        // our own duplicate for the asynchronous write-back.
        let Ok(owned) = nix::unistd::dup(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }) else {
            return false;
        };
        let target = x11_target_for_mime(mime);
        self.pending_receives.push(PendingReceive {
            target: target.clone(),
            fd: owned,
            deadline: Instant::now() + RECEIVE_TIMEOUT,
        });
        bridge.push_event(WmEvent::RequestClipboardData { target });
        true
    }

    /// Called after every batch of Wayland messages (and on a timer while
    /// idle) to publish X11 selection changes and finish pending transfers.
    pub fn poll(&mut self, ctx: &mut Context) {
        let Some(bridge) = self.bridge.clone() else {
            return;
        };

        // 1. Internal WM notifications (owner changes). Stale pending reads
        //    must not be fulfilled with the previous owner's bytes.
        let mut owner_changed = false;
        while let Some((tag, _payload)) = bridge.take_clipboard_data() {
            if tag.starts_with("x-sommelier/") {
                owner_changed = true;
            }
        }
        if owner_changed {
            self.pending_receives.clear();
            // Pending host `send`s belong to the previous X11 owner.
            self.host_sends.clear();
            self.exported_targets_generation = u64::MAX;
            self.pending_targets_clear();
        }

        // 2. New TARGETS from the X11 owner -> synthetic guest offers and a
        //    host wl_data_source (mainline sl_get_selection_targets).
        let (generation, targets) = bridge.clipboard_targets_snapshot();
        if generation != self.last_targets_generation {
            self.last_targets_generation = generation;
            let targets = targets.unwrap_or_default();
            self.publish_x11_targets(ctx, &targets);
            let mimes = mimes_from_x11_targets(&targets);
            self.export_x11_targets_to_host(ctx, &mimes, generation);
        }

        // 3. Complete receive() calls whose ConvertSelection finished.
        let now = Instant::now();
        let mut i = 0;
        while i < self.pending_receives.len() {
            if self.pending_receives[i].deadline <= now {
                self.pending_receives.remove(i);
                continue;
            }
            let target = self.pending_receives[i].target.clone();
            if let Some(data) = bridge.clipboard_data_for(&target) {
                let pending = self.pending_receives.remove(i);
                spawn_write(pending.fd, data);
            } else {
                i += 1;
            }
        }

        // 4. WM -> proxy: an X11 SelectionRequest needs bytes we do not have.
        while let Some(target) = bridge.take_clipboard_request() {
            self.fetch_for_x11(ctx, &target);
        }

        // 5. host -> X11: finish wl_data_source.send() with ConvertSelection
        //    bytes (or an empty payload on timeout).
        self.poll_host_sends();
    }

    fn pending_targets_clear(&mut self) {
        self.prefetched.clear();
    }

    fn publish_x11_targets(&mut self, ctx: &mut Context, targets: &[String]) {
        let mimes = mimes_from_x11_targets(targets);
        if mimes.is_empty() || self.data_devices.is_empty() {
            return;
        }
        for &device in &self.data_devices {
            let offer_id = ctx.shadow_table.allocate_guest_server_id();
            ctx.shadow_table
                .track_interface(offer_id, "wl_data_offer".to_string());

            let mut b = MessageBuilder::new();
            b.write_u32(offer_id);
            ctx.host_to_client_queue.push((
                b.build_message(device, wl_data_device::EVT_DATA_OFFER as u16),
                Vec::new(),
            ));
            for mime in &mimes {
                let mut ob = MessageBuilder::new();
                ob.write_string(mime);
                ctx.host_to_client_queue.push((
                    ob.build_message(offer_id, wl_data_offer::EVT_OFFER as u16),
                    Vec::new(),
                ));
            }
            let mut sb = MessageBuilder::new();
            sb.write_u32(offer_id);
            ctx.host_to_client_queue.push((
                sb.build_message(device, wl_data_device::EVT_SELECTION as u16),
                Vec::new(),
            ));

            self.x11_offers.push_back((
                offer_id,
                X11Offer {
                    targets: mimes.clone(),
                },
            ));
            while self.x11_offers.len() > MAX_X11_OFFERS {
                self.x11_offers.pop_front();
            }
        }
        log::debug!(
            "x11: published CLIPBOARD offer with {} MIME types to {} guest data devices",
            mimes.len(),
            self.data_devices.len()
        );
    }

    /// Fetch `target` (an X11 atom name) from the active Wayland offer/source
    /// so a deferred X11 `SelectionRequest` can be answered.
    fn fetch_for_x11(&mut self, ctx: &mut Context, target: &str) {
        let Some(bridge) = self.bridge.clone() else {
            return;
        };
        let target_candidates = x11_target_candidates(target);
        let matches = |m: &&String| {
            target_candidates.iter().any(|t| t == *m)
                || x11_target_aliases(m).iter().any(|a| a == target)
        };
        if let Some(guest_offer) = self.active_host_offer {
            let offer = self.host_offers.get(&guest_offer).cloned();
            if let Some(offer) = offer {
                let mime = offer.mimes.iter().find(matches).cloned();
                if let Some(mime) = mime {
                    if self.prefetched.insert(format!("fetch:{target}")) {
                        prefetch_from_host_offer(ctx, offer.host_id, &mime, target, bridge.clone());
                        return;
                    }
                }
            }
        }
        if let Some(source) = self.active_source {
            let mimes = self.sources.get(&source).cloned().unwrap_or_default();
            let mime = mimes.iter().find(matches).cloned();
            if let Some(mime) = mime {
                if self.prefetched.insert(format!("fetch:{target}")) {
                    prefetch_from_guest_source(ctx, source, &mime, target, bridge.clone());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Asynchronous fd plumbing.
// ---------------------------------------------------------------------------

/// Create a pipe the *host compositor* can write into, returning the local
/// read end and a raw fd to hand to the host. virtio_wl needs its own ioctl
/// based pipe; a local/unix compositor can use a plain `pipe(2)`.
fn host_read_pipe(ctx: &Context) -> Option<(OwnedFd, RawFd)> {
    if let Some(virtwl) = &ctx.virtwayland_channel {
        match virtwl.create_pipe(true) {
            Ok(read) => match read.try_clone() {
                Ok(dup) => Some((read, dup.into_raw_fd())),
                Err(e) => {
                    log::warn!("x11: dup virtwl clipboard pipe failed: {}", e);
                    None
                }
            },
            Err(e) => {
                log::warn!("x11: virtwl clipboard pipe failed: {}", e);
                None
            }
        }
    } else {
        match nix::unistd::pipe() {
            Ok((read, write)) => Some((read, write.into_raw_fd())),
            Err(e) => {
                log::warn!("x11: clipboard pipe failed: {}", e);
                None
            }
        }
    }
}

/// Read a pipe to EOF and hand the bytes to the WM as `ClipboardData`.
fn spawn_collect(read: OwnedFd, target: String, bridge: Arc<X11Bridge>) {
    // Unit tests (and any non-tokio caller) have no runtime: read the pipe
    // synchronously rather than panicking inside tokio::spawn.
    if tokio::runtime::Handle::try_current().is_err() {
        use std::io::Read;
        let file = std::fs::File::from(read);
        let mut buf = Vec::new();
        let _ = file.take(PREFETCH_LIMIT as u64 + 1).read_to_end(&mut buf);
        if buf.len() <= PREFETCH_LIMIT {
            bridge.push_event(WmEvent::ClipboardData { target, data: buf });
        }
        return;
    }
    tokio::spawn(async move {
        let mut file = tokio::fs::File::from_std(std::fs::File::from(read));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match file.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() + n > PREFETCH_LIMIT {
                        log::warn!(
                            "x11: clipboard payload for {} exceeded prefetch limit, dropping",
                            target
                        );
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
        bridge.push_event(WmEvent::ClipboardData { target, data: buf });
    });
}

/// Write ConvertSelection bytes into the fd a guest supplied to
/// `wl_data_offer.receive`.
fn spawn_write(fd: OwnedFd, data: Vec<u8>) {
    if tokio::runtime::Handle::try_current().is_err() {
        use std::io::Write as _;
        let mut file = std::fs::File::from(fd);
        let _ = file.write_all(&data);
        return;
    }
    tokio::spawn(async move {
        let mut file = tokio::fs::File::from_std(std::fs::File::from(fd));
        if let Err(e) = file.write_all(&data).await {
            log::warn!("x11: writing clipboard data to guest fd failed: {}", e);
        }
        let _ = file.flush().await;
    });
}

/// Ask a host `wl_data_offer` for `mime`, then publish the result under
/// `reply_target` for X11 requestors.
fn prefetch_from_host_offer(
    ctx: &mut Context,
    host_offer: u32,
    mime: &str,
    reply_target: &str,
    bridge: Arc<X11Bridge>,
) {
    let Some((read, send_fd)) = host_read_pipe(ctx) else {
        return;
    };
    let mut b = MessageBuilder::new();
    b.write_string(mime);
    let msg = b.build_message(host_offer, wl_data_offer::REQ_RECEIVE as u16);
    ctx.client_to_host_queue.push((msg, vec![send_fd]));
    spawn_collect(read, reply_target.to_string(), bridge);
}

/// Ask a guest `wl_data_source` for `mime` by sending the compositor-style
/// `send` event, then publish the result under `reply_target`.
fn prefetch_from_guest_source(
    ctx: &mut Context,
    guest_source: u32,
    mime: &str,
    reply_target: &str,
    bridge: Arc<X11Bridge>,
) {
    let Ok((read, write)) = nix::unistd::pipe() else {
        return;
    };
    let mut b = MessageBuilder::new();
    b.write_string(mime);
    let msg = b.build_message(guest_source, wl_data_source::EVT_SEND as u16);
    ctx.host_to_client_queue
        .push((msg, vec![write.into_raw_fd()]));
    spawn_collect(read, reply_target.to_string(), bridge);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn mime_target_mapping_is_mainline_identity() {
        // sommelier.cc:2819-2841 / :2931-2967 — atom names and MIME strings
        // are passed through verbatim in both directions.
        assert_eq!(
            x11_target_for_mime("text/plain;charset=utf-8"),
            "text/plain;charset=utf-8"
        );
        assert_eq!(x11_target_for_mime("UTF8_STRING"), "UTF8_STRING");
        assert_eq!(x11_target_for_mime("image/png"), "image/png");
        assert_eq!(mime_for_x11_target("UTF8_STRING"), "UTF8_STRING");
        assert_eq!(mime_for_x11_target("TARGETS"), "");
        assert_eq!(mime_for_x11_target("_WL_SELECTION"), "");
        assert_eq!(mime_for_x11_target("image/png"), "image/png");
        // Prefetching still prefers the widely served UTF8_STRING atom.
        assert_eq!(
            preferred_x11_target("text/plain;charset=utf-8"),
            "UTF8_STRING"
        );
        assert_eq!(preferred_x11_target("image/png"), "image/png");
        assert!(
            x11_target_candidates("UTF8_STRING").contains(&"text/plain;charset=utf-8".to_string())
        );
    }

    #[test]
    fn normalize_targets_starts_with_mainline_control_atoms() {
        let t = normalize_targets(&[
            "text/plain;charset=utf-8".into(),
            "text/plain;charset=utf-8".into(),
            "image/png".into(),
        ]);
        // Mainline sl_set_selection inserts TARGETS and TIMESTAMP first.
        assert_eq!(t[0], "TARGETS");
        assert_eq!(t[1], "TIMESTAMP");
        assert_eq!(t[2], "text/plain;charset=utf-8");
        assert!(t.contains(&"UTF8_STRING".to_string()));
        assert!(t.contains(&"image/png".to_string()));
        assert_eq!(
            t.iter().filter(|x| *x == "image/png").count(),
            1,
            "targets must be deduplicated"
        );
    }

    #[test]
    fn only_small_textual_payloads_are_prefetched() {
        assert!(is_prefetchable("text/plain;charset=utf-8"));
        assert!(is_prefetchable("UTF8_STRING"));
        assert!(is_prefetchable("image/png"));
        assert!(!is_prefetchable("application/octet-stream"));
        assert!(!is_prefetchable("video/mp4"));
    }

    #[test]
    fn synthetic_offer_is_emitted_for_every_data_device() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.selection.lock().unwrap().set_bridge(Some(bridge));
        let mut sel = std::mem::take(&mut *ctx.selection.lock().unwrap());
        sel.track_data_device(7);
        sel.track_data_device(9);
        bridge_set_targets(&sel, &["UTF8_STRING".into(), "image/png".into()]);
        sel.poll(&mut ctx);

        // 2 devices * (data_offer + 2 offers + selection) = 8 events.
        assert_eq!(ctx.host_to_client_queue.len(), 8);
        let first = &ctx.host_to_client_queue[0].0;
        let sender = u32::from_ne_bytes(first[0..4].try_into().unwrap());
        assert!(sender == 7 || sender == 9);
        let opcode = (u32::from_ne_bytes(first[4..8].try_into().unwrap()) & 0xffff) as u16;
        assert_eq!(opcode, wl_data_device::EVT_DATA_OFFER as u16);
        *ctx.selection.lock().unwrap() = sel;
    }

    #[test]
    fn receive_on_synthetic_offer_requests_convert_selection() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.selection
            .lock()
            .unwrap()
            .set_bridge(Some(bridge.clone()));
        let mut sel = std::mem::take(&mut *ctx.selection.lock().unwrap());
        sel.track_data_device(7);
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into()]);
        sel.poll(&mut ctx);

        let offer_id = u32::from_ne_bytes(ctx.host_to_client_queue[0].0[8..12].try_into().unwrap());
        let (read, write) = nix::unistd::pipe().unwrap();
        // 1:1 MIME passthrough: the guest sees the X11 atom name verbatim.
        assert!(sel.receive_x11_offer(&ctx, offer_id, "UTF8_STRING", write.as_raw_fd()));
        assert_eq!(sel.pending_receives.len(), 1);
        match bridge.try_recv_event() {
            Some(WmEvent::RequestClipboardData { target }) => {
                assert_eq!(target, "UTF8_STRING")
            }
            other => panic!("unexpected WmEvent: {:?}", other),
        }
        drop(read);
        *ctx.selection.lock().unwrap() = sel;
    }

    #[test]
    fn pending_receive_is_fulfilled_from_convert_selection_cache() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = Context::new_for_test(false, false, vec![]);
        let mut sel = SelectionBridge::new(Some(bridge.clone()));
        sel.track_data_device(7);
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into()]);
        sel.poll(&mut ctx);
        let offer_id = u32::from_ne_bytes(ctx.host_to_client_queue[0].0[8..12].try_into().unwrap());
        let (read, write) = nix::unistd::pipe().unwrap();
        assert!(sel.receive_x11_offer(&ctx, offer_id, "UTF8_STRING", write.as_raw_fd()));
        bridge.set_clipboard_data("UTF8_STRING".into(), b"hello".to_vec());
        sel.poll(&mut ctx);
        assert!(sel.pending_receives.is_empty());
        drop(read);
    }

    /// Extract (sender, opcode, payload) from a built Wayland message.
    fn decode(msg: &[u8]) -> (u32, u16, &[u8]) {
        let sender = u32::from_ne_bytes(msg[0..4].try_into().unwrap());
        let word2 = u32::from_ne_bytes(msg[4..8].try_into().unwrap());
        (sender, (word2 & 0xffff) as u16, &msg[8..])
    }

    fn export_ready_ctx(bridge: Arc<X11Bridge>) -> Context {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.is_xwayland_client = true;
        ctx.host_data_device_manager_id = Some(10);
        ctx.host_seat_id = Some(11);
        ctx.host_serial = 42;
        ctx.selection.lock().unwrap().set_bridge(Some(bridge));
        ctx
    }

    #[test]
    fn x11_targets_are_exported_as_a_host_data_source() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        let mut sel = SelectionBridge::new(Some(bridge.clone()));
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into(), "TARGETS".into()]);

        sel.poll(&mut ctx);

        let q = &ctx.client_to_host_queue;
        // get_data_device, create_data_source, one offer, set_selection.
        assert_eq!(q.len(), 4, "unexpected export messages: {:?}", q);
        let (sender, op, payload) = decode(&q[0].0);
        assert_eq!(sender, 10);
        assert_eq!(op, wl_data_device_manager::REQ_GET_DATA_DEVICE as u16);
        let device = u32::from_ne_bytes(payload[0..4].try_into().unwrap());
        let seat = u32::from_ne_bytes(payload[4..8].try_into().unwrap());
        assert_eq!(seat, 11);
        assert_eq!(ctx.host_selection_device_id, Some(device));

        let (sender, op, payload) = decode(&q[1].0);
        assert_eq!(sender, 10);
        assert_eq!(op, wl_data_device_manager::REQ_CREATE_DATA_SOURCE as u16);
        let source = u32::from_ne_bytes(payload[0..4].try_into().unwrap());
        assert_eq!(ctx.host_selection_source_id, Some(source));
        assert!(ctx.is_internal_selection_id(source));
        assert!(ctx.is_internal_selection_id(device));

        let (sender, op, payload) = decode(&q[2].0);
        assert_eq!(sender, source);
        assert_eq!(op, wl_data_source::REQ_OFFER as u16);
        let mime_len = u32::from_ne_bytes(payload[0..4].try_into().unwrap()) as usize;
        // Mainline sl_get_selection_targets offers the atom name verbatim.
        assert_eq!(&payload[4..4 + mime_len - 1], b"UTF8_STRING");

        let (sender, op, payload) = decode(&q[3].0);
        assert_eq!(sender, device);
        assert_eq!(op, wl_data_device::REQ_SET_SELECTION as u16);
        assert_eq!(
            u32::from_ne_bytes(payload[0..4].try_into().unwrap()),
            source
        );
        assert_eq!(u32::from_ne_bytes(payload[4..8].try_into().unwrap()), 42);
    }

    #[test]
    fn export_only_happens_on_the_xwayland_connection() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        ctx.is_xwayland_client = false;
        let mut sel = SelectionBridge::new(Some(bridge.clone()));
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into()]);

        sel.poll(&mut ctx);

        assert!(ctx.client_to_host_queue.is_empty());
        assert!(ctx.host_selection_source_id.is_none());
    }

    #[test]
    fn empty_targets_clear_the_host_selection() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        let mut sel = SelectionBridge::new(Some(bridge.clone()));
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into()]);
        sel.poll(&mut ctx);
        assert!(ctx.host_selection_source_id.is_some());

        bridge.set_clipboard_targets(Vec::new());
        sel.poll(&mut ctx);

        let last = ctx.client_to_host_queue.last().unwrap();
        let (_, op, payload) = decode(&last.0);
        assert_eq!(op, wl_data_device::REQ_SET_SELECTION as u16);
        assert_eq!(u32::from_ne_bytes(payload[0..4].try_into().unwrap()), 0);
    }

    #[test]
    fn host_source_send_converts_x11_selection_and_writes_fd() {
        use std::io::Read;
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        ctx.host_selection_source_id = Some(99);
        ctx.last_sender_id = 99;
        let mut sel = SelectionBridge::new(Some(bridge.clone()));

        let (read, write) = nix::unistd::pipe().unwrap();
        assert!(sel.on_host_source_send(&ctx, "text/plain;charset=utf-8", write.as_raw_fd()));
        drop(write);
        match bridge.try_recv_event() {
            Some(WmEvent::RequestClipboardData { target }) => {
                // 1:1: the MIME the host asked for is the X11 target we request.
                assert_eq!(target, "text/plain;charset=utf-8")
            }
            other => panic!("unexpected WmEvent: {:?}", other),
        }

        bridge.set_clipboard_data("text/plain;charset=utf-8".into(), b"from-x11".to_vec());
        sel.poll(&mut ctx);
        let mut buf = Vec::new();
        let mut file = std::fs::File::from(read);
        file.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"from-x11");
        assert!(sel.host_sends.is_empty());
    }

    #[test]
    fn send_from_an_unknown_source_is_not_consumed() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        ctx.host_selection_source_id = Some(99);
        ctx.last_sender_id = 1234;
        let mut sel = SelectionBridge::new(Some(bridge));
        let (_read, write) = nix::unistd::pipe().unwrap();
        assert!(!sel.on_host_source_send(&ctx, "text/plain", write.as_raw_fd()));
        assert!(sel.host_sends.is_empty());
    }

    #[test]
    fn internal_device_selection_echo_is_not_published_to_x11() {
        let (bridge, _rx) = X11Bridge::new();
        let mut ctx = export_ready_ctx(bridge.clone());
        ctx.host_selection_device_id = Some(77);
        ctx.last_sender_id = 77;
        let mut sel = SelectionBridge::new(Some(bridge.clone()));

        sel.on_host_selection(&mut ctx, 5);

        assert!(bridge.try_recv_event().is_none());
    }

    #[test]
    fn control_atoms_never_become_exported_mimes() {
        // Mainline exports atom names verbatim; only control atoms are dropped.
        assert_eq!(
            mimes_from_x11_targets(&[
                "TARGETS".into(),
                "TIMESTAMP".into(),
                "UTF8_STRING".into(),
                "STRING".into(),
            ]),
            vec!["UTF8_STRING".to_string(), "STRING".to_string()]
        );
    }

    /// Helper: publish targets through the bridge held by `sel`.
    fn bridge_set_targets(sel: &SelectionBridge, targets: &[String]) {
        if let Some(b) = &sel.bridge {
            b.set_clipboard_targets(targets.to_vec());
        }
    }
}
