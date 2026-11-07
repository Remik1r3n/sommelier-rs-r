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

//! The X11 window manager main loop.
//!
//! This is the Rust counterpart of mainline `sl_connect()` (sommelier.cc:3222)
//! for setup and `sl_handle_x_connection_event()` (sommelier.cc:3053) for
//! event dispatch, plus `sl_handle_map_request()` (sommelier.cc:1200) for the
//! batched ICCCM/EWMH property read.
//!
//! Note on the trait bound: x11rb 0.13 places `poll_for_event()`, `flush()`,
//! `setup()` and `generate_id()` on `x11rb::connection::Connection`, not on
//! `RequestConnection`. Both bounds are therefore required even though the
//! original API sketch only listed `RequestConnection`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use x11rb::connection::{Connection as X11Connection, RequestConnection};
use x11rb::cookie::{Cookie, VoidCookie};
use x11rb::errors::ConnectionError;
use x11rb::protocol::xproto::{
    self, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent, ConfigWindow,
    ConfigureNotifyEvent, ConfigureWindowAux, CreateWindowAux, EventMask, Gravity, InputFocus,
    PropMode, Property, WindowClass,
};
use x11rb::protocol::{composite, shape, xfixes, Event};
use x11rb::x11_utils::TryParse;

use crate::x11::atoms::{Atom as X11Atom, AtomTable};
use crate::x11::bridge::{WmCommand, WmEvent, X11Bridge, X11WindowInfo};
use crate::x11::clipboard::ClipboardBridge;
use crate::x11::xwayland::X11Options;

/// ICCCM `WM_STATE` values.
const WM_STATE_NORMAL: u32 = 1;
const WM_STATE_ICONIC: u32 = 3;
/// `MWM_HINTS_DECORATIONS`, `MWM_DECOR_ALL`, `MWM_DECOR_TITLE`.
const MWM_HINTS_DECORATIONS: u32 = 1 << 1;
const MWM_DECOR_ALL: u32 = 1 << 0;
const MWM_DECOR_TITLE: u32 = 1 << 3;
/// ICCCM `WM_NORMAL_HINTS` flags (`PMinSize` / `PMaxSize`).
const P_MIN_SIZE: u32 = 1 << 4;
const P_MAX_SIZE: u32 = 1 << 5;
/// `XURGENCY_HINT` in WM_HINTS.flags.
const X_URGENCY_HINT: u32 = 1 << 8;
/// EWMH `_NET_WM_STATE` client-message action codes.
const STATE_ACTION_REMOVE: u32 = 0;
const STATE_ACTION_ADD: u32 = 1;
const STATE_ACTION_TOGGLE: u32 = 2;
/// `WM_CHANGE_STATE` -> IconicState.
const ICONIC_STATE: u32 = 3;

// ---------------------------------------------------------------------------
// Pure byte-level property parsers (kept as free functions for unit tests).
// ---------------------------------------------------------------------------

/// Decode a Latin-1 (ISO-8859-1) X11 property into a Rust `String`.
pub fn parse_latin1(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

/// Decode a UTF8_STRING X11 property, replacing invalid sequences.
pub fn parse_utf8(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

/// Interpret a byte buffer as little-endian u32 values.
pub fn parse_u32_list(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Parse an X11 atom array. `format` is the property format (16 or 32).
pub fn parse_atom_list(data: &[u8], format: u8) -> Vec<u32> {
    match format {
        32 => parse_u32_list(data),
        16 => data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
            .collect(),
        _ => Vec::new(),
    }
}

/// First u32 of a property, if present.
pub fn parse_u32(data: &[u8]) -> Option<u32> {
    if data.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

/// `WM_CLASS` is two NUL-terminated Latin-1 strings: `instance\0class\0`.
pub fn parse_wm_class(data: &[u8]) -> (String, String) {
    let mut parts = data.split(|&b| b == 0);
    let instance = parts.next().map(parse_latin1).unwrap_or_default();
    let class = parts.next().map(parse_latin1).unwrap_or_default();
    (instance, class)
}

/// The 18 INT32 fields of a `WM_NORMAL_HINTS` (XSizeHints) property.
///
/// Layout: flags, x, y, width, height, min_width, min_height, max_width,
/// max_height, width_inc, height_inc, min_aspect.x/y, max_aspect.x/y,
/// base_width, base_height, win_gravity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WmSizeHints {
    pub flags: u32,
    pub min_width: i32,
    pub min_height: i32,
    pub max_width: i32,
    pub max_height: i32,
    pub width_inc: i32,
    pub height_inc: i32,
    pub base_width: i32,
    pub base_height: i32,
    pub win_gravity: i32,
}

/// Decode `WM_NORMAL_HINTS`. Short/truncated properties degrade to zeros.
pub fn parse_wm_normal_hints(data: &[u8]) -> WmSizeHints {
    let v = parse_u32_list(data);
    let get = |i: usize| v.get(i).copied().unwrap_or(0) as i32;
    WmSizeHints {
        flags: v.first().copied().unwrap_or(0),
        min_width: get(5),
        min_height: get(6),
        max_width: get(7),
        max_height: get(8),
        width_inc: get(9),
        height_inc: get(10),
        base_width: get(15),
        base_height: get(16),
        win_gravity: get(17),
    }
}

/// `_MOTIF_WM_HINTS`: flags, functions, decorations, input_mode, status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MotifHints {
    pub flags: u32,
    pub decorations: u32,
}

pub fn parse_motif_hints(data: &[u8]) -> Option<MotifHints> {
    let v = parse_u32_list(data);
    if v.len() < 5 {
        return None;
    }
    Some(MotifHints {
        flags: v[0],
        decorations: v[2],
    })
}

/// Apply the mainline `mwm_hints` decoration rule.
fn motif_is_decorated(h: &MotifHints, fallback: bool) -> bool {
    if h.flags & MWM_HINTS_DECORATIONS == 0 {
        return fallback;
    }
    if h.decorations & MWM_DECOR_ALL != 0 {
        h.decorations & MWM_DECOR_TITLE == 0
    } else {
        h.decorations & MWM_DECOR_TITLE != 0
    }
}

fn u32s_to_bytes(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn state_action(action: u32, current: bool) -> bool {
    match action {
        STATE_ACTION_REMOVE => false,
        STATE_ACTION_ADD => true,
        STATE_ACTION_TOGGLE => !current,
        _ => current,
    }
}

// ---------------------------------------------------------------------------
// x11rb request/reply helpers. Errors are logged, never panicked on.
// ---------------------------------------------------------------------------

/// Parse `--frame-color` (`#RRGGBB`, `0xRRGGBB` or bare `RRGGBB`) into an
/// X11 0x00RRGGBB pixel value. Invalid values disable the border.
fn parse_frame_color(value: &str) -> Option<u32> {
    let s = value.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s
        .strip_prefix('#')
        .or_else(|| s.strip_prefix("0x"))
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

fn checked<C: RequestConnection>(result: Result<VoidCookie<'_, C>, ConnectionError>, what: &str) {
    match result {
        Ok(cookie) => {
            if let Err(e) = cookie.check() {
                warn!("x11: {} failed: {}", what, e);
            }
        }
        Err(e) => warn!("x11: {} failed: {}", what, e),
    }
}

fn take_reply<C, R>(result: Result<Cookie<'_, C, R>, ConnectionError>, what: &str) -> Option<R>
where
    C: RequestConnection,
    R: TryParse,
{
    match result {
        Ok(cookie) => match cookie.reply() {
            Ok(reply) => Some(reply),
            Err(e) => {
                warn!("x11: {} reply failed: {}", what, e);
                None
            }
        },
        Err(e) => {
            warn!("x11: {} request failed: {}", what, e);
            None
        }
    }
}

/// Synthetic `ConfigureNotify`, as sent by mainline `sl_send_configure_notify`.
fn send_synthetic_configure<C: RequestConnection>(
    conn: &C,
    xid: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let ev = ConfigureNotifyEvent {
        response_type: xproto::CONFIGURE_NOTIFY_EVENT,
        sequence: 0,
        event: xid,
        window: xid,
        above_sibling: 0,
        x: x as i16,
        y: y as i16,
        width: w.max(1) as u16,
        height: h.max(1) as u16,
        border_width: 0,
        override_redirect: false,
    };
    checked(
        xproto::send_event(conn, false, xid, EventMask::NO_EVENT, ev),
        "synthetic ConfigureNotify",
    );
}

fn write_frame_extents<C: RequestConnection>(conn: &C, atoms: &AtomTable, xid: u32, border: u32) {
    let extents = [border; 4];
    checked(
        xproto::change_property(
            conn,
            PropMode::REPLACE,
            xid,
            atoms.get(X11Atom::NetFrameExtents),
            atoms.get(X11Atom::Cardinal),
            32,
            extents.len() as u32,
            &u32s_to_bytes(&extents),
        ),
        "write _NET_FRAME_EXTENTS",
    );
}

fn write_wm_state<C: RequestConnection>(conn: &C, atoms: &AtomTable, xid: u32, state: u32) {
    let values = [state, 0u32];
    checked(
        xproto::change_property(
            conn,
            PropMode::REPLACE,
            xid,
            atoms.get(X11Atom::WmState),
            atoms.get(X11Atom::WmState),
            32,
            values.len() as u32,
            &u32s_to_bytes(&values),
        ),
        "write WM_STATE",
    );
}

/// Reflect the current fullscreen/maximized flags back into `_NET_WM_STATE`.
fn write_net_wm_state<C: RequestConnection>(
    conn: &C,
    atoms: &AtomTable,
    xid: u32,
    xinfo: &X11WindowInfo,
) {
    let mut states = Vec::new();
    if xinfo.fullscreen {
        states.push(atoms.get(X11Atom::NetWmStateFullscreen));
    }
    if xinfo.maximized {
        states.push(atoms.get(X11Atom::NetWmStateMaximizedVert));
        states.push(atoms.get(X11Atom::NetWmStateMaximizedHorz));
    }
    let bytes = u32s_to_bytes(&states);
    checked(
        xproto::change_property(
            conn,
            PropMode::REPLACE,
            xid,
            atoms.get(X11Atom::NetWmState),
            atoms.get(X11Atom::AtomType),
            32,
            states.len() as u32,
            &bytes,
        ),
        "write _NET_WM_STATE",
    );
}

fn configure_aux_from_request(e: &xproto::ConfigureRequestEvent) -> ConfigureWindowAux {
    let m = e.value_mask;
    ConfigureWindowAux {
        x: if m.contains(ConfigWindow::X) {
            Some(e.x as i32)
        } else {
            None
        },
        y: if m.contains(ConfigWindow::Y) {
            Some(e.y as i32)
        } else {
            None
        },
        width: if m.contains(ConfigWindow::WIDTH) {
            Some(e.width as u32)
        } else {
            None
        },
        height: if m.contains(ConfigWindow::HEIGHT) {
            Some(e.height as u32)
        } else {
            None
        },
        border_width: if m.contains(ConfigWindow::BORDER_WIDTH) {
            Some(e.border_width as u32)
        } else {
            None
        },
        sibling: if m.contains(ConfigWindow::SIBLING) {
            Some(e.sibling)
        } else {
            None
        },
        stack_mode: if m.contains(ConfigWindow::STACK_MODE) {
            Some(e.stack_mode)
        } else {
            None
        },
    }
}

// ---------------------------------------------------------------------------
// Window bookkeeping.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropKind {
    WmName,
    NetWmName,
    WmClass,
    WmTransientFor,
    WmNormalHints,
    WmClientLeader,
    WmProtocols,
    MotifWmHints,
    NetStartupId,
    NetWmState,
    GtkThemeVariant,
    XwaylandRandrEmuMonitorRects,
    SteamGame,
    NetWmWindowType,
    NetWmPid,
    WmHints,
}

fn prop_kind_for_atom(atom: u32, atoms: &AtomTable) -> Option<PropKind> {
    let a = |x: X11Atom| atoms.get(x);
    if atom == a(X11Atom::WmName) {
        Some(PropKind::WmName)
    } else if atom == a(X11Atom::NetWmName) {
        Some(PropKind::NetWmName)
    } else if atom == a(X11Atom::WmClass) {
        Some(PropKind::WmClass)
    } else if atom == a(X11Atom::WmTransientFor) {
        Some(PropKind::WmTransientFor)
    } else if atom == a(X11Atom::WmNormalHints) {
        Some(PropKind::WmNormalHints)
    } else if atom == a(X11Atom::WmClientLeader) {
        Some(PropKind::WmClientLeader)
    } else if atom == a(X11Atom::WmProtocols) {
        Some(PropKind::WmProtocols)
    } else if atom == a(X11Atom::MotifWmHints) {
        Some(PropKind::MotifWmHints)
    } else if atom == a(X11Atom::NetStartupId) {
        Some(PropKind::NetStartupId)
    } else if atom == a(X11Atom::NetWmState) {
        Some(PropKind::NetWmState)
    } else if atom == a(X11Atom::GtkThemeVariant) {
        Some(PropKind::GtkThemeVariant)
    } else if atom == a(X11Atom::XwaylandRandrEmuMonitorRects) {
        Some(PropKind::XwaylandRandrEmuMonitorRects)
    } else if atom == a(X11Atom::SteamGame) {
        Some(PropKind::SteamGame)
    } else if atom == a(X11Atom::NetWmWindowType) {
        Some(PropKind::NetWmWindowType)
    } else if atom == a(X11Atom::NetWmPid) {
        Some(PropKind::NetWmPid)
    } else if atom == a(X11Atom::WmHints) {
        Some(PropKind::WmHints)
    } else {
        None
    }
}

#[derive(Debug, Default)]
struct PropertyResult {
    info: X11WindowInfo,
    has_delete_window: bool,
    has_take_focus: bool,
    client_leader: u32,
}

/// Decode one property reply into `PropertyResult`.
///
/// Corresponds to the `switch (properties[i].type)` body of
/// `sl_handle_map_request()`.
fn apply_property(
    result: &mut PropertyResult,
    kind: PropKind,
    data: &[u8],
    format: u8,
    atoms: &AtomTable,
) {
    let info = &mut result.info;
    let a = |x: X11Atom| atoms.get(x);
    match kind {
        PropKind::WmName => {
            // WM_NAME (Latin-1) is only a fallback; _NET_WM_NAME wins.
            if info.title.is_empty() {
                info.title = parse_latin1(data);
            }
        }
        PropKind::NetWmName => {
            info.title = parse_utf8(data);
        }
        PropKind::WmClass => {
            let (instance, class) = parse_wm_class(data);
            info.wm_class = class.clone();
            info.app_id = if class.is_empty() { instance } else { class };
        }
        PropKind::WmTransientFor => {
            info.transient_for = parse_u32(data).unwrap_or(0);
        }
        PropKind::WmNormalHints => {
            let h = parse_wm_normal_hints(data);
            if h.flags & P_MIN_SIZE != 0 {
                info.min_size = (h.min_width, h.min_height);
            }
            if h.flags & P_MAX_SIZE != 0 {
                info.max_size = (h.max_width, h.max_height);
            }
        }
        PropKind::WmClientLeader => {
            result.client_leader = parse_u32(data).unwrap_or(0);
        }
        PropKind::WmProtocols => {
            for atom in parse_atom_list(data, format) {
                if atom == a(X11Atom::WmDeleteWindow) {
                    result.has_delete_window = true;
                } else if atom == a(X11Atom::WmTakeFocus) {
                    result.has_take_focus = true;
                }
            }
        }
        PropKind::MotifWmHints => {
            if let Some(h) = parse_motif_hints(data) {
                info.decorated = motif_is_decorated(&h, info.decorated);
            }
        }
        PropKind::NetStartupId => {
            debug!("x11: _NET_STARTUP_ID={}", parse_utf8(data));
        }
        PropKind::NetWmState => {
            let mut fullscreen = false;
            let mut max_h = false;
            let mut max_v = false;
            for atom in parse_atom_list(data, format) {
                if atom == a(X11Atom::NetWmStateFullscreen) {
                    fullscreen = true;
                } else if atom == a(X11Atom::NetWmStateMaximizedHorz) {
                    max_h = true;
                } else if atom == a(X11Atom::NetWmStateMaximizedVert) {
                    max_v = true;
                }
            }
            // Neither Wayland nor CrOS supports 1D maximizing.
            info.maximized = max_h && max_v;
            info.fullscreen = fullscreen;
        }
        PropKind::GtkThemeVariant => {
            info.dark_theme = data.starts_with(b"dark");
        }
        PropKind::XwaylandRandrEmuMonitorRects => {
            // Consumed by the proxy's RandR emulation; here just diagnostic.
            debug!(
                "x11: _XWAYLAND_RANDR_EMU_MONITOR_RECTS: {} cardinals",
                parse_u32_list(data).len()
            );
        }
        PropKind::SteamGame => {
            info.steam_game_id = parse_u32(data).unwrap_or(0);
        }
        PropKind::NetWmWindowType => {
            if let Some(first) = parse_atom_list(data, format).first() {
                info.window_type = *first;
            }
        }
        PropKind::NetWmPid => {
            info.pid = parse_u32(data).unwrap_or(0);
        }
        PropKind::WmHints => {
            let flags = parse_u32(data).unwrap_or(0);
            info.urgent = flags & X_URGENCY_HINT != 0;
        }
    }
}

#[derive(Debug)]
struct XWindow {
    /// The client XID; retained for diagnostics and the clipboard/selection
    /// bridge even though window lookup currently keys on `frame_id`.
    #[allow(dead_code)]
    xid: u32,
    frame_id: u32,
    info: X11WindowInfo,
    managed: bool,
    mapped: bool,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    pending_configure: Option<(i32, i32)>,
    iconified: bool,
    surface_guest_id: u32,
    /// `WL_SURFACE_ID` resolved to a host wl_surface.
    paired: bool,
    toplevel_created: bool,
    has_delete_window: bool,
    has_take_focus: bool,
    client_leader: u32,
}

impl XWindow {
    fn new(xid: u32) -> Self {
        Self {
            xid,
            frame_id: 0,
            info: X11WindowInfo {
                xid,
                ..Default::default()
            },
            managed: false,
            mapped: false,
            width: 0,
            height: 0,
            x: 0,
            y: 0,
            pending_configure: None,
            iconified: false,
            surface_guest_id: 0,
            paired: false,
            toplevel_created: false,
            has_delete_window: false,
            has_take_focus: false,
            client_leader: 0,
        }
    }
}

struct WmState {
    root: u32,
    screen_w: i32,
    screen_h: i32,
    wm_window: u32,
    selection_window: u32,
    atoms: AtomTable,
    windows: HashMap<u32, XWindow>,
    /// (xid, guest wl_surface id) pairs awaiting `publish_surface`.
    pending_surface: Vec<(u32, u32)>,
    clipboard: ClipboardBridge,
    app_id_prefix: String,
    shape_enabled: bool,
    quit: bool,
    /// `--allow-xwayland-emulate-screen-pos-size` (RandR emulation).
    allow_randr_emu: bool,
    last_output_rects_gen: u64,
    /// Parsed `--frame-color` / `--dark-frame-color`, as X11 0xRRGGBB pixels.
    frame_color: Option<u32>,
    dark_frame_color: Option<u32>,
}

impl WmState {
    fn new(root: u32, screen_w: i32, screen_h: i32, atoms: AtomTable, opts: &X11Options) -> Self {
        let selection_window = 0;
        Self {
            root,
            screen_w,
            screen_h,
            wm_window: 0,
            selection_window,
            atoms,
            windows: HashMap::new(),
            pending_surface: Vec::new(),
            clipboard: ClipboardBridge::new(selection_window),
            app_id_prefix: opts.app_id_prefix.clone(),
            shape_enabled: opts.enable_xshape,
            quit: false,
            allow_randr_emu: opts.allow_randr_emu,
            last_output_rects_gen: 0,
            frame_color: parse_frame_color(&opts.frame_color),
            dark_frame_color: parse_frame_color(&opts.dark_frame_color),
        }
    }

    /// Write `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` whenever the proxy reports a
    /// new host output layout. Xwayland reads this property to emulate
    /// multiple monitors for X11 clients (mainline
    /// `--allow-xwayland-emulate-screen-pos-size`).
    fn update_randr_emu<C>(&mut self, conn: &C, bridge: &X11Bridge)
    where
        C: RequestConnection,
    {
        if !self.allow_randr_emu {
            return;
        }
        let (generation, rects) = bridge.output_rects_snapshot();
        if generation == self.last_output_rects_gen || rects.is_empty() {
            return;
        }
        self.last_output_rects_gen = generation;
        let mut bytes = Vec::with_capacity(rects.len() * 4);
        for value in &rects {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let atoms = self.atoms.clone();
        checked(
            xproto::change_property(
                conn,
                PropMode::REPLACE,
                self.root,
                atoms.get(X11Atom::XwaylandRandrEmuMonitorRects),
                atoms.get(X11Atom::Cardinal),
                32,
                rects.len() as u32,
                &bytes,
            ),
            "update _XWAYLAND_RANDR_EMU_MONITOR_RECTS",
        );
        debug!(
            "x11: RandR emulation: {} host output rectangles published",
            rects.len() / 4
        );
    }

    /// Frame colour for a window; dark-theme windows get the dark colour
    /// when one was configured.
    fn border_color(&self, info: &X11WindowInfo) -> Option<u32> {
        if info.dark_theme {
            self.dark_frame_color.or(self.frame_color)
        } else {
            self.frame_color.or(self.dark_frame_color)
        }
    }

    /// `sl_connect()` setup: event mask, COMPOSITE/XFixes/shape, WM window,
    /// selection owners, `_NET_SUPPORTING_WM_CHECK`, `_NET_SUPPORTED`.
    fn init<C>(&mut self, conn: &C, bridge: &X11Bridge, opts: &X11Options)
    where
        C: RequestConnection + X11Connection,
    {
        // Clone the resolved table into a local so borrows never conflict with
        // subsequent `&mut self` method calls in the same handler.
        let atoms_snapshot = self.atoms.clone();
        let a = &atoms_snapshot;
        bridge.set_screen_size(self.screen_w, self.screen_h);

        let mask = EventMask::STRUCTURE_NOTIFY
            | EventMask::SUBSTRUCTURE_NOTIFY
            | EventMask::SUBSTRUCTURE_REDIRECT;
        checked(
            xproto::change_window_attributes(
                conn,
                self.root,
                &ChangeWindowAttributesAux::new().event_mask(mask),
            ),
            "select root SUBSTRUCTURE_REDIRECT",
        );

        // Rootless Xwayland requires COMPOSITE_REDIRECT_MANUAL.
        checked(
            composite::redirect_subwindows(conn, self.root, composite::Redirect::MANUAL),
            "composite redirect_subwindows",
        );

        if let Some(reply) = take_reply(xfixes::query_version(conn, 5, 0), "xfixes query_version") {
            if reply.major_version >= 5 {
                info!(
                    "x11: XFixes {}.{} available",
                    reply.major_version, reply.minor_version
                );
            } else {
                warn!(
                    "x11: XFixes {}.{} is older than 5, selection events may be unavailable",
                    reply.major_version, reply.minor_version
                );
            }
        }

        if self.shape_enabled {
            if let Some(reply) = take_reply(shape::query_version(conn), "shape query_version") {
                info!(
                    "x11: SHAPE {}.{} available",
                    reply.major_version, reply.minor_version
                );
            }
            checked(
                shape::select_input(conn, self.root, true),
                "shape select_input on root",
            );
        }

        // 1x1 INPUT_ONLY WM window, used for _NET_SUPPORTING_WM_CHECK / WM_S0.
        let wm_window = match conn.generate_id() {
            Ok(id) => id,
            Err(e) => {
                warn!("x11: generate_id for WM window failed: {}", e);
                return;
            }
        };
        self.wm_window = wm_window;
        checked(
            xproto::create_window(
                conn,
                0,
                wm_window,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            ),
            "create WM window",
        );

        // Optional CLIPBOARD_MANAGER / CLIPBOARD owner (sl_connect).
        if opts.clipboard_manager {
            let sel = match conn.generate_id() {
                Ok(id) => id,
                Err(e) => {
                    warn!("x11: generate_id for selection window failed: {}", e);
                    0
                }
            };
            if sel != 0 {
                self.selection_window = sel;
                self.clipboard = ClipboardBridge::new(sel);
                checked(
                    xproto::create_window(
                        conn,
                        0,
                        sel,
                        self.root,
                        0,
                        0,
                        1,
                        1,
                        0,
                        WindowClass::INPUT_OUTPUT,
                        0,
                        &CreateWindowAux::new(),
                    ),
                    "create selection window",
                );
                let mut aux = ChangeWindowAttributesAux::new();
                aux.event_mask = Some(EventMask::PROPERTY_CHANGE);
                checked(
                    xproto::change_window_attributes(conn, sel, &aux),
                    "select selection window events",
                );
                checked(
                    xproto::set_selection_owner(conn, sel, a.get(X11Atom::ClipboardManager), 0u32),
                    "own CLIPBOARD_MANAGER",
                );
                checked(
                    xproto::set_selection_owner(conn, sel, a.get(X11Atom::Clipboard), 0u32),
                    "own CLIPBOARD",
                );
                let sel_mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
                checked(
                    xfixes::select_selection_input(conn, sel, a.get(X11Atom::Clipboard), sel_mask),
                    "xfixes select_selection_input",
                );
            }
        }

        // _NET_SUPPORTING_WM_CHECK (on both WM window and root) + _NET_WM_NAME.
        let win_bytes = u32s_to_bytes(&[wm_window]);
        for target in [wm_window, self.root] {
            checked(
                xproto::change_property(
                    conn,
                    PropMode::REPLACE,
                    target,
                    a.get(X11Atom::NetSupportingWmCheck),
                    a.get(X11Atom::Window),
                    32,
                    1,
                    &win_bytes,
                ),
                "write _NET_SUPPORTING_WM_CHECK",
            );
        }
        let wm_name = b"Sommelier";
        checked(
            xproto::change_property(
                conn,
                PropMode::REPLACE,
                wm_window,
                a.get(X11Atom::NetWmName),
                a.get(X11Atom::Utf8String),
                8,
                wm_name.len() as u32,
                wm_name,
            ),
            "write _NET_WM_NAME",
        );

        // _NET_SUPPORTED: the subset of EWMH we actually implement.
        let supported = [
            X11Atom::NetActiveWindow,
            X11Atom::NetWmName,
            X11Atom::NetWmState,
            X11Atom::NetWmStateFullscreen,
            X11Atom::NetWmStateMaximizedVert,
            X11Atom::NetWmStateMaximizedHorz,
            X11Atom::NetWmStateFocused,
            X11Atom::NetFrameExtents,
            X11Atom::NetWmMoveresize,
            X11Atom::NetSupportingWmCheck,
            X11Atom::NetSupported,
            X11Atom::NetWmWindowType,
            X11Atom::NetWmPid,
        ]
        .iter()
        .map(|x| a.get(*x))
        .collect::<Vec<u32>>();
        let supported_bytes = u32s_to_bytes(&supported);
        checked(
            xproto::change_property(
                conn,
                PropMode::REPLACE,
                self.root,
                a.get(X11Atom::NetSupported),
                a.get(X11Atom::AtomType),
                32,
                supported.len() as u32,
                &supported_bytes,
            ),
            "write _NET_SUPPORTED",
        );

        // Become the WM_S0 selection owner and reset focus.
        checked(
            xproto::set_selection_owner(conn, wm_window, a.get(X11Atom::WmS0), 0u32),
            "own WM_S0",
        );
        checked(
            xproto::set_input_focus(conn, InputFocus::NONE, 0u32, 0u32),
            "set_input_focus",
        );

        if let Err(e) = conn.flush() {
            warn!("x11: initial flush failed: {}", e);
        }
        info!(
            "x11: WM initialized, root={:#x}, wm_window={:#x}",
            self.root, wm_window
        );
    }

    /// Batched ICCCM/EWMH property read, equivalent to `sl_handle_map_request`.
    fn read_properties<C>(&self, conn: &C, xid: u32) -> PropertyResult
    where
        C: RequestConnection,
    {
        // Clone the resolved table into a local so borrows never conflict with
        // subsequent `&mut self` method calls in the same handler.
        let atoms_snapshot = self.atoms.clone();
        let a = &atoms_snapshot;
        let props: Vec<(PropKind, u32)> = vec![
            (PropKind::WmName, a.get(X11Atom::WmName)),
            (PropKind::NetWmName, a.get(X11Atom::NetWmName)),
            (PropKind::WmClass, a.get(X11Atom::WmClass)),
            (PropKind::WmTransientFor, a.get(X11Atom::WmTransientFor)),
            (PropKind::WmNormalHints, a.get(X11Atom::WmNormalHints)),
            (PropKind::WmClientLeader, a.get(X11Atom::WmClientLeader)),
            (PropKind::WmProtocols, a.get(X11Atom::WmProtocols)),
            (PropKind::MotifWmHints, a.get(X11Atom::MotifWmHints)),
            (PropKind::NetStartupId, a.get(X11Atom::NetStartupId)),
            (PropKind::NetWmState, a.get(X11Atom::NetWmState)),
            (PropKind::GtkThemeVariant, a.get(X11Atom::GtkThemeVariant)),
            (
                PropKind::XwaylandRandrEmuMonitorRects,
                a.get(X11Atom::XwaylandRandrEmuMonitorRects),
            ),
            (PropKind::SteamGame, a.get(X11Atom::SteamGame)),
            (PropKind::NetWmWindowType, a.get(X11Atom::NetWmWindowType)),
            (PropKind::NetWmPid, a.get(X11Atom::NetWmPid)),
            (PropKind::WmHints, a.get(X11Atom::WmHints)),
        ];

        // Fire every GetProperty before consuming any reply (sl_handle_map_request).
        let cookies: Vec<_> = props
            .iter()
            .map(|(_, atom)| xproto::get_property(conn, false, xid, *atom, 0u32, 0, 2048))
            .collect();

        let mut result = PropertyResult::default();
        result.info.xid = xid;
        result.info.decorated = true; // mainline: window->decorated = 1
        for ((kind, _), cookie) in props.iter().zip(cookies.into_iter()) {
            if let Some(reply) = take_reply(cookie, "get_property(map)") {
                if reply.type_ != 0 && reply.format > 0 {
                    apply_property(&mut result, *kind, &reply.value, reply.format, a);
                }
            }
        }
        result
    }

    // ----- event handlers -----

    fn handle_event<C>(&mut self, conn: &C, bridge: &X11Bridge, ev: Event)
    where
        C: RequestConnection + X11Connection,
    {
        let atoms_snapshot = self.atoms.clone();
        match ev {
            Event::CreateNotify(e) => self.on_create_notify(conn, e),
            Event::DestroyNotify(e) => self.on_destroy_notify(bridge, e),
            Event::ReparentNotify(e) => self.on_reparent_notify(e),
            Event::MapRequest(e) => self.on_map_request(conn, bridge, e),
            Event::ConfigureRequest(e) => self.on_configure_request(conn, e),
            Event::ConfigureNotify(e) => self.on_configure_notify(e),
            Event::ClientMessage(e) => self.on_client_message(conn, bridge, e),
            Event::PropertyNotify(e) => self.on_property_notify(conn, bridge, e),
            Event::FocusIn(e) => self.on_focus(bridge, e.event, true),
            Event::FocusOut(e) => self.on_focus(bridge, e.event, false),
            Event::SelectionRequest(e) => {
                self.clipboard
                    .handle_selection_request(conn, &atoms_snapshot, &e, bridge)
            }
            Event::SelectionNotify(e) => {
                self.clipboard
                    .handle_selection_notify(conn, &atoms_snapshot, &e, bridge)
            }
            Event::XfixesSelectionNotify(e) => {
                self.clipboard
                    .handle_xfixes_selection_notify(conn, &atoms_snapshot, &e, bridge)
            }
            Event::ShapeNotify(e) => {
                if self.windows.contains_key(&e.affected_window) {
                    debug!("x11: ShapeNotify for xid {:#x}", e.affected_window);
                    bridge.send_command(WmCommand::Commit {
                        xid: e.affected_window,
                    });
                }
            }
            Event::Error(e) => warn!("x11: X11 error packet: {:?}", e),
            Event::Unknown(_) => {}
            other => debug!("x11: unhandled event: {:?}", other),
        }
    }

    fn on_create_notify<C>(&mut self, conn: &C, e: xproto::CreateNotifyEvent)
    where
        C: RequestConnection,
    {
        if e.window == self.wm_window || e.window == self.selection_window {
            return;
        }
        let mut w = XWindow::new(e.window);
        w.x = e.x as i32;
        w.y = e.y as i32;
        w.width = e.width as i32;
        w.height = e.height as i32;
        self.windows.insert(e.window, w);

        // Watch properties so PropertyNotify reaches us for EWMH updates and
        // for the INCR clipboard handshake.
        let mut aux = ChangeWindowAttributesAux::new();
        aux.event_mask = Some(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY);
        checked(
            xproto::change_window_attributes(conn, e.window, &aux),
            "select client window events",
        );
        if self.shape_enabled {
            checked(
                shape::select_input(conn, e.window, true),
                "shape select_input on client",
            );
        }
        debug!("x11: CreateNotify xid {:#x}", e.window);
    }

    fn on_destroy_notify(&mut self, bridge: &X11Bridge, e: xproto::DestroyNotifyEvent) {
        if self.windows.remove(&e.window).is_some() {
            bridge.unregister_window(e.window);
            bridge.send_command(WmCommand::DestroyToplevel { xid: e.window });
            self.pending_surface.retain(|(x, _)| *x != e.window);
            debug!("x11: DestroyNotify xid {:#x}", e.window);
        }
    }

    fn on_reparent_notify(&mut self, e: xproto::ReparentNotifyEvent) {
        if let Some(w) = self.windows.get_mut(&e.window) {
            w.frame_id = e.parent;
            w.x = e.x as i32;
            w.y = e.y as i32;
        }
    }

    /// `sl_handle_map_request`: batch property read, geometry, writebacks.
    fn on_map_request<C>(&mut self, conn: &C, bridge: &X11Bridge, e: xproto::MapRequestEvent)
    where
        C: RequestConnection + X11Connection,
    {
        let xid = e.window;
        if xid == self.wm_window || xid == self.selection_window {
            return;
        }

        let pr = self.read_properties(conn, xid);
        let mut xinfo = pr.info;
        xinfo.xid = xid;
        if !self.app_id_prefix.is_empty() && !xinfo.app_id.is_empty() {
            xinfo.app_id = format!("{}{}", self.app_id_prefix, xinfo.app_id);
        }

        // Geometry (skipped when we already have a frame, as in mainline).
        let geometry = take_reply(xproto::get_geometry(conn, xid), "get_geometry");

        checked(xproto::map_window(conn, xid), "map_window");
        let mut attr = ChangeWindowAttributesAux::new();
        attr.win_gravity = Some(Gravity::NORTH_WEST);
        // `--frame-color` / `--dark-frame-color`: paint a 1px X11 border on
        // the client window. A full reparenting frame would change every
        // geometry calculation, so mainline's visible result (a coloured
        // outline + _NET_FRAME_EXTENTS) is achieved without reparenting.
        let border = self.border_color(&xinfo);
        if let Some(color) = border {
            attr.border_pixel = Some(color);
        }
        checked(
            xproto::change_window_attributes(conn, xid, &attr),
            "set win_gravity NORTH_WEST",
        );
        let border_width = if border.is_some() { 1 } else { 0 };
        if border_width != 0 {
            let mut cw = ConfigureWindowAux::new();
            cw.border_width = Some(border_width);
            checked(
                xproto::configure_window(conn, xid, &cw),
                "set frame border width",
            );
        }
        write_frame_extents(conn, &self.atoms, xid, border_width);
        write_wm_state(conn, &self.atoms, xid, WM_STATE_NORMAL);

        let win = self.windows.entry(xid).or_insert_with(|| XWindow::new(xid));
        win.managed = true;
        win.mapped = true;
        win.iconified = false;
        win.info = xinfo;
        win.has_delete_window = pr.has_delete_window;
        win.has_take_focus = pr.has_take_focus;
        win.client_leader = pr.client_leader;
        if let Some(g) = geometry {
            win.x = g.x as i32;
            win.y = g.y as i32;
            win.width = g.width as i32;
            win.height = g.height as i32;
        }
        debug!(
            "x11: MapRequest xid {:#x} title={:?} app_id={:?}",
            xid, win.info.title, win.info.app_id
        );
        self.ensure_toplevel(bridge, xid);
    }

    fn on_configure_request<C>(&mut self, conn: &C, e: xproto::ConfigureRequestEvent)
    where
        C: RequestConnection,
    {
        let managed = self
            .windows
            .get(&e.window)
            .map(|w| w.managed && w.paired)
            .unwrap_or(false);
        if !managed {
            // Unmanaged / unpaired: pass the requested geometry straight through.
            let aux = configure_aux_from_request(&e);
            checked(
                xproto::configure_window(conn, e.window, &aux),
                "ConfigureRequest passthrough",
            );
            if let Some(w) = self.windows.get_mut(&e.window) {
                if e.value_mask.contains(ConfigWindow::WIDTH) {
                    w.width = e.width as i32;
                }
                if e.value_mask.contains(ConfigWindow::HEIGHT) {
                    w.height = e.height as i32;
                }
                if e.value_mask.contains(ConfigWindow::X) {
                    w.x = e.x as i32;
                }
                if e.value_mask.contains(ConfigWindow::Y) {
                    w.y = e.y as i32;
                }
            }
            return;
        }
        // Managed: host owns the geometry; answer with a synthetic notify.
        let (x, y, w, h) = self
            .windows
            .get(&e.window)
            .map(|win| (win.x, win.y, win.width, win.height))
            .unwrap_or((0, 0, e.width as i32, e.height as i32));
        send_synthetic_configure(conn, e.window, x, y, w, h);
        if let Some(win) = self.windows.get_mut(&e.window) {
            win.pending_configure = None;
        }
    }

    fn on_configure_notify(&mut self, e: xproto::ConfigureNotifyEvent) {
        if let Some(w) = self.windows.get_mut(&e.window) {
            w.x = e.x as i32;
            w.y = e.y as i32;
            w.width = e.width as i32;
            w.height = e.height as i32;
        }
    }

    /// `sl_handle_client_message`: WL_SURFACE_ID, EWMH state, active, moveresize.
    fn on_client_message<C>(&mut self, conn: &C, bridge: &X11Bridge, e: ClientMessageEvent)
    where
        C: RequestConnection + X11Connection,
    {
        // Clone the resolved table into a local so borrows never conflict with
        // subsequent `&mut self` method calls in the same handler.
        let atoms_snapshot = self.atoms.clone();
        let a = &atoms_snapshot;
        let d = e.data.as_data32();
        let t = e.type_;
        let xid = e.window;

        if t == a.get(X11Atom::WlSurfaceId) {
            self.on_surface_id(bridge, xid, d[0]);
        } else if t == a.get(X11Atom::NetActiveWindow) {
            debug!("x11: _NET_ACTIVE_WINDOW for {:#x}", xid);
            bridge.send_command(WmCommand::Activate { xid });
            checked(
                xproto::set_input_focus(conn, InputFocus::POINTER_ROOT, xid, 0u32),
                "set_input_focus(active)",
            );
        } else if t == a.get(X11Atom::NetWmMoveresize) {
            // data32[2] is the EWMH direction/edge enum.
            bridge.send_command(WmCommand::MoveResize { xid, edges: d[2] });
        } else if t == a.get(X11Atom::NetWmState) {
            self.on_net_wm_state_message(conn, bridge, xid, &d);
        } else if t == a.get(X11Atom::WmChangeState) {
            if d[0] == ICONIC_STATE {
                if let Some(win) = self.windows.get_mut(&xid) {
                    win.iconified = true;
                    win.info.minimized = true;
                    win.info.fullscreen = false;
                    win.info.maximized = false;
                    let xinfo = win.info.clone();
                    bridge.send_command(WmCommand::UpdateToplevel { xid, info: xinfo });
                    write_wm_state(conn, a, xid, WM_STATE_ICONIC);
                }
            }
        } else {
            debug!("x11: unhandled ClientMessage atom {} on {:#x}", t, xid);
        }
    }

    fn on_net_wm_state_message<C>(&mut self, conn: &C, bridge: &X11Bridge, xid: u32, d: &[u32; 5])
    where
        C: RequestConnection,
    {
        // Clone the resolved table into a local so borrows never conflict with
        // subsequent `&mut self` method calls in the same handler.
        let atoms_snapshot = self.atoms.clone();
        let a = &atoms_snapshot;
        let action = d[0];
        let fs_atom = a.get(X11Atom::NetWmStateFullscreen);
        let max_v = a.get(X11Atom::NetWmStateMaximizedVert);
        let max_h = a.get(X11Atom::NetWmStateMaximizedHorz);
        let mut fullscreen = false;
        let mut maximized = false;
        if let Some(win) = self.windows.get(&xid) {
            fullscreen = win.info.fullscreen;
            maximized = win.info.maximized;
        }
        for &atom in &d[1..=2] {
            if atom == fs_atom {
                fullscreen = state_action(action, fullscreen);
            } else if atom == max_v || atom == max_h {
                maximized = state_action(action, maximized);
            }
        }
        if let Some(win) = self.windows.get_mut(&xid) {
            win.info.fullscreen = fullscreen;
            win.info.maximized = maximized;
            let xinfo = win.info.clone();
            write_net_wm_state(conn, a, xid, &xinfo);
            bridge.send_command(WmCommand::UpdateToplevel { xid, info: xinfo });
        }
    }

    /// `WL_SURFACE_ID`: pair XID with the host wl_surface published by proxy.
    fn on_surface_id(&mut self, bridge: &X11Bridge, xid: u32, guest_id: u32) {
        if let Some(w) = self.windows.get_mut(&xid) {
            w.surface_guest_id = guest_id;
            w.info.surface_guest_id = guest_id;
        }
        match bridge.host_surface(guest_id) {
            Some(host_id) => {
                bridge.register_window(xid, host_id);
                if let Some(w) = self.windows.get_mut(&xid) {
                    w.paired = true;
                }
                self.pending_surface
                    .retain(|(x, g)| !(*x == xid && *g == guest_id));
                debug!(
                    "x11: WL_SURFACE_ID {:#x} guest={} host={}",
                    xid, guest_id, host_id
                );
                self.ensure_toplevel(bridge, xid);
            }
            None => {
                debug!(
                    "x11: WL_SURFACE_ID {:#x} guest={} not yet published, queuing",
                    xid, guest_id
                );
                if !self.pending_surface.contains(&(xid, guest_id)) {
                    self.pending_surface.push((xid, guest_id));
                }
            }
        }
    }

    /// Retry queued WL_SURFACE_ID pairs once the proxy publishes a surface.
    fn retry_pending_surfaces(&mut self, bridge: &X11Bridge) {
        if self.pending_surface.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_surface);
        for (xid, guest_id) in pending {
            match bridge.host_surface(guest_id) {
                Some(host_id) => {
                    bridge.register_window(xid, host_id);
                    if let Some(w) = self.windows.get_mut(&xid) {
                        w.paired = true;
                    }
                    self.ensure_toplevel(bridge, xid);
                }
                None => self.pending_surface.push((xid, guest_id)),
            }
        }
    }

    /// Create (or refresh) the host toplevel once paired + managed.
    fn ensure_toplevel(&mut self, bridge: &X11Bridge, xid: u32) {
        let Some(win) = self.windows.get_mut(&xid) else {
            return;
        };
        if win.paired && win.managed {
            let xinfo = win.info.clone();
            if !win.toplevel_created {
                win.toplevel_created = true;
                bridge.send_command(WmCommand::CreateToplevel(xinfo));
            } else {
                bridge.send_command(WmCommand::UpdateToplevel { xid, info: xinfo });
            }
        }
    }

    fn on_property_notify<C>(
        &mut self,
        conn: &C,
        bridge: &X11Bridge,
        e: xproto::PropertyNotifyEvent,
    ) where
        C: RequestConnection,
    {
        // Local snapshot keeps `&self.atoms` out of the `&mut self.windows` borrow.
        let atoms_snapshot = self.atoms.clone();

        // INCR clipboard transfers are driven by PropertyNotify on the requestor.
        self.clipboard.handle_property_notify(conn, &e, bridge);

        if e.state != Property::NEW_VALUE || !self.windows.contains_key(&e.window) {
            return;
        }
        let Some(kind) = prop_kind_for_atom(e.atom, &atoms_snapshot) else {
            return;
        };
        let reply = take_reply(
            xproto::get_property(conn, false, e.window, e.atom, 0u32, 0, 2048),
            "get_property(notify)",
        );
        let Some(reply) = reply else {
            return;
        };
        if reply.type_ == 0 || reply.format == 0 {
            return;
        }

        // Preserve protocol flags that were parsed at MapRequest time; only the
        // changed property is re-parsed here.
        let prior = self.windows.get(&e.window).unwrap();
        let mut pr = PropertyResult {
            info: prior.info.clone(),
            has_delete_window: prior.has_delete_window,
            has_take_focus: prior.has_take_focus,
            client_leader: prior.client_leader,
        };
        apply_property(&mut pr, kind, &reply.value, reply.format, &atoms_snapshot);
        if let Some(win) = self.windows.get_mut(&e.window) {
            win.info = pr.info;
            if kind == PropKind::WmProtocols {
                win.has_delete_window = pr.has_delete_window;
                win.has_take_focus = pr.has_take_focus;
            }
            if kind == PropKind::WmClientLeader {
                win.client_leader = pr.client_leader;
            }
            let xinfo = win.info.clone();
            if win.toplevel_created {
                bridge.send_command(WmCommand::UpdateToplevel {
                    xid: e.window,
                    info: xinfo,
                });
            }
        }
    }

    fn on_focus(&self, bridge: &X11Bridge, xid: u32, is_in: bool) {
        if !self.windows.contains_key(&xid) {
            return;
        }
        if is_in {
            debug!("x11: FocusIn {:#x}", xid);
            bridge.send_command(WmCommand::Activate { xid });
        } else {
            debug!("x11: FocusOut {:#x}", xid);
        }
    }

    // ----- WmEvent (proxy -> WM) handlers -----

    fn handle_wm_event<C>(&mut self, conn: &C, _bridge: &X11Bridge, ev: WmEvent)
    where
        C: RequestConnection + X11Connection,
    {
        let atoms_snapshot = self.atoms.clone();
        match ev {
            WmEvent::Configure {
                xid,
                width,
                height,
                activated,
                fullscreen,
                maximized,
            } => {
                let w = width.max(1) as u32;
                let h = height.max(1) as u32;
                let mut aux = ConfigureWindowAux::new();
                aux.width = Some(w);
                aux.height = Some(h);
                checked(
                    xproto::configure_window(conn, xid, &aux),
                    "configure_window from host",
                );
                if let Some(win) = self.windows.get_mut(&xid) {
                    win.width = width.max(1);
                    win.height = height.max(1);
                    send_synthetic_configure(conn, xid, win.x, win.y, win.width, win.height);
                    win.info.fullscreen = fullscreen;
                    win.info.maximized = maximized;
                    if activated {
                        win.iconified = false;
                        win.info.minimized = false;
                        win.mapped = true;
                    }
                    let xinfo = win.info.clone();
                    write_net_wm_state(conn, &atoms_snapshot, xid, &xinfo);
                }
            }
            WmEvent::Close { xid } => self.on_close(conn, xid),
            WmEvent::ClipboardTargets { targets } => {
                // Wayland -> X11: these are the MIME types an X11 requestor
                // sees in TARGETS. Do NOT publish them back through
                // set_clipboard_targets(): that channel is X11 -> Wayland and
                // would synthesize a bogus offer on every guest connection.
                self.clipboard.note_targets(targets);
            }
            WmEvent::ClipboardData { target, data } => {
                // Likewise `set_clipboard_data` is reserved for ConvertSelection
                // results (X11 -> Wayland). Feeding prefetched Wayland bytes
                // back into it would make the X11 selection look like an owner
                // change to the proxy.
                self.clipboard.note_data(target, data);
            }
            WmEvent::RequestClipboardData { target } => {
                // Wayland asked for the bytes of an X11 CLIPBOARD target:
                // run ConvertSelection against the current X11 owner.
                self.clipboard
                    .request_target(conn, &atoms_snapshot, &target);
            }
            WmEvent::Shutdown => {
                info!("x11: WM thread shutting down");
                self.quit = true;
            }
        }
        // A ClipboardData event may unblock SelectionRequests that were held
        // while the proxy fetched their payload.
        self.clipboard.flush_pending(conn, &atoms_snapshot);
    }

    /// Close: `WM_DELETE_WINDOW` if supported, otherwise `kill_client`.
    fn on_close<C>(&mut self, conn: &C, xid: u32)
    where
        C: RequestConnection,
    {
        let Some(win) = self.windows.get(&xid) else {
            return;
        };
        // Clone the resolved table into a local so borrows never conflict with
        // subsequent `&mut self` method calls in the same handler.
        let atoms_snapshot = self.atoms.clone();
        let a = &atoms_snapshot;
        if win.has_delete_window {
            let cm = ClientMessageEvent {
                response_type: xproto::CLIENT_MESSAGE_EVENT,
                format: 32,
                sequence: 0,
                window: xid,
                type_: a.get(X11Atom::WmProtocols),
                data: ClientMessageData::from([a.get(X11Atom::WmDeleteWindow), 0, 0, 0, 0]),
            };
            checked(
                xproto::send_event(conn, false, xid, EventMask::NO_EVENT, cm),
                "send WM_DELETE_WINDOW",
            );
        } else {
            warn!(
                "x11: window {:#x} has no WM_DELETE_WINDOW, killing client",
                xid
            );
            checked(xproto::kill_client(conn, xid), "kill_client");
        }
    }
}

/// WM entry point. Runs on a dedicated OS thread spawned by `start_x11`.
///
/// Interns atoms, performs `sl_connect()`-style initialization, then polls X11
/// events and proxy `WmEvent`s in a 2 ms loop (no blocking `wait_for_event`
/// on the tokio runtime).
pub fn run<C>(
    conn: C,
    setup: &xproto::Setup,
    bridge: Arc<X11Bridge>,
    mut atoms: AtomTable,
    opts: X11Options,
) where
    C: RequestConnection + X11Connection + Send + 'static,
{
    if let Err(e) = atoms.intern_all(&conn) {
        warn!("x11: interning atoms failed, WM disabled: {}", e);
        return;
    }

    let Some(screen) = setup.roots.first() else {
        warn!("x11: Xwayland reported no screens, WM disabled");
        return;
    };

    let mut state = WmState::new(
        screen.root,
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
        atoms,
        &opts,
    );
    state.init(&conn, &bridge, &opts);

    loop {
        if state.quit {
            break;
        }
        match conn.poll_for_event() {
            Ok(Some(ev)) => state.handle_event(&conn, &bridge, ev),
            Ok(None) => {}
            Err(e) => {
                warn!("x11: poll_for_event failed, leaving WM loop: {}", e);
                break;
            }
        }

        while let Some(ev) = bridge.try_recv_event() {
            state.handle_wm_event(&conn, &bridge, ev);
            if state.quit {
                break;
            }
        }
        if state.quit {
            break;
        }

        state.retry_pending_surfaces(&bridge);
        state.update_randr_emu(&conn, &bridge);
        // Expire deferred SelectionRequests even when no new WmEvent arrives.
        let atoms_snapshot = state.atoms.clone();
        state.clipboard.flush_pending(&conn, &atoms_snapshot);
        if let Err(e) = conn.flush() {
            warn!("x11: flush failed, leaving WM loop: {}", e);
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    info!("x11: WM loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_color_parsing_accepts_hex_forms() {
        assert_eq!(parse_frame_color("#1a2b3c"), Some(0x001a_2b3c));
        assert_eq!(parse_frame_color("0xFF8800"), Some(0x00ff_8800));
        assert_eq!(parse_frame_color("AABBCC"), Some(0x00aa_bbcc));
        assert_eq!(parse_frame_color("  #000000 "), Some(0));
        assert_eq!(parse_frame_color(""), None);
        assert_eq!(parse_frame_color("#12345"), None);
        assert_eq!(parse_frame_color("not-a-color"), None);
    }

    #[test]
    fn dark_theme_prefers_dark_frame_color() {
        let opts = X11Options {
            xwayland_path: "Xwayland".into(),
            xwayland_cmd_prefix: vec![],
            x_display: None,
            x_auth: None,
            x_font_path: None,
            gl_driver_path: None,
            enable_xshape: false,
            clipboard_manager: true,
            shm: false,
            scale: 1.0,
            app_id_prefix: String::new(),
            frame_color: "#ffffff".into(),
            dark_frame_color: "#202020".into(),
            allow_randr_emu: true,
        };
        let state = WmState::new(1, 1920, 1080, AtomTable::new(), &opts);
        assert!(state.allow_randr_emu);
        let light = X11WindowInfo::default();
        let mut dark = X11WindowInfo::default();
        dark.dark_theme = true;
        assert_eq!(state.border_color(&light), Some(0x00ff_ffff));
        assert_eq!(state.border_color(&dark), Some(0x0020_2020));
    }

    fn hints_bytes() -> Vec<u8> {
        // 18 INT32 fields, little endian.
        let flags = P_MIN_SIZE | P_MAX_SIZE;
        let values: [u32; 18] = [
            flags, 0, 0, 0, 0, // flags, x, y, width, height
            100, 200, // min_width, min_height
            800, 600, // max_width, max_height
            16, 16, // width_inc, height_inc
            0, 0, 0, 0, // aspect
            0, 0, // base size
            1, // win_gravity = NorthWest
        ];
        let mut out = Vec::new();
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn parse_wm_class_two_nul_strings() {
        assert_eq!(
            parse_wm_class(b"google-chrome\0Google-chrome\0"),
            ("google-chrome".to_string(), "Google-chrome".to_string())
        );
    }

    #[test]
    fn parse_wm_class_tolerates_truncation() {
        assert_eq!(
            parse_wm_class(b"only-instance"),
            ("only-instance".to_string(), String::new())
        );
        assert_eq!(parse_wm_class(b""), (String::new(), String::new()));
        assert_eq!(
            parse_wm_class(b"inst\0cls"),
            ("inst".to_string(), "cls".to_string())
        );
    }

    #[test]
    fn parse_wm_class_handles_latin1() {
        let (instance, class) = parse_wm_class(b"caf\xe9\0Caf\xe9\0");
        assert_eq!(instance, "caf\u{e9}");
        assert_eq!(class, "Caf\u{e9}");
    }

    #[test]
    fn parse_wm_normal_hints_full_property() {
        let h = parse_wm_normal_hints(&hints_bytes());
        assert_eq!(h.flags, P_MIN_SIZE | P_MAX_SIZE);
        assert_eq!((h.min_width, h.min_height), (100, 200));
        assert_eq!((h.max_width, h.max_height), (800, 600));
        assert_eq!((h.width_inc, h.height_inc), (16, 16));
        assert_eq!(h.win_gravity, 1);
        assert_eq!(h.base_width, 0);
    }

    #[test]
    fn parse_wm_normal_hints_truncated_degrades_to_zero() {
        let short = [7u32, 1, 2]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let h = parse_wm_normal_hints(&short);
        assert_eq!(h.flags, 7);
        assert_eq!(h.min_width, 0);
        assert_eq!(h.max_width, 0);
        assert_eq!(parse_wm_normal_hints(&[]), WmSizeHints::default());
    }

    #[test]
    fn parse_atom_list_handles_16_and_32_bit_formats() {
        let bytes32 = [4u32, 31u32, 6u32]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(parse_atom_list(&bytes32, 32), vec![4, 31, 6]);
        let bytes16 = [4u16, 31, 6]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(parse_atom_list(&bytes16, 16), vec![4, 31, 6]);
        assert!(parse_atom_list(&bytes32, 8).is_empty());
    }

    #[test]
    fn parse_motif_hints_and_decorations() {
        let values = [MWM_HINTS_DECORATIONS, 0u32, MWM_DECOR_TITLE, 0, 0];
        let bytes = values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let h = parse_motif_hints(&bytes).expect("5 u32 motif hints");
        assert_eq!(h.flags, MWM_HINTS_DECORATIONS);
        assert!(motif_is_decorated(&h, false));

        let no_title = [MWM_HINTS_DECORATIONS, 0u32, MWM_DECOR_ALL, 0, 0];
        let bytes2 = no_title
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        // MWM_DECOR_ALL set but TITLE not requested by the inverted rule.
        assert!(motif_is_decorated(
            &parse_motif_hints(&bytes2).unwrap(),
            false
        ));
        assert!(parse_motif_hints(&[0u8; 8]).is_none());
    }

    #[test]
    fn parse_u32_and_utf8() {
        assert_eq!(parse_u32(&[42, 0, 0, 0]), Some(42));
        assert_eq!(parse_u32(&[1, 2]), None);
        assert_eq!(parse_utf8("héllo".as_bytes()), "héllo");
        assert_eq!(parse_latin1(&[0x41, 0xe9]), "A\u{e9}");
    }

    #[test]
    fn state_action_codes_match_ewmh() {
        assert!(!state_action(STATE_ACTION_REMOVE, true));
        assert!(state_action(STATE_ACTION_ADD, false));
        assert!(!state_action(STATE_ACTION_TOGGLE, true));
        assert!(state_action(STATE_ACTION_TOGGLE, false));
        assert!(state_action(999, true));
    }

    #[test]
    fn configure_aux_maps_value_mask() {
        let e = xproto::ConfigureRequestEvent {
            response_type: 0,
            stack_mode: xproto::StackMode::ABOVE,
            sequence: 0,
            parent: 1,
            window: 2,
            sibling: 0,
            x: 10,
            y: 20,
            width: 300,
            height: 200,
            border_width: 0,
            value_mask: ConfigWindow::X | ConfigWindow::WIDTH,
        };
        let aux = configure_aux_from_request(&e);
        assert_eq!(aux.x, Some(10));
        assert_eq!(aux.width, Some(300));
        assert_eq!(aux.y, None);
        assert_eq!(aux.height, None);
    }
}
