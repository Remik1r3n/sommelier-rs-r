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

//! Xwayland process launch and WM bootstrap.
//!
//! Mirrors mainline `sl_spawn_xwayland()` (sommelier.cc:4029) and
//! `sl_handle_display_ready_event()` (sommelier.cc:3561): three socketpairs
//! carry the `-displayfd` handshake, the dedicated `-wm` X11 connection and
//! the `WAYLAND_SOCKET` client connection. Once `:<N>` arrives we set the X11
//! environment, wrap `wm[0]` in an x11rb `RustConnection` and run `wm::run()`
//! on a dedicated OS thread.

use std::error::Error;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::{info, warn};
use x11rb::connection::Connection as _;
use x11rb::rust_connection::{DefaultStream, RustConnection};

use crate::x11::atoms::AtomTable;
use crate::x11::bridge::X11Bridge;
use crate::x11::wm;

/// Default Xwayland binary on ChromeOS Crostini.
const DEFAULT_XWAYLAND_PATH: &str = "/opt/google/cros-containers/bin/Xwayland";
/// Mainline sommelier uses `XCURSOR_SIZE_BASE * scale` with base 24.
const XCURSOR_SIZE_BASE: f64 = 24.0;
/// How long to wait for Xwayland to write its display number to `-displayfd`.
const DISPLAY_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// All knobs that affect Xwayland startup and WM behaviour.
///
/// Fields that are only consumed by the Wayland proxy (`app_id_prefix`,
/// `frame_color`, `dark_frame_color`, `allow_randr_emu`) are kept here so the
/// caller has a single options object; the X11 side simply carries them along.
#[derive(Debug, Clone)]
pub struct X11Options {
    pub xwayland_path: String,
    pub xwayland_cmd_prefix: Vec<String>,
    /// `--x-display=N`: when set, `:<N>` is passed on the command line.
    pub x_display: Option<u32>,
    pub x_auth: Option<String>,
    pub x_font_path: Option<String>,
    pub gl_driver_path: Option<String>,
    pub enable_xshape: bool,
    pub clipboard_manager: bool,
    /// Pass `-shm` to Xwayland (software rendering fallback).
    pub shm: bool,
    pub scale: f64,
    /// `org.chromium.guest_os.<vm>.` — mainline APPLICATION_ID_FORMAT_PREFIX.
    /// The WM appends `wmclass.`/`xid.`/`wmclientleader.`/`xprop.` + value.
    pub app_id_prefix: String,
    /// `--application-id`: overrides every derived X11 app id.
    pub application_id: Option<String>,
    /// `--application-id-x11-property`: CARDINAL property read for app ids.
    pub application_id_property: Option<String>,
    // X11 frame colour (X11 0xRRGGBB) and RandR emulation, both applied by
    // the WM thread; see `wm.rs::border_color` / `update_randr_emu`.
    pub frame_color: String,
    pub dark_frame_color: String,
    pub allow_randr_emu: bool,
}

impl Default for X11Options {
    fn default() -> Self {
        Self {
            xwayland_path: DEFAULT_XWAYLAND_PATH.to_string(),
            xwayland_cmd_prefix: Vec::new(),
            x_display: None,
            x_auth: None,
            x_font_path: None,
            gl_driver_path: None,
            // Mainline defaults (sommelier-ctx.cc:210): XShape is opt-in.
            enable_xshape: false,
            clipboard_manager: true,
            shm: true,
            scale: 1.0,
            app_id_prefix: String::new(),
            application_id: None,
            application_id_property: None,
            frame_color: String::new(),
            dark_frame_color: String::new(),
            allow_randr_emu: false,
        }
    }
}

/// Handle returned by [`start_x11`].
pub struct X11Runtime {
    /// The `DISPLAY` value, e.g. `":0"`.
    pub display: String,
    /// PID of the Xwayland child.
    pub pid: i32,
    /// The WM OS thread, if it was spawned successfully.
    pub wm_thread: Option<JoinHandle<()>>,
}

/// Create the socketpair Xwayland uses as its `WAYLAND_SOCKET`.
///
/// This is created by `main` *before* the proxy starts: Xwayland must be able
/// to talk to the Wayland proxy before it writes the display number to
/// `-displayfd`, so the compositor end has to be handed to `proxy::run()`
/// immediately. Only the child end is passed into [`start_x11`].
///
/// CLOEXEC is intentionally set on every fd; `pre_exec` below clears it only
/// for the three fds the child actually needs, so no stray descriptors leak
/// into Xwayland.
pub fn make_socketpair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds: [libc::c_int; 2] = [0, 0];
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socketpair() produced two valid, owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Blocking read of `":<N>\n"` from the `-displayfd` socket, with a timeout.
///
/// Equivalent to the readable-event handler `sl_handle_display_ready_event`.
/// We `poll()` first and use `MSG_DONTWAIT` for the actual read so a misbehaving
/// Xwayland can never hang the caller.
fn read_display_number(fd: RawFd, timeout: Duration) -> io::Result<String> {
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 64];
    let mut len: usize = 0;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        if timeout_ms <= 0 && len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for Xwayland -displayfd",
            ));
        }

        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if rc == 0 {
            break;
        }
        if pfd.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            break;
        }

        let n = unsafe {
            libc::recv(
                fd,
                buf[len..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - len,
                libc::MSG_DONTWAIT,
            )
        };
        if n <= 0 {
            break;
        }
        len += n as usize;
        if buf[..len].contains(&b'\n') || len == buf.len() {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buf[..len]);
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Xwayland displayfd did not contain a display number: {:?}",
                text
            ),
        ));
    }
    Ok(format!(":{}", digits))
}

/// Launch Xwayland and start the WM thread.
pub fn start_x11(
    opts: X11Options,
    bridge: Arc<X11Bridge>,
    wl_child: OwnedFd,
) -> Result<X11Runtime, Box<dyn Error + Send + Sync>> {
    // ds[0] is read by us, ds[1] is written by Xwayland.
    let (display_parent, display_child) = make_socketpair()?;
    // wm[0] becomes our X11 WM connection, wm[1] is passed via -wm.
    let (wm_parent, wm_child) = make_socketpair()?;
    // The compositor side (wl[0]) was already handed to proxy::run() by main;
    // wl_child is the WAYLAND_SOCKET end owned by the Xwayland child.

    let display_child_fd = display_child.as_raw_fd();
    let wm_child_fd = wm_child.as_raw_fd();
    let wl_child_fd = wl_child.as_raw_fd();

    // ---- Build the Xwayland command line (sl_spawn_xwayland) ----
    let mut args: Vec<String> = opts.xwayland_cmd_prefix.clone();
    args.push(opts.xwayland_path.clone());
    if let Some(n) = opts.x_display {
        args.push(format!(":{}", n));
    }
    args.push("-nolisten".to_string());
    args.push("tcp".to_string());
    args.push("-rootless".to_string());
    if opts.shm {
        args.push("-shm".to_string());
    }
    args.push("-displayfd".to_string());
    args.push(display_child_fd.to_string());
    args.push("-wm".to_string());
    args.push(wm_child_fd.to_string());
    if let Some(path) = &opts.x_auth {
        args.push("-auth".to_string());
        args.push(path.clone());
    }
    if let Some(path) = &opts.x_font_path {
        args.push("-fp".to_string());
        args.push(path.clone());
    }

    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]);
    // sl_execvp(): Xwayland uses this pre-connected socket as its Wayland
    // client connection to sommelier.
    cmd.env("WAYLAND_SOCKET", wl_child_fd.to_string());
    cmd.env("SOMMELIER_VERSION", env!("CARGO_PKG_VERSION"));
    if let Some(path) = &opts.gl_driver_path {
        if !path.is_empty() {
            cmd.env("LIBGL_DRIVERS_PATH", path);
        }
    }

    // Clear FD_CLOEXEC for exactly the three fds the child must keep. Using
    // `pre_exec` (rather than a raw fork) keeps Rust's exec error reporting.
    let child_fds = [display_child_fd, wm_child_fd, wl_child_fd];
    unsafe {
        cmd.pre_exec(move || {
            for &fd in &child_fds {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    info!("x11: launching Xwayland: {:?}", args);
    let mut child = cmd.spawn()?;
    let pid = child.id() as i32;

    // Close our copies of the child ends (the forked child has its own table).
    drop(display_child);
    drop(wm_child);
    drop(wl_child);

    // Reap Xwayland when it eventually exits so it does not become a zombie.
    // Dropping `Child` without wait() would leave it unreaped.
    std::thread::Builder::new()
        .name("sommelier-xwayland-reaper".to_string())
        .spawn(move || {
            if let Err(e) = child.wait() {
                warn!("x11: waiting for Xwayland failed: {}", e);
            }
        })
        .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })?;

    // ---- Wait for -displayfd (sl_handle_display_ready_event) ----
    let display = read_display_number(display_parent.as_raw_fd(), DISPLAY_READY_TIMEOUT)?;
    drop(display_parent);

    std::env::set_var("DISPLAY", &display);
    let cursor_size = (XCURSOR_SIZE_BASE * opts.scale).round() as i32;
    std::env::set_var("XCURSOR_SIZE", cursor_size.to_string());
    if let Some(path) = &opts.gl_driver_path {
        if !path.is_empty() {
            std::env::set_var("LIBGL_DRIVERS_PATH", path);
        }
    }
    bridge.set_display(display.clone());
    info!(
        "x11: Xwayland ready on {} (pid {}), XCURSOR_SIZE={}",
        display, pid, cursor_size
    );

    // ---- Build the WM X11 connection from wm[0] ----
    // `from_unix_stream` returns `(DefaultStream, PeerAddr)`; we only need the
    // stream. It also puts the fd in non-blocking mode, which suits the WM
    // poll loop in wm::run.
    let wm_stream = unsafe { UnixStream::from_raw_fd(wm_parent.into_raw_fd()) };
    let (stream, _peer_addr) = DefaultStream::from_unix_stream(wm_stream)?;
    let conn = RustConnection::connect_to_stream(stream, 0)?;

    let bridge_for_thread = bridge.clone();
    let atoms = AtomTable::new();
    let wm_thread = std::thread::Builder::new()
        .name("sommelier-x11-wm".to_string())
        .spawn(move || {
            // `Setup` is Clone; keep an owned copy on this thread's stack and
            // hand `run` a borrow that lives for the whole WM loop.
            let setup = conn.setup().clone();
            wm::run(conn, &setup, bridge_for_thread, atoms, opts);
        })?;

    Ok(X11Runtime {
        display,
        pid,
        wm_thread: Some(wm_thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_sane() {
        let o = X11Options::default();
        assert_eq!(o.xwayland_path, DEFAULT_XWAYLAND_PATH);
        assert!(o.xwayland_cmd_prefix.is_empty());
        assert_eq!(o.x_display, None);
        assert!(o.clipboard_manager);
        assert!(!o.enable_xshape);
        assert!(o.shm);
        assert_eq!(o.scale, 1.0);
        assert_eq!((XCURSOR_SIZE_BASE * o.scale).round() as i32, 24);
    }

    #[test]
    fn socketpair_ends_are_usable() {
        let (a, b) = make_socketpair().expect("socketpair");
        assert!(a.as_raw_fd() >= 0);
        assert!(b.as_raw_fd() >= 0);
        assert_ne!(a.as_raw_fd(), b.as_raw_fd());
    }
}
