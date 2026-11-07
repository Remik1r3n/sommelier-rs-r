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

//! Shared state between the X11 window-manager thread and the Wayland proxy.
//!
//! This is the Rust equivalent of the bookkeeping that mainline sommelier keeps
//! in `struct sl_context` / `struct sl_window`: it correlates X11 XIDs with the
//! `wl_surface` object ids that Xwayland reports through `WL_SURFACE_ID`, and
//! provides the two message channels used to drive the host compositor.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Everything the host compositor needs to know about a managed X11 window.
///
/// Mirrors the subset of `struct sl_window` that `sl_handle_map_request()`
/// extracts from the ICCCM/EWMH properties.
#[derive(Debug, Clone, Default)]
pub struct X11WindowInfo {
    pub xid: u32,
    /// Xwayland-side `wl_surface` object id, taken from `WL_SURFACE_ID`.
    pub surface_guest_id: u32,
    pub title: String,
    pub app_id: String,
    pub wm_class: String,
    pub min_size: (i32, i32),
    pub max_size: (i32, i32),
    pub fullscreen: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub decorated: bool,
    pub dark_theme: bool,
    pub urgent: bool,
    pub steam_game_id: u32,
    pub window_type: u32,
    pub pid: u32,
    pub transient_for: u32,
}

/// Commands sent from the WM thread to the Wayland proxy.
///
/// `CreateToplevel` corresponds to sommelier creating an `xdg_surface` /
/// `xdg_toplevel` / `zaura_surface` for a newly paired X11 window.
#[derive(Debug, Clone)]
pub enum WmCommand {
    CreateToplevel(X11WindowInfo),
    DestroyToplevel { xid: u32 },
    UpdateToplevel { xid: u32, info: X11WindowInfo },
    Activate { xid: u32 },
    MoveResize { xid: u32, edges: u32 },
    Commit { xid: u32 },
}

/// Events sent from the Wayland proxy to the WM thread.
///
/// The clipboard variants carry guest `wl_data_source` offers and X11
/// selection payloads between `x11/selection.rs` and the WM loop.
#[derive(Debug, Clone)]
pub enum WmEvent {
    Configure {
        xid: u32,
        width: i32,
        height: i32,
        activated: bool,
        fullscreen: bool,
        maximized: bool,
    },
    Close {
        xid: u32,
    },
    ClipboardTargets {
        targets: Vec<String>,
    },
    ClipboardData {
        target: String,
        data: Vec<u8>,
    },
    /// Ask the WM to run `ConvertSelection(CLIPBOARD, target)` against the
    /// current X11 owner. The resulting bytes come back through
    /// [`X11Bridge::set_clipboard_data`].
    RequestClipboardData {
        target: String,
    },
}

/// Channel/state hub shared by the proxy and the dedicated WM OS thread.
///
/// The command receiver is handed to the proxy; the event receiver stays
/// inside the bridge and is drained by [`X11Bridge::try_recv_event`] from the
/// WM thread's poll loop. All maps are mutex-protected because the two sides
/// run on different threads.
pub struct X11Bridge {
    cmd_tx: UnboundedSender<WmCommand>,
    ev_rx: Mutex<UnboundedReceiver<WmEvent>>,
    ev_tx: UnboundedSender<WmEvent>,

    guest_to_host: Mutex<HashMap<u32, u32>>,
    host_to_guest: Mutex<HashMap<u32, u32>>,

    xid_to_host: Mutex<HashMap<u32, u32>>,
    host_to_xid: Mutex<HashMap<u32, u32>>,

    host_globals: Mutex<(Option<u32>, Option<u32>)>,
    display: Mutex<Option<String>>,
    screen_size: Mutex<(i32, i32)>,
    /// Host output rectangles as a flat `[x, y, w, h, ...]` list in virtual
    /// coordinates, plus a generation counter. The WM writes these to
    /// `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` when RandR emulation is enabled.
    output_rects: Mutex<(u64, Vec<i32>)>,

    // X11 -> Wayland clipboard notifications. The proxy polls these because
    // `WmCommand` deliberately has no clipboard variant (see clipboard.rs).
    clipboard_targets: Mutex<Option<Vec<String>>>,
    /// Bumped on every owner/TARGETS change. Each guest connection keeps its
    /// own `SelectionBridge` and must all see the same update, so the proxy
    /// polls a (generation, snapshot) pair instead of `take()`-ing a value
    /// that only one connection would win.
    clipboard_targets_generation: AtomicU64,
    clipboard_data: Mutex<VecDeque<(String, Vec<u8>)>>,
    /// Last payload per target. Request-driven transfers are asynchronous:
    /// the request is issued on `receive`, the bytes may arrive many poll
    /// cycles later, and several clients may ask for the same target.
    clipboard_data_cache: Mutex<HashMap<String, Vec<u8>>>,
    /// X11 -> proxy data requests raised by a `SelectionRequest` that the WM
    /// cannot answer from its cache. The proxy fetches the bytes from the
    /// active Wayland offer/source.
    clipboard_requests: Mutex<VecDeque<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned mutex here would otherwise wedge the WM loop forever. The
    // contained maps/channels are still usable, so recover the guard.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// Reverse-lookup and runtime-state getters are currently read only by the
// bridge unit tests, while their setters are on the production path. Keep both
// sides of the state machine together instead of splitting them by cfg(test).
#[allow(dead_code)]
impl X11Bridge {
    /// Create the bridge and return the command receiver owned by the proxy.
    ///
    /// The WM thread never receives a `WmEvent` receiver directly; it calls
    /// [`X11Bridge::try_recv_event`] instead.
    pub fn new() -> (Arc<Self>, UnboundedReceiver<WmCommand>) {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = Arc::new(Self {
            cmd_tx,
            ev_rx: Mutex::new(ev_rx),
            ev_tx,
            guest_to_host: Mutex::new(HashMap::new()),
            host_to_guest: Mutex::new(HashMap::new()),
            xid_to_host: Mutex::new(HashMap::new()),
            host_to_xid: Mutex::new(HashMap::new()),
            host_globals: Mutex::new((None, None)),
            display: Mutex::new(None),
            screen_size: Mutex::new((0, 0)),
            output_rects: Mutex::new((0, Vec::new())),
            clipboard_targets: Mutex::new(None),
            clipboard_targets_generation: AtomicU64::new(0),
            clipboard_data: Mutex::new(VecDeque::new()),
            clipboard_data_cache: Mutex::new(HashMap::new()),
            clipboard_requests: Mutex::new(VecDeque::new()),
        });
        (bridge, cmd_rx)
    }

    // ----- proxy side: Wayland object id correlation -----

    /// Register `guest_id` (Xwayland's `wl_surface` object id) -> `host_id`.
    pub fn publish_surface(&self, guest_id: u32, host_id: u32) {
        lock(&self.guest_to_host).insert(guest_id, host_id);
        lock(&self.host_to_guest).insert(host_id, guest_id);
    }

    pub fn host_surface(&self, guest_id: u32) -> Option<u32> {
        lock(&self.guest_to_host).get(&guest_id).copied()
    }

    pub fn guest_surface(&self, host_id: u32) -> Option<u32> {
        lock(&self.host_to_guest).get(&host_id).copied()
    }

    /// Register the host globals the proxy bound internally.
    pub fn set_host_globals(&self, xdg_wm_base: Option<u32>, aura_shell: Option<u32>) {
        *lock(&self.host_globals) = (xdg_wm_base, aura_shell);
    }

    pub fn host_globals(&self) -> (Option<u32>, Option<u32>) {
        *lock(&self.host_globals)
    }

    // ----- WM thread side -----

    pub fn send_command(&self, cmd: WmCommand) {
        if let Err(e) = self.cmd_tx.send(cmd) {
            log::warn!("x11: dropping WmCommand, proxy disconnected: {}", e);
        }
    }

    /// Non-blocking drain of one proxy event, used by the WM poll loop.
    pub fn try_recv_event(&self) -> Option<WmEvent> {
        lock(&self.ev_rx).try_recv().ok()
    }

    // ----- proxy side: events into the WM thread -----

    pub fn push_event(&self, ev: WmEvent) {
        if let Err(e) = self.ev_tx.send(ev) {
            log::warn!("x11: dropping WmEvent, WM thread gone: {}", e);
        }
    }

    // ----- xid <-> host wl_surface binding -----

    pub fn register_window(&self, xid: u32, host_surface_id: u32) {
        lock(&self.xid_to_host).insert(xid, host_surface_id);
        lock(&self.host_to_xid).insert(host_surface_id, xid);
    }

    pub fn unregister_window(&self, xid: u32) {
        if let Some(host) = lock(&self.xid_to_host).remove(&xid) {
            lock(&self.host_to_xid).remove(&host);
        }
    }

    pub fn xid_for_host_surface(&self, host_id: u32) -> Option<u32> {
        lock(&self.host_to_xid).get(&host_id).copied()
    }

    pub fn host_surface_for_xid(&self, xid: u32) -> Option<u32> {
        lock(&self.xid_to_host).get(&xid).copied()
    }

    // ----- misc runtime state -----

    pub fn set_display(&self, s: String) {
        *lock(&self.display) = Some(s);
    }

    pub fn display(&self) -> Option<String> {
        lock(&self.display).clone()
    }

    pub fn set_screen_size(&self, w: i32, h: i32) {
        *lock(&self.screen_size) = (w, h);
    }

    pub fn screen_size(&self) -> (i32, i32) {
        *lock(&self.screen_size)
    }

    /// Publish the current output layout. `rects` are `(x, y, width, height)`
    /// tuples in the guest's virtual coordinate system.
    pub fn set_output_rects(&self, rects: &[(i32, i32, i32, i32)]) {
        let flat: Vec<i32> = rects
            .iter()
            .flat_map(|&(x, y, w, h)| [x, y, w, h])
            .collect();
        let mut slot = lock(&self.output_rects);
        if slot.1 == flat {
            return; // unchanged: do not make the WM rewrite the property
        }
        let generation = slot.0.wrapping_add(1);
        *slot = (generation, flat);
    }

    /// Non-destructive read for the WM thread.
    pub fn output_rects_snapshot(&self) -> (u64, Vec<i32>) {
        lock(&self.output_rects).clone()
    }

    // ----- clipboard helpers (X11 -> Wayland direction) -----
    //
    // The WM thread publishes CLIPBOARD/TARGETS results here. The proxy reads
    // them non-destructively via `clipboard_targets_snapshot()` so every guest
    // connection can build its own `wl_data_offer`.

    /// Publish the TARGETS list obtained from an X11 CLIPBOARD owner.
    /// Every publish is a new generation so each guest connection can
    /// synthesize its own `wl_data_offer` exactly once.
    pub fn set_clipboard_targets(&self, targets: Vec<String>) {
        *lock(&self.clipboard_targets) = Some(targets);
        // The owner changed: payloads cached for the previous owner must not
        // satisfy a `receive` issued after the new TARGETS arrived.
        lock(&self.clipboard_data_cache).clear();
        self.clipboard_targets_generation
            .fetch_add(1, Ordering::Release);
    }

    /// Non-destructive TARGETS snapshot with its generation counter.
    pub fn clipboard_targets_snapshot(&self) -> (u64, Option<Vec<String>>) {
        let gen = self.clipboard_targets_generation.load(Ordering::Acquire);
        (gen, lock(&self.clipboard_targets).clone())
    }

    /// Consume the published TARGETS list (one-shot read for unit tests).
    #[cfg(test)]
    pub fn take_clipboard_targets(&self) -> Option<Vec<String>> {
        lock(&self.clipboard_targets).take()
    }

    /// Publish a converted selection payload for the Wayland side.
    pub fn set_clipboard_data(&self, target: String, data: Vec<u8>) {
        if target.starts_with("x-sommelier/") {
            // Internal WM notification, e.g. CLIPBOARD owner changed. Those
            // are one-shot control messages consumed by the proxy's poll.
            lock(&self.clipboard_data).push_back((target, data));
        } else {
            // ConvertSelection result: keep the latest payload per target so
            // every guest connection can read it (a queue would let exactly
            // one connection win the race).
            lock(&self.clipboard_data_cache).insert(target, data);
        }
    }

    /// Most recent payload for `target`, without consuming it.
    pub fn clipboard_data_for(&self, target: &str) -> Option<Vec<u8>> {
        lock(&self.clipboard_data_cache).get(target).cloned()
    }

    /// Drop the cached payload for a single target before starting a new
    /// `ConvertSelection`. Without this a host `wl_data_source.send` could be
    /// answered with bytes belonging to an earlier X11 CLIPBOARD owner.
    pub fn invalidate_clipboard_data(&self, target: &str) {
        lock(&self.clipboard_data_cache).remove(target);
    }

    /// Consume the oldest queued selection payload.
    pub fn take_clipboard_data(&self) -> Option<(String, Vec<u8>)> {
        lock(&self.clipboard_data).pop_front()
    }

    /// WM -> proxy: an X11 `SelectionRequest` needs bytes for `target`.
    pub fn request_clipboard_data(&self, target: String) {
        let mut q = lock(&self.clipboard_requests);
        // Coalesce duplicate requests while one is still in flight.
        if !q.contains(&target) {
            q.push_back(target);
        }
    }

    /// Proxy -> WM: consume one pending X11 data request.
    pub fn take_clipboard_request(&self) -> Option<String> {
        lock(&self.clipboard_requests).pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_window_info_default_is_empty() {
        let i = X11WindowInfo::default();
        assert_eq!(i.xid, 0);
        assert_eq!(i.surface_guest_id, 0);
        assert!(i.title.is_empty());
        assert!(i.app_id.is_empty());
        assert!(i.wm_class.is_empty());
        assert_eq!(i.min_size, (0, 0));
        assert_eq!(i.max_size, (0, 0));
        assert!(!i.fullscreen);
        assert!(!i.maximized);
        assert!(!i.minimized);
        assert!(!i.decorated);
        assert!(!i.dark_theme);
        assert!(!i.urgent);
        assert_eq!(i.steam_game_id, 0);
        assert_eq!(i.window_type, 0);
        assert_eq!(i.pid, 0);
        assert_eq!(i.transient_for, 0);
    }

    #[test]
    fn surface_mapping_is_bidirectional() {
        let (bridge, _rx) = X11Bridge::new();
        assert_eq!(bridge.host_surface(7), None);
        assert_eq!(bridge.guest_surface(99), None);

        bridge.publish_surface(7, 99);
        assert_eq!(bridge.host_surface(7), Some(99));
        assert_eq!(bridge.guest_surface(99), Some(7));
        assert_eq!(bridge.guest_surface(100), None);
    }

    #[test]
    fn window_registration_roundtrip() {
        let (bridge, _rx) = X11Bridge::new();
        bridge.register_window(0x0040_0001, 42);
        assert_eq!(bridge.host_surface_for_xid(0x0040_0001), Some(42));
        assert_eq!(bridge.xid_for_host_surface(42), Some(0x0040_0001));

        bridge.unregister_window(0x0040_0001);
        assert_eq!(bridge.host_surface_for_xid(0x0040_0001), None);
        assert_eq!(bridge.xid_for_host_surface(42), None);
    }

    #[test]
    fn event_push_and_try_recv() {
        let (bridge, _rx) = X11Bridge::new();
        assert!(bridge.try_recv_event().is_none());
        bridge.push_event(WmEvent::Close { xid: 12 });
        match bridge.try_recv_event() {
            Some(WmEvent::Close { xid }) => assert_eq!(xid, 12),
            other => panic!("unexpected event: {:?}", other),
        }
        assert!(bridge.try_recv_event().is_none());
    }

    #[test]
    fn command_channel_delivers_to_proxy() {
        let (bridge, mut rx) = X11Bridge::new();
        bridge.send_command(WmCommand::Activate { xid: 5 });
        match rx.try_recv() {
            Ok(WmCommand::Activate { xid }) => assert_eq!(xid, 5),
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn globals_display_and_screen_size() {
        let (bridge, _rx) = X11Bridge::new();
        assert_eq!(bridge.host_globals(), (None, None));
        bridge.set_host_globals(Some(3), None);
        assert_eq!(bridge.host_globals(), (Some(3), None));

        assert_eq!(bridge.display(), None);
        bridge.set_display(":3".to_string());
        assert_eq!(bridge.display().as_deref(), Some(":3"));

        bridge.set_screen_size(1920, 1080);
        assert_eq!(bridge.screen_size(), (1920, 1080));
    }

    #[test]
    fn output_rects_are_flattened_and_deduplicated() {
        let (bridge, _rx) = X11Bridge::new();
        assert_eq!(bridge.output_rects_snapshot(), (0, vec![]));
        bridge.set_output_rects(&[(0, 0, 1920, 1080), (1920, 0, 1280, 720)]);
        let (gen, rects) = bridge.output_rects_snapshot();
        assert_eq!(gen, 1);
        assert_eq!(rects, vec![0, 0, 1920, 1080, 1920, 0, 1280, 720]);
        // Re-publishing the same layout must not make the WM rewrite the
        // X11 property on every wl_output event.
        bridge.set_output_rects(&[(0, 0, 1920, 1080), (1920, 0, 1280, 720)]);
        assert_eq!(bridge.output_rects_snapshot().0, 1);
        bridge.set_output_rects(&[(0, 0, 2560, 1440)]);
        let (gen, rects) = bridge.output_rects_snapshot();
        assert_eq!(gen, 2);
        assert_eq!(rects, vec![0, 0, 2560, 1440]);
    }

    #[test]
    fn clipboard_helpers_take_once() {
        let (bridge, _rx) = X11Bridge::new();
        assert_eq!(bridge.take_clipboard_targets(), None);
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into(), "STRING".into()]);
        assert_eq!(
            bridge.take_clipboard_targets(),
            Some(vec!["UTF8_STRING".to_string(), "STRING".to_string()])
        );
        assert_eq!(bridge.take_clipboard_targets(), None);

        // ConvertSelection results are cached per target, not queued: every
        // guest connection must be able to read the same payload.
        bridge.set_clipboard_data("text/plain".into(), b"hello".to_vec());
        assert_eq!(
            bridge.clipboard_data_for("text/plain"),
            Some(b"hello".to_vec())
        );
        assert_eq!(bridge.take_clipboard_data(), None);

        // Internal WM control messages remain one-shot queue entries.
        bridge.set_clipboard_data("x-sommelier/clipboard-owner-changed".into(), vec![1]);
        assert_eq!(
            bridge.take_clipboard_data(),
            Some(("x-sommelier/clipboard-owner-changed".to_string(), vec![1]))
        );
    }

    #[test]
    fn invalidate_clipboard_data_drops_only_one_target() {
        let (bridge, _rx) = X11Bridge::new();
        bridge.set_clipboard_data("UTF8_STRING".into(), b"old".to_vec());
        bridge.set_clipboard_data("image/png".into(), b"png".to_vec());
        bridge.invalidate_clipboard_data("UTF8_STRING");
        assert_eq!(bridge.clipboard_data_for("UTF8_STRING"), None);
        assert_eq!(
            bridge.clipboard_data_for("image/png"),
            Some(b"png".to_vec())
        );
        bridge.set_clipboard_data("UTF8_STRING".into(), b"new".to_vec());
        assert_eq!(
            bridge.clipboard_data_for("UTF8_STRING"),
            Some(b"new".to_vec())
        );
    }

    #[test]
    fn new_targets_clear_stale_payload_cache() {
        let (bridge, _rx) = X11Bridge::new();
        bridge.set_clipboard_data("UTF8_STRING".into(), b"old-owner".to_vec());
        bridge.set_clipboard_targets(vec!["UTF8_STRING".into()]);
        // A fresh TARGETS list means a new X11 CLIPBOARD owner; bytes cached
        // for the previous one must never answer a new receive/send.
        assert_eq!(bridge.clipboard_data_for("UTF8_STRING"), None);
    }
}
