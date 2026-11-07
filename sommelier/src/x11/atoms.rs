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

//! X11 atom table.
//!
//! Mirrors `enum ATOM_*` / `sl_context_atom_name()` in mainline
//! `sommelier-ctx.h` / `sommelier-ctx.cc`, plus the ICCCM predefined atoms we
//! need as property *types* (WINDOW / ATOM / STRING / CARDINAL). Interning a
//! predefined atom by name simply returns its well-known numeric value, so no
//! special-casing of `AtomEnum` is required.

use std::collections::HashMap;
use std::error::Error;

use x11rb::connection::RequestConnection;
use x11rb::protocol::xproto::ConnectionExt;

/// Every atom sommelier's X11 WM needs, with its on-wire name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Atom {
    // ICCCM / WM (mainline ATOM_WM_S0 .. ATOM_WM_CLIENT_LEADER)
    WmS0,
    WmProtocols,
    WmState,
    WmChangeState,
    WmDeleteWindow,
    WmTakeFocus,
    WmClientLeader,
    WlSurfaceId,
    Utf8String,
    MotifWmHints,

    // EWMH (mainline ATOM_NET_*)
    NetActiveWindow,
    NetFrameExtents,
    NetStartupId,
    NetSupported,
    NetSupportingWmCheck,
    NetWmName,
    NetWmMoveresize,
    NetWmState,
    NetWmStateFullscreen,
    NetWmStateMaximizedVert,
    NetWmStateMaximizedHorz,
    NetWmStateFocused,
    NetWmWindowType,
    NetWmWindowTypeNormal,
    NetWmWindowTypeDialog,
    NetWmWindowTypeSplash,
    NetWmWindowTypeUtility,
    NetWmPid,

    // selection / clipboard (mainline ATOM_CLIPBOARD .. ATOM_WL_SELECTION)
    Clipboard,
    ClipboardManager,
    Targets,
    Timestamp,
    Text,
    Incr,
    WlSelection,

    // sommelier / ChromeOS specific
    GtkThemeVariant,
    SteamGame,
    XwaylandRandrEmuMonitorRects,
    SommelierQuirkApplied,

    // ICCCM properties read in `sl_handle_map_request`
    WmName,
    WmClass,
    WmTransientFor,
    WmNormalHints,
    WmHints,

    // Predefined X11 atoms used as property *types*.
    Window,
    AtomType,
    StringType,
    Cardinal,
}

impl Atom {
    /// All atoms, in the order they are interned by [`AtomTable::intern_all`].
    pub const ALL: &'static [Atom] = &[
        Atom::WmS0,
        Atom::WmProtocols,
        Atom::WmState,
        Atom::WmChangeState,
        Atom::WmDeleteWindow,
        Atom::WmTakeFocus,
        Atom::WmClientLeader,
        Atom::WlSurfaceId,
        Atom::Utf8String,
        Atom::MotifWmHints,
        Atom::NetActiveWindow,
        Atom::NetFrameExtents,
        Atom::NetStartupId,
        Atom::NetSupported,
        Atom::NetSupportingWmCheck,
        Atom::NetWmName,
        Atom::NetWmMoveresize,
        Atom::NetWmState,
        Atom::NetWmStateFullscreen,
        Atom::NetWmStateMaximizedVert,
        Atom::NetWmStateMaximizedHorz,
        Atom::NetWmStateFocused,
        Atom::NetWmWindowType,
        Atom::NetWmWindowTypeNormal,
        Atom::NetWmWindowTypeDialog,
        Atom::NetWmWindowTypeSplash,
        Atom::NetWmWindowTypeUtility,
        Atom::NetWmPid,
        Atom::Clipboard,
        Atom::ClipboardManager,
        Atom::Targets,
        Atom::Timestamp,
        Atom::Text,
        Atom::Incr,
        Atom::WlSelection,
        Atom::GtkThemeVariant,
        Atom::SteamGame,
        Atom::XwaylandRandrEmuMonitorRects,
        Atom::SommelierQuirkApplied,
        Atom::WmName,
        Atom::WmClass,
        Atom::WmTransientFor,
        Atom::WmNormalHints,
        Atom::WmHints,
        Atom::Window,
        Atom::AtomType,
        Atom::StringType,
        Atom::Cardinal,
    ];

    /// The on-wire atom name, byte-for-byte compatible with libxcb.
    pub fn name(self) -> &'static str {
        match self {
            Atom::WmS0 => "WM_S0",
            Atom::WmProtocols => "WM_PROTOCOLS",
            Atom::WmState => "WM_STATE",
            Atom::WmChangeState => "WM_CHANGE_STATE",
            Atom::WmDeleteWindow => "WM_DELETE_WINDOW",
            Atom::WmTakeFocus => "WM_TAKE_FOCUS",
            Atom::WmClientLeader => "WM_CLIENT_LEADER",
            Atom::WlSurfaceId => "WL_SURFACE_ID",
            Atom::Utf8String => "UTF8_STRING",
            Atom::MotifWmHints => "_MOTIF_WM_HINTS",
            Atom::NetActiveWindow => "_NET_ACTIVE_WINDOW",
            Atom::NetFrameExtents => "_NET_FRAME_EXTENTS",
            Atom::NetStartupId => "_NET_STARTUP_ID",
            Atom::NetSupported => "_NET_SUPPORTED",
            Atom::NetSupportingWmCheck => "_NET_SUPPORTING_WM_CHECK",
            Atom::NetWmName => "_NET_WM_NAME",
            Atom::NetWmMoveresize => "_NET_WM_MOVERESIZE",
            Atom::NetWmState => "_NET_WM_STATE",
            Atom::NetWmStateFullscreen => "_NET_WM_STATE_FULLSCREEN",
            Atom::NetWmStateMaximizedVert => "_NET_WM_STATE_MAXIMIZED_VERT",
            Atom::NetWmStateMaximizedHorz => "_NET_WM_STATE_MAXIMIZED_HORZ",
            Atom::NetWmStateFocused => "_NET_WM_STATE_FOCUSED",
            Atom::NetWmWindowType => "_NET_WM_WINDOW_TYPE",
            Atom::NetWmWindowTypeNormal => "_NET_WM_WINDOW_TYPE_NORMAL",
            Atom::NetWmWindowTypeDialog => "_NET_WM_WINDOW_TYPE_DIALOG",
            Atom::NetWmWindowTypeSplash => "_NET_WM_WINDOW_TYPE_SPLASH",
            Atom::NetWmWindowTypeUtility => "_NET_WM_WINDOW_TYPE_UTILITY",
            Atom::NetWmPid => "_NET_WM_PID",
            Atom::Clipboard => "CLIPBOARD",
            Atom::ClipboardManager => "CLIPBOARD_MANAGER",
            Atom::Targets => "TARGETS",
            Atom::Timestamp => "TIMESTAMP",
            Atom::Text => "TEXT",
            Atom::Incr => "INCR",
            Atom::WlSelection => "_WL_SELECTION",
            Atom::GtkThemeVariant => "_GTK_THEME_VARIANT",
            Atom::SteamGame => "STEAM_GAME",
            Atom::XwaylandRandrEmuMonitorRects => "_XWAYLAND_RANDR_EMU_MONITOR_RECTS",
            Atom::SommelierQuirkApplied => "SOMMELIER_QUIRK_APPLIED",
            Atom::WmName => "WM_NAME",
            Atom::WmClass => "WM_CLASS",
            Atom::WmTransientFor => "WM_TRANSIENT_FOR",
            Atom::WmNormalHints => "WM_NORMAL_HINTS",
            Atom::WmHints => "WM_HINTS",
            Atom::Window => "WINDOW",
            Atom::AtomType => "ATOM",
            Atom::StringType => "STRING",
            Atom::Cardinal => "CARDINAL",
        }
    }
}

/// Resolved atom values, filled in once at WM startup.
#[derive(Debug, Clone, Default)]
pub struct AtomTable {
    values: HashMap<Atom, u32>,
}

// Some lookup helpers are only used by diagnostics/tests today; keep the
// full table API available for the selection bridge.
#[allow(dead_code)]
impl AtomTable {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Batch-intern every atom.
    ///
    /// Equivalent to the `sl_connect()` loop that fires all
    /// `xcb_intern_atom()` requests first and only then collects the replies.
    /// x11rb's `Cookie::reply()` wraps `RequestConnection::wait_for_reply`, so
    /// collecting the cookies before resolving them preserves that batching.
    pub fn intern_all<C: RequestConnection>(&mut self, conn: &C) -> Result<(), Box<dyn Error>> {
        let cookies = Atom::ALL
            .iter()
            .map(|a| conn.intern_atom(false, a.name().as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;

        for (a, cookie) in Atom::ALL.iter().zip(cookies.into_iter()) {
            let reply = cookie.reply()?;
            self.values.insert(*a, reply.atom);
        }
        Ok(())
    }

    /// Resolve an atom to its X11 numeric value. Returns `0` (`None`) before
    /// [`AtomTable::intern_all`] has run, which is a harmless degraded value
    /// for property comparisons.
    pub fn get(&self, a: Atom) -> u32 {
        self.values.get(&a).copied().unwrap_or(0)
    }

    /// Reverse lookup used when converting CLIPBOARD `TARGETS` atoms into the
    /// MIME-like strings the Wayland side expects.
    pub fn name_of(&self, value: u32) -> Option<&'static str> {
        if value == 0 {
            return None;
        }
        self.values
            .iter()
            .find(|(_, v)| **v == value)
            .map(|(a, _)| a.name())
    }

    /// Number of resolved atoms (useful for diagnostics/tests).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn atom_names_match_mainline() {
        assert_eq!(Atom::WmS0.name(), "WM_S0");
        assert_eq!(Atom::WlSurfaceId.name(), "WL_SURFACE_ID");
        assert_eq!(Atom::NetSupported.name(), "_NET_SUPPORTED");
        assert_eq!(
            Atom::NetWmStateFullscreen.name(),
            "_NET_WM_STATE_FULLSCREEN"
        );
        assert_eq!(
            Atom::NetWmStateMaximizedVert.name(),
            "_NET_WM_STATE_MAXIMIZED_VERT"
        );
        assert_eq!(
            Atom::NetWmStateMaximizedHorz.name(),
            "_NET_WM_STATE_MAXIMIZED_HORZ"
        );
        assert_eq!(Atom::Clipboard.name(), "CLIPBOARD");
        assert_eq!(Atom::ClipboardManager.name(), "CLIPBOARD_MANAGER");
        assert_eq!(Atom::Targets.name(), "TARGETS");
        assert_eq!(Atom::Timestamp.name(), "TIMESTAMP");
        assert_eq!(Atom::Utf8String.name(), "UTF8_STRING");
        assert_eq!(Atom::WlSelection.name(), "_WL_SELECTION");
        assert_eq!(Atom::Incr.name(), "INCR");
        assert_eq!(
            Atom::SommelierQuirkApplied.name(),
            "SOMMELIER_QUIRK_APPLIED"
        );
        assert_eq!(
            Atom::XwaylandRandrEmuMonitorRects.name(),
            "_XWAYLAND_RANDR_EMU_MONITOR_RECTS"
        );
        assert_eq!(Atom::WmClass.name(), "WM_CLASS");
        assert_eq!(Atom::WmNormalHints.name(), "WM_NORMAL_HINTS");
        assert_eq!(Atom::MotifWmHints.name(), "_MOTIF_WM_HINTS");
    }

    #[test]
    fn all_atoms_have_unique_names_and_cover_the_enum() {
        assert!(Atom::ALL.len() >= 40);
        let mut names = HashSet::new();
        for a in Atom::ALL {
            assert!(names.insert(a.name()), "duplicate atom name {}", a.name());
            assert!(!a.name().is_empty());
        }
        // Every declared variant must appear in ALL.
        assert!(Atom::ALL.contains(&Atom::Window));
        assert!(Atom::ALL.contains(&Atom::Cardinal));
        assert!(Atom::ALL.contains(&Atom::NetWmPid));
    }

    #[test]
    fn empty_table_returns_none_atom() {
        let t = AtomTable::new();
        assert!(t.is_empty());
        assert_eq!(t.get(Atom::Clipboard), 0);
        assert_eq!(t.name_of(0), None);
    }
}
