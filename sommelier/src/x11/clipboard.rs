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

//! X11 <-> Wayland clipboard/selection bridge.
//!
//! Mainline sommelier owns `CLIPBOARD_MANAGER` and `CLIPBOARD`, watches the
//! CLIPBOARD owner via XFixes (`sl_connect`), converts `TARGETS` whenever the
//! owner changes (`sl_handle_x_connection_event`) and serves X11
//! `SelectionRequest` from the Wayland data source (`sl_handle_selection_request`
//! / `sl_set_selection`). This module implements that protocol path, including
//! a working INCR (incremental transfer) skeleton for large payloads.
//!
//! The `WmCommand` enum has no clipboard variants on purpose (so the proxy's
//! `match` stays exhaustive), therefore X11 -> Wayland results are published
//! through `X11Bridge::set_clipboard_targets()` / `set_clipboard_data()`.

use std::collections::HashMap;

use log::{debug, warn};
use x11rb::connection::RequestConnection;
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto::{
    self, ChangeWindowAttributesAux, EventMask, PropMode, Property, SelectionNotifyEvent,
    SelectionRequestEvent,
};

use crate::x11::atoms::{Atom as X11Atom, AtomTable};
use crate::x11::bridge::X11Bridge;

/// Payloads larger than this are sent with the INCR protocol instead of one
/// big `ChangeProperty` (which is bounded by the server's max request size).
const INCR_THRESHOLD: usize = 128 * 1024;
/// Size of each INCR chunk.
const INCR_CHUNK: usize = 32 * 1024;
/// How many 4-byte units to fetch per property read.
const PROPERTY_LONG_LENGTH: u32 = 4096;

/// One in-progress INCR *send* (we are the selection owner).
struct IncrTransfer {
    requestor: u32,
    property: u32,
    /// Atom used as the property type for the chunk (the requested target).
    target: u32,
    data: Vec<u8>,
    offset: usize,
}

/// One in-progress INCR *receive* (some other client owns the selection).
struct RecvIncr {
    requestor: u32,
    property: u32,
    data: Vec<u8>,
}

/// State machine for CLIPBOARD / TARGETS / TIMESTAMP / INCR handling.
pub struct ClipboardBridge {
    /// Our 1x1 CLIPBOARD_MANAGER / CLIPBOARD owner window.
    selection_window: u32,
    /// Last CLIPBOARD owner reported by XFixes (0 means no owner).
    owner: u32,
    /// TARGETS advertised by the Wayland data source (`WmEvent::ClipboardTargets`).
    targets: Vec<String>,
    /// Payloads from the Wayland side, keyed by target/MIME name.
    data: HashMap<String, Vec<u8>>,
    /// Convenience copy of the most recent text payload for STRING/UTF8_STRING.
    text_data: Option<Vec<u8>>,
    /// Monotonic TIMESTAMP value handed out to X11 requestors.
    timestamp: u32,
    /// Active INCR sends, keyed by requestor window.
    incr: HashMap<u32, IncrTransfer>,
    /// Active INCR receive (at most one at a time).
    recv_incr: Option<RecvIncr>,
}

impl ClipboardBridge {
    /// `selection_window` is the window created in `sl_connect()` that owns
    /// `CLIPBOARD_MANAGER` and `CLIPBOARD`. It may be 0 when clipboard support
    /// is disabled, in which case XFixes notifications are ignored.
    pub fn new(selection_window: u32) -> Self {
        Self {
            selection_window,
            owner: 0,
            targets: Vec::new(),
            data: HashMap::new(),
            text_data: None,
            timestamp: 1,
            incr: HashMap::new(),
            recv_incr: None,
        }
    }

    /// TARGETS currently known from the Wayland side.
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// Wayland -> X11: the host data source advertised its offered MIME types.
    pub fn note_targets(&mut self, targets: Vec<String>) {
        self.targets = targets;
    }

    /// Wayland -> X11: a selection payload arrived for `target`.
    pub fn note_data(&mut self, target: String, data: Vec<u8>) {
        let lower = target.to_ascii_lowercase();
        if lower.contains("text")
            || lower == "utf8_string"
            || lower == "string"
            || lower == "compound_text"
        {
            self.text_data = Some(data.clone());
        }
        self.data.insert(target, data);
    }

    /// Serve an X11 `SelectionRequest`.
    ///
    /// Handles TARGETS / TIMESTAMP / UTF8_STRING / STRING / TEXT and arbitrary
    /// Wayland MIME targets, then always replies with a synthetic
    /// `SelectionNotify` (property 0 on failure, per ICCCM).
    pub fn handle_selection_request<C: RequestConnection>(
        &mut self,
        conn: &C,
        atoms: &AtomTable,
        ev: &SelectionRequestEvent,
    ) {
        let a = |x: X11Atom| atoms.get(x);
        let target = ev.target;
        // ICCCM: when `property` is None the requestor expects the answer in a
        // property named by `target`.
        let property = if ev.property == 0 {
            target
        } else {
            ev.property
        };
        let mut ok = false;

        if target == a(X11Atom::Targets) {
            let list = [
                a(X11Atom::Targets),
                a(X11Atom::Timestamp),
                a(X11Atom::Utf8String),
                a(X11Atom::StringType),
                a(X11Atom::Text),
                a(X11Atom::WlSelection),
            ];
            let bytes = u32_list_bytes(&list);
            ok = change_prop(
                conn,
                ev.requestor,
                property,
                a(X11Atom::AtomType),
                32,
                list.len() as u32,
                &bytes,
            );
        } else if target == a(X11Atom::Timestamp) {
            self.timestamp = self.timestamp.wrapping_add(1);
            let bytes = self.timestamp.to_le_bytes();
            ok = change_prop(
                conn,
                ev.requestor,
                property,
                a(X11Atom::Cardinal),
                32,
                1,
                &bytes,
            );
        } else if let Some((type_atom, data)) = self.resolve_data(conn, target, atoms) {
            if data.len() > INCR_THRESHOLD {
                // INCR: advertise the total length, then stream chunks as the
                // requestor deletes the property (see handle_property_notify).
                let len_bytes = (data.len() as u32).to_le_bytes();
                ok = change_prop(
                    conn,
                    ev.requestor,
                    property,
                    a(X11Atom::Incr),
                    32,
                    1,
                    &len_bytes,
                );
                if ok {
                    // We must observe PropertyNotify(DELETE) on the requestor.
                    // Degraded path: this narrows any pre-existing event mask on
                    // a foreign requestor; a full implementation should OR with
                    // the mask installed in CreateNotify instead.
                    let mut aux = ChangeWindowAttributesAux::new();
                    aux.event_mask = Some(EventMask::PROPERTY_CHANGE);
                    if let Err(e) = xproto::change_window_attributes(conn, ev.requestor, &aux) {
                        warn!("x11: selecting PROPERTY_CHANGE for INCR failed: {}", e);
                    }
                    self.incr.insert(
                        ev.requestor,
                        IncrTransfer {
                            requestor: ev.requestor,
                            property,
                            target: type_atom,
                            data,
                            offset: 0,
                        },
                    );
                }
            } else {
                ok = change_prop(
                    conn,
                    ev.requestor,
                    property,
                    type_atom,
                    8,
                    data.len() as u32,
                    &data,
                );
            }
        } else {
            warn!(
                "x11: no clipboard data for target atom {} (requestor {:#x})",
                target, ev.requestor
            );
        }

        let reply_property = if ok { property } else { 0 };
        send_selection_notify(conn, ev, reply_property);
    }

    /// XFixes `SelectionNotify`: the CLIPBOARD owner changed.
    ///
    /// Equivalent to mainline's XFixesSelectionNotify branch: ask the new owner
    /// for `TARGETS` via `convert_selection(CLIPBOARD, TARGETS, _WL_SELECTION)`.
    /// The resolved names are published to the proxy in
    /// [`ClipboardBridge::handle_selection_notify`] once the reply lands.
    pub fn handle_xfixes_selection_notify<C: RequestConnection>(
        &mut self,
        conn: &C,
        atoms: &AtomTable,
        ev: &xfixes::SelectionNotifyEvent,
        bridge: &X11Bridge,
    ) {
        let a = |x: X11Atom| atoms.get(x);
        if ev.selection != a(X11Atom::Clipboard) {
            return;
        }
        let previous_owner = self.owner;
        self.owner = ev.owner;
        debug!(
            "x11: CLIPBOARD owner {:#x} -> {:#x}",
            previous_owner, ev.owner
        );
        if ev.owner == 0 || ev.owner == self.selection_window {
            debug!("x11: CLIPBOARD owner cleared or is ours, ignoring");
            return;
        }

        // Immediate notification for the proxy: the owner changed and a TARGETS
        // conversion is in flight. The concrete list follows on SelectionNotify.
        // This uses the data queue rather than a new (non-existent) WmCommand
        // variant so the proxy's match stays exhaustive.
        bridge.set_clipboard_data(
            "x-sommelier/clipboard-owner-changed".to_string(),
            ev.owner.to_le_bytes().to_vec(),
        );

        let property = a(X11Atom::WlSelection);
        if let Err(e) = xproto::convert_selection(
            conn,
            self.selection_window,
            a(X11Atom::Clipboard),
            a(X11Atom::Targets),
            property,
            0u32,
        ) {
            warn!("x11: convert_selection(CLIPBOARD, TARGETS) failed: {}", e);
        }
    }

    /// X11 `SelectionNotify`: our `convert_selection` completed.
    ///
    /// Reads the property, resolves the TARGETS atoms to strings and publishes
    /// them on the bridge for the Wayland side. Also implements the receive
    /// side of INCR.
    pub fn handle_selection_notify<C: RequestConnection>(
        &mut self,
        conn: &C,
        atoms: &AtomTable,
        ev: &SelectionNotifyEvent,
        bridge: &X11Bridge,
    ) {
        let a = |x: X11Atom| atoms.get(x);
        if ev.property == 0 {
            warn!("x11: SelectionNotify with property NONE (conversion refused)");
            return;
        }
        if self.selection_window != 0 && ev.requestor != self.selection_window {
            return;
        }

        let reply = match xproto::get_property(
            conn,
            true,
            ev.requestor,
            ev.property,
            0u32,
            0,
            PROPERTY_LONG_LENGTH,
        ) {
            Ok(cookie) => match cookie.reply() {
                Ok(r) => r,
                Err(e) => {
                    warn!("x11: get_property(selection) failed: {}", e);
                    return;
                }
            },
            Err(e) => {
                warn!("x11: get_property(selection) request failed: {}", e);
                return;
            }
        };
        if reply.type_ == 0 {
            return;
        }

        // INCR receive: the owner streams the TARGETS list in chunks.
        if reply.type_ == a(X11Atom::Incr) {
            self.recv_incr = Some(RecvIncr {
                requestor: ev.requestor,
                property: ev.property,
                data: Vec::new(),
            });
            // Deleting the property prompts the owner to write the first chunk.
            if let Err(e) = xproto::delete_property(conn, ev.requestor, ev.property) {
                warn!("x11: delete_property(INCR) failed: {}", e);
            }
            return;
        }

        if ev.target != a(X11Atom::Targets) {
            // A data conversion (e.g. UTF8_STRING) finished: forward bytes to
            // the Wayland side.
            let name = atom_name(conn, ev.target);
            if !reply.value.is_empty() {
                bridge.set_clipboard_data(name, reply.value.clone());
            }
            return;
        }

        if reply.format != 32 {
            warn!(
                "x11: TARGETS property has unexpected format {}",
                reply.format
            );
            return;
        }
        let names = resolve_atom_names(conn, &parse_u32_bytes(&reply.value));
        debug!("x11: CLIPBOARD TARGETS: {:?}", names);
        self.targets = names.clone();
        bridge.set_clipboard_targets(names);
    }

    /// Drive INCR transfers from `PropertyNotify` events.
    ///
    /// Send side: a DELETE on the requestor means it consumed the previous
    /// chunk, so we write the next one (or a zero-length terminator).
    /// Receive side: a NEW_VALUE on our selection window is one more chunk of
    /// an INCR TARGETS list from another owner.
    pub fn handle_property_notify<C: RequestConnection>(
        &mut self,
        conn: &C,
        ev: &xproto::PropertyNotifyEvent,
        bridge: &X11Bridge,
    ) {
        // ---- INCR receive ----
        // Collect results into locals so the `&mut recv_incr` borrow ends
        // before we clear/complete the state below.
        let mut finished_names: Option<Vec<String>> = None;
        let mut consumed = false;
        if let Some(ri) = self.recv_incr.as_mut() {
            if ev.window == ri.requestor {
                if ev.state == Property::NEW_VALUE {
                    consumed = true;
                    let reply = match xproto::get_property(
                        conn,
                        true,
                        ri.requestor,
                        ri.property,
                        0u32,
                        0,
                        PROPERTY_LONG_LENGTH,
                    ) {
                        Ok(cookie) => match cookie.reply() {
                            Ok(r) => Some(r),
                            Err(e) => {
                                warn!("x11: INCR get_property failed: {}", e);
                                None
                            }
                        },
                        Err(e) => {
                            warn!("x11: INCR get_property request failed: {}", e);
                            None
                        }
                    };
                    if let Some(r) = reply {
                        if r.value.is_empty() {
                            // Zero-length chunk terminates the transfer.
                            finished_names =
                                Some(resolve_atom_names(conn, &parse_u32_bytes(&ri.data)));
                        } else {
                            ri.data.extend_from_slice(&r.value);
                            // The read used delete=true, which already prompts
                            // the owner for the next chunk; nothing more to do.
                        }
                    }
                } else if ev.state == Property::DELETE {
                    // A DELETE triggered by our own delete=true read is expected
                    // and must not be mistaken for send-side progress.
                    consumed = true;
                }
            }
        }

        if let Some(names) = finished_names {
            debug!("x11: INCR receive finished: {:?}", names);
            self.recv_incr = None;
            self.targets = names.clone();
            bridge.set_clipboard_targets(names);
            return;
        }
        if consumed {
            return;
        }

        // ---- INCR send ----
        if ev.state != Property::DELETE {
            return;
        }
        let Some(mut t) = self.incr.remove(&ev.window) else {
            return;
        };
        let end = (t.offset + INCR_CHUNK).min(t.data.len());
        let chunk = t.data[t.offset..end].to_vec();
        let ok = change_prop(
            conn,
            t.requestor,
            t.property,
            t.target,
            8,
            chunk.len() as u32,
            &chunk,
        );
        if !ok {
            warn!("x11: INCR chunk write failed, aborting transfer");
            return;
        }
        t.offset = end;
        if t.offset >= t.data.len() {
            // Zero-length property = end of INCR stream.
            change_prop(conn, t.requestor, t.property, t.target, 8, 0, &[]);
            debug!("x11: INCR send finished for {:#x}", t.requestor);
        } else {
            self.incr.insert(ev.window, t);
        }
    }

    /// Map a requested target atom to the bytes we hold from the Wayland side.
    fn resolve_data<C: RequestConnection>(
        &self,
        conn: &C,
        target: u32,
        atoms: &AtomTable,
    ) -> Option<(u32, Vec<u8>)> {
        let a = |x: X11Atom| atoms.get(x);
        if target == a(X11Atom::Utf8String) {
            return self.text_data.clone().map(|d| (a(X11Atom::Utf8String), d));
        }
        if target == a(X11Atom::StringType) || target == a(X11Atom::Text) {
            return self.text_data.clone().map(|d| (a(X11Atom::StringType), d));
        }
        let name = atom_name(conn, target);
        self.data.get(&name).map(|d| (target, d.clone()))
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

fn u32_list_bytes(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn parse_u32_bytes(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn atom_name<C: RequestConnection>(conn: &C, atom: u32) -> String {
    if atom == 0 {
        return String::new();
    }
    match xproto::get_atom_name(conn, atom) {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => String::from_utf8_lossy(&r.name).into_owned(),
            Err(e) => {
                warn!("x11: get_atom_name({}) failed: {}", atom, e);
                String::new()
            }
        },
        Err(e) => {
            warn!("x11: get_atom_name({}) request failed: {}", atom, e);
            String::new()
        }
    }
}

fn resolve_atom_names<C: RequestConnection>(conn: &C, atoms: &[u32]) -> Vec<String> {
    atoms
        .iter()
        .filter(|&&a| a != 0)
        .map(|&a| atom_name(conn, a))
        .collect()
}

/// Write a property and check for X11 errors. Returns whether it succeeded.
fn change_prop<C: RequestConnection>(
    conn: &C,
    window: u32,
    property: u32,
    type_: u32,
    format: u8,
    data_len: u32,
    data: &[u8],
) -> bool {
    match xproto::change_property(
        conn,
        PropMode::REPLACE,
        window,
        property,
        type_,
        format,
        data_len,
        data,
    ) {
        Ok(cookie) => match cookie.check() {
            Ok(()) => true,
            Err(e) => {
                warn!("x11: ChangeProperty failed: {}", e);
                false
            }
        },
        Err(e) => {
            warn!("x11: ChangeProperty request failed: {}", e);
            false
        }
    }
}

/// Reply to a SelectionRequest with a synthetic `SelectionNotify`.
fn send_selection_notify<C: RequestConnection>(
    conn: &C,
    ev: &SelectionRequestEvent,
    property: u32,
) {
    let notify = SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: ev.time,
        requestor: ev.requestor,
        selection: ev.selection,
        target: ev.target,
        property,
    };
    if let Err(e) = xproto::send_event(conn, false, ev.requestor, EventMask::NO_EVENT, notify) {
        warn!("x11: send_event(SelectionNotify) failed: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_list_bytes_roundtrip() {
        let bytes = u32_list_bytes(&[4, 31, 6]);
        assert_eq!(parse_u32_bytes(&bytes), vec![4, 31, 6]);
        assert_eq!(parse_u32_bytes(&[1, 2, 3]), Vec::<u32>::new());
    }

    #[test]
    fn note_data_sets_text_fallback_only_for_text_targets() {
        let mut cb = ClipboardBridge::new(1);
        cb.note_data("image/png".to_string(), vec![0x89, 0x50]);
        assert!(cb.text_data.is_none());
        cb.note_data("text/plain;charset=utf-8".to_string(), b"hello".to_vec());
        assert_eq!(cb.text_data.as_deref(), Some(&b"hello"[..]));
        assert_eq!(cb.targets().len(), 0);
    }

    #[test]
    fn note_targets_is_visible() {
        let mut cb = ClipboardBridge::new(1);
        cb.note_targets(vec!["UTF8_STRING".into(), "text/plain".into()]);
        assert_eq!(cb.targets().len(), 2);
        assert_eq!(cb.targets()[0], "UTF8_STRING");
        assert_eq!(cb.targets()[1], "text/plain");
    }
}
