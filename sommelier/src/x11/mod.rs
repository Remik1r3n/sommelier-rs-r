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

//! XWayland / X11 window-management subsystem.
//!
//! This fills the X11 gap that upstream `sommelier-rs` deliberately left out.
//! It follows the design of mainline C++ sommelier: `start_x11()` launches
//! Xwayland with `-displayfd` / `-wm` / `WAYLAND_SOCKET`, `wm::run()` acts as
//! the rootless X11 window manager over the dedicated `-wm` connection, and
//! `X11Bridge` correlates X11 XIDs with host `wl_surface` ids so the Wayland
//! proxy can build `xdg_surface` / `xdg_toplevel` / `zaura_surface` objects.

pub mod atoms;
pub mod bridge;
pub mod clipboard;
pub mod selection;
pub mod wm;
pub mod xwayland;

pub use bridge::{WmCommand, WmEvent, X11Bridge, X11WindowInfo};
pub use selection::SelectionBridge;
pub use xwayland::{start_x11, X11Options, X11Runtime};
