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

//! Unified configuration: upstream `sommelier` flags + `SOMMELIER_*` env vars
//! + the flags from both `sommelier-rs` branches.
//!
//! Mainline rule of thumb (README.md: "every option has a corresponding
//! environment variable, command line overrides environment") is preserved.

use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

use crate::quirks::Quirks;
use crate::transform::{ScalingMode, Transform};
use crate::x11::X11Options;

pub const DEFAULT_XWAYLAND_PATH: &str = "/opt/google/cros-containers/bin/Xwayland";
pub const DEFAULT_VIRTWL_DEVICE: &str = "/dev/wl0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// `/dev/wl0` virtio_wl — Crostini's only working backend.
    VirtWl,
    /// virtio-gpu cross-domain — crosvm / generic VMMs.
    VirtGpu,
    /// Plain unix socket to an already-running compositor (debugging).
    Unix,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VirtWl => "virtwl",
            Self::VirtGpu => "virtgpu",
            Self::Unix => "unix",
        }
    }
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "sommelier-rs-r",
    version,
    about = "X11-capable Wayland proxy for ChromeOS Crostini (sommelier-rs-r)",
    long_about = None
)]
pub struct Args {
    // ---- listener -------------------------------------------------------
    /// Name of the socket to listen on under `$XDG_RUNTIME_DIR`.
    ///
    /// Mainline `--socket` (sommelier.cc:4253). Defaults to `wayland-0`.
    /// This is the display name *guest* clients use, NOT the host display.
    #[arg(long = "socket", env = "SOMMELIER_SOCKET", alias = "listen-display")]
    pub socket: Option<String>,

    /// Host Wayland display to connect to (mainline `--display` /
    /// `SOMMELIER_DISPLAY`, sommelier.cc:4176,4255).
    ///
    /// When set, mainline skips the virtual Wayland channel entirely and uses
    /// this socket; we do the same by selecting the `unix` transport.
    /// A relative name is resolved against `$XDG_RUNTIME_DIR`.
    #[arg(long = "display", env = "SOMMELIER_DISPLAY")]
    pub display: Option<String>,

    // ---- host transport -------------------------------------------------
    /// Host transport: auto | virtwl | virtgpu | unix.
    /// `auto` tries /dev/wl0 first (Crostini), then virtio-gpu cross-domain,
    /// then a local compositor socket.
    #[arg(long, default_value = "auto", env = "SOMMELIER_TRANSPORT")]
    pub transport: String,

    /// virtio_wl device path (default /dev/wl0). `--virtwl-device` is the
    /// spelling used in mainline's README.md.
    #[arg(long, alias = "virtwl-device", env = "SOMMELIER_VIRTWL_DEVICE")]
    pub virtio_wl: Option<String>,

    /// Connect directly to a host Wayland compositor socket path (debug only).
    /// `--display` / `SOMMELIER_DISPLAY` is the mainline spelling and takes a
    /// display *name* instead of a full path.
    #[arg(long, env = "SOMMELIER_LOCAL_COMPOSITOR")]
    pub local_compositor: Option<String>,

    /// Prefer the virtio-gpu cross-domain backend (main branch behaviour).
    /// `SOMMELIER_VIRTGPU_CHANNEL` is read in `Config::from_args`; binding it
    /// directly to a clap bool would reject common string values such as "1".
    #[arg(long)]
    pub virtgpu_channel: bool,

    /// Force the no-op driver: no VM channel, require --local-compositor.
    #[arg(long)]
    pub noop_driver: bool,

    /// Force a DRM render node for the virtio-gpu backend.
    #[arg(long)]
    pub force_drm_device: Option<String>,

    // ---- ChromeOS / scaling ---------------------------------------------
    /// Content scale, clamped to [0.1, 10]. Kept as a string so we can apply
    /// mainline's `atof()` semantics (sommelier.cc:4404) instead of clap's
    /// stricter float parser.
    #[arg(long, env = "SOMMELIER_SCALE")]
    pub scale: Option<String>,

    /// Do not round the scale to an integer (direct-scale mode).
    /// `SOMMELIER_DIRECT_SCALE` is read in `Config::from_args`.
    #[arg(long)]
    pub direct_scale: bool,

    /// Stable scaling (integer-aligned coordinates).
    /// `SOMMELIER_STABLE_SCALING` is read in `Config::from_args`.
    #[arg(long)]
    pub stable_scaling: bool,

    /// Comma-separated DPI buckets, e.g. "96,144,192".
    #[arg(long, env = "SOMMELIER_DPI")]
    pub dpi: Option<String>,

    /// Use wp_viewport to scale non-resizable windows.
    #[arg(long)]
    pub viewport_resize: bool,

    /// Accelerators reserved by the host (ChromeOS shortcuts).
    #[arg(long, env = "SOMMELIER_ACCELERATORS")]
    pub accelerators: Option<String>,

    /// Accelerators reserved only while not fullscreen.
    #[arg(
        long,
        alias = "windowed_accelerators",
        env = "SOMMELIER_WINDOWED_ACCELERATORS"
    )]
    pub windowed_accelerators: Option<String>,

    /// VM identifier used in `org.chromium.guest_os.<id>.wayland.<app_id>`.
    #[arg(long, env = "SOMMELIER_VM_IDENTIFIER")]
    pub vm_identifier: Option<String>,

    /// Default aura-shell fullscreen mode: immersive | plain.
    #[arg(long, env = "SOMMELIER_FULLSCREEN_MODE")]
    pub fullscreen_mode: Option<String>,

    /// Frame colour for X11 windows (#RRGGBB).
    #[arg(long, env = "SOMMELIER_FRAME_COLOR")]
    pub frame_color: Option<String>,

    /// Dark frame colour for X11 windows (#RRGGBB).
    #[arg(long, env = "SOMMELIER_DARK_FRAME_COLOR")]
    pub dark_frame_color: Option<String>,

    // ---- X11 / XWayland --------------------------------------------------
    /// Enable X11 forwarding by launching Xwayland and acting as its WM.
    /// `SOMMELIER_X11` is read in `Config::from_args`.
    #[arg(short = 'X', long)]
    pub x11: bool,

    /// Force an X11 display number (also enables X11 forwarding).
    #[arg(long)]
    pub x_display: Option<u32>,

    /// Path to the Xwayland binary.
    #[arg(long, env = "SOMMELIER_XWAYLAND_PATH")]
    pub xwayland_path: Option<String>,

    /// Command prefix for Xwayland (e.g. gdb --args).
    #[arg(long, env = "SOMMELIER_XWAYLAND_CMD_PREFIX")]
    pub xwayland_cmd_prefix: Option<String>,

    /// Xwayland -auth file.
    #[arg(long, env = "SOMMELIER_XAUTH_PATH")]
    pub x_auth: Option<String>,

    /// Xwayland -fp font path.
    #[arg(long, env = "SOMMELIER_XFONT_PATH")]
    pub x_font_path: Option<String>,

    /// LIBGL_DRIVERS_PATH for Xwayland.
    #[arg(long)]
    pub xwayland_gl_driver_path: Option<String>,

    /// Enable XShape support for X11 windows.
    #[arg(long)]
    pub enable_xshape: bool,

    /// Disable the X11 clipboard manager.
    /// Mainline `SOMMELIER_CLIPBOARD_MANAGER` is a *string* where only a
    /// literal "0" disables the manager; it is handled in `Config::from_args`.
    #[arg(long)]
    pub no_clipboard_manager: bool,

    /// Allow X11 clients to position their own windows.
    #[arg(long)]
    pub enable_x11_move_windows: bool,

    /// Allow `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`.
    #[arg(long)]
    pub allow_xwayland_emulate_screen_pos_size: bool,

    /// Use glamor accelerated Xwayland (drop -shm).
    /// Mainline `SOMMELIER_GLAMOR` is a *string* where only "0" disables
    /// glamor; it is handled in `Config::from_args` (sommelier.cc:4085-4089).
    #[arg(long)]
    pub glamor: bool,

    // ---- protocol / feature switches ------------------------------------
    /// Advertise zwp_linux_dmabuf_v1 to guests (experimental).
    #[arg(long, hide = true)]
    pub gpu_accel: bool,

    /// Enable linux-dmabuf explicit fence support.
    #[arg(long)]
    pub explicit_fence: bool,

    /// Disable server-side decorations via xdg-decoration.
    #[arg(long)]
    pub disable_xdg_decoration: bool,

    /// Do not advertise wl_surface.damage_buffer.
    #[arg(long)]
    pub no_support_damage_buffer: bool,

    /// Comma-separated quirk config files (textproto-like syntax).
    #[arg(long, env = "SOMMELIER_QUIRKS_CONFIG")]
    pub quirks_config: Option<String>,

    /// Print quirks enabled for a Steam game id and exit.
    #[arg(long)]
    pub print_enabled_features: Option<u32>,

    // ---- process behaviour ----------------------------------------------
    /// Run as parent: accept connections and spawn a worker per client.
    #[arg(long)]
    pub parent: bool,

    /// Do not exit when the launched child exits.
    #[arg(long)]
    pub no_exit_with_child: bool,

    /// systemd readiness notification. Mainline takes an optional value
    /// (`--sd-notify=...`, sommelier.cc:4294); a bare flag is also accepted.
    #[arg(
        long,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "READY=1"
    )]
    pub sd_notify: Option<String>,

    /// Log level 0..3 (higher = quieter).
    #[arg(long, env = "SOMMELIER_LOG_LEVEL")]
    pub log_level: Option<i32>,

    /// Optional program to launch after the proxy is ready.
    ///
    /// Mainline stops parsing options at the first positional argument and
    /// treats everything after it (including things starting with `-`) as
    /// `[program] [args...]` (sommelier.cc:4354-4364). `trailing_var_arg`
    /// reproduces that exact behaviour.
    #[arg(
        value_name = "PROGRAM",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub runprog: Vec<String>,

    // ---- mainline compatibility flags -----------------------------------
    // These are accepted with the same spelling as upstream sommelier. Flags
    // whose behaviour we do not implement yet are parsed and logged so an
    // upstream-style command line does not fail to start.
    /// Mainline spelling of `--gpu-accel` (sommelier.cc:4318).
    #[arg(long)]
    pub enable_linux_dmabuf: bool,

    /// Force an application id for every X11 window (highest priority).
    #[arg(long)]
    pub application_id: Option<String>,

    /// CARDINAL X11 property used to derive an application id.
    #[arg(long)]
    pub application_id_x11_property: Option<String>,

    /// Mainline parent/child plumbing: peer process id.
    #[arg(long)]
    pub peer_pid: Option<i32>,

    /// Mainline parent/child plumbing: command prefix for the peer process.
    #[arg(long)]
    pub peer_cmd_prefix: Option<String>,

    /// Mainline parent/child plumbing: pre-connected client fd.
    #[arg(long)]
    pub client_fd: Option<i32>,

    /// Frame timing log (optional feature, accepted for compatibility).
    #[arg(long)]
    pub timing_filename: Option<String>,

    /// Frame stats summary / log paths.
    #[arg(long)]
    pub stats_summary: Option<String>,
    #[arg(long)]
    pub stats_log: Option<String>,
    #[arg(long)]
    pub stats_timer: Option<u32>,

    /// Perfetto tracing (optional feature, accepted for compatibility).
    #[arg(long)]
    pub trace_filename: Option<String>,
    #[arg(long)]
    pub trace_system: bool,

    /// Mainline xdg-shell behaviour switches.
    #[arg(long)]
    pub ignore_stateless_toplevel_configure: bool,
    #[arg(long)]
    pub only_client_can_exit_fullscreen: bool,
}

fn env_bool(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// C `atoi` semantics, matching mainline's DPI bucket parser
/// (sommelier.cc:4519-4532): leading whitespace/sign/digits are consumed,
/// trailing junk is ignored, and a non-numeric token becomes 0 (then clamps
/// to MIN_DPI). `strtok` splits on commas and keeps empty tokens out.
fn atoi_prefix(s: &str) -> i64 {
    let t = s.trim();
    let bytes = t.as_bytes();
    let mut i = 0usize;
    let mut negative = false;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        negative = bytes[i] == b'-';
        i += 1;
    }
    let start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if start == i {
        return 0;
    }
    let value: i64 = t[start..i].parse().unwrap_or(i64::MAX);
    if negative {
        -value
    } else {
        value
    }
}

fn parse_dpi(s: &str) -> Vec<u32> {
    s.split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| atoi_prefix(part).clamp(72, 9600) as u32)
        .collect()
}

/// C `atof` semantics for `--scale` (sommelier.cc:4404): a leading floating
/// point literal is accepted, trailing junk ignored, junk-only input is 0.
fn atof_prefix(s: &str) -> f64 {
    let t = s.trim();
    let bytes = t.as_bytes();
    let mut i = 0usize;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut seen_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        seen_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            seen_digit = true;
        }
    }
    if !seen_digit {
        return 0.0;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    t[..i].parse().unwrap_or(0.0)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_name: String,
    pub socket_path: PathBuf,
    pub transport: TransportKind,
    pub virtwl_device: String,
    pub local_compositor: Option<String>,
    pub gpu_accel: bool,
    pub xdg_decoration: bool,
    pub damage_buffer: bool,
    pub scale: f64,
    /// The raw `--scale` value. Mainline keeps this in `ctx.desired_scale`
    /// and drops the integer rounding once the host advertises wp_viewporter
    /// (sommelier.cc:657).
    pub desired_scale: f64,
    pub scaling_mode: ScalingMode,
    pub dpi_buckets: Vec<u32>,
    pub viewport_resize: bool,
    pub vm_identifier: String,
    /// `--application-id`: forces the same app id on every X11 window.
    pub application_id: Option<String>,
    /// `--application-id-x11-property`: CARDINAL property used for app ids.
    pub application_id_property: Option<String>,
    pub fullscreen_mode: String,
    pub force_drm_device: Option<String>,
    pub explicit_fence: bool,
    pub x11_enabled: bool,
    pub x11: X11Options,
    pub quirks: Arc<Quirks>,
    pub log_level: i32,
    pub parent: bool,
    pub exit_with_child: bool,
    pub sd_notify: bool,
    pub runprog: Vec<String>,
}

impl Config {
    pub fn from_args(args: &Args) -> Self {
        let xdg_runtime = env_string("XDG_RUNTIME_DIR")
            .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }));
        // Mainline: `--socket` names the listening socket (default wayland-0),
        // `--display` names the *host* compositor. The `SOMMELIER_SOCKET`
        // environment variable is our own extension for the same thing.
        let socket_name = args
            .socket
            .clone()
            .or_else(|| env_string("SOMMELIER_SOCKET"))
            .unwrap_or_else(|| "wayland-0".to_string());
        let socket_path = PathBuf::from(&xdg_runtime).join(&socket_name);

        let direct = args.direct_scale || env_bool("SOMMELIER_DIRECT_SCALE");
        let stable = args.stable_scaling || env_bool("SOMMELIER_STABLE_SCALING");
        let scaling_mode = if stable {
            ScalingMode::Stable
        } else if direct {
            ScalingMode::Direct
        } else {
            ScalingMode::Normal
        };

        // sommelier.cc:4403-4413 — until wp_viewporter is seen the effective
        // scale is rounded to an integer in normal/stable mode; direct-scale
        // keeps the fractional value. `desired_scale` is retained so the
        // registry handler can apply sommelier.cc:657 later.
        let scale_text = args
            .scale
            .clone()
            .or_else(|| env_string("SOMMELIER_SCALE"))
            .unwrap_or_else(|| "1.0".to_string());
        let desired_scale = atof_prefix(&scale_text);
        let raw_scale = if direct {
            desired_scale
        } else {
            desired_scale.round()
        };
        let scale = if raw_scale.is_finite() {
            raw_scale.clamp(0.1, 10.0)
        } else {
            1.0
        };
        let desired_scale = if desired_scale.is_finite() {
            desired_scale.clamp(0.1, 10.0)
        } else {
            1.0
        };

        let dpi_from_env = env_string("SOMMELIER_DPI");
        let dpi_buckets = args
            .dpi
            .as_deref()
            .or(dpi_from_env.as_deref())
            .map(parse_dpi)
            .unwrap_or_default();

        let x11_enabled = args.x11 || args.x_display.is_some() || env_bool("SOMMELIER_X11");
        let vm_identifier = args
            .vm_identifier
            .clone()
            .or_else(|| env_string("SOMMELIER_VM_IDENTIFIER"))
            .unwrap_or_else(|| "termina".to_string());

        // sommelier.cc:4085-4089 — `-shm` is added unless a DRM device is
        // present AND glamor is enabled. Mainline treats SOMMELIER_GLAMOR as
        // a string and only "0" disables it; `--glamor` forces "1".
        let glamor_env = std::env::var("SOMMELIER_GLAMOR").ok();
        let glamor = if args.glamor {
            true
        } else {
            glamor_env
                .as_deref()
                .is_some_and(|v| v != "0" && !v.is_empty())
        };
        let has_render_node =
            std::path::Path::new("/dev/dri/renderD128").exists() || args.force_drm_device.is_some();
        let use_shm = !(glamor && has_render_node);

        // sommelier.cc:4394-4401 — clipboard manager is on by default in X11
        // mode and only a literal "0" in SOMMELIER_CLIPBOARD_MANAGER (or the
        // flag) disables it.
        let clipboard_manager = x11_enabled
            && !args.no_clipboard_manager
            && std::env::var("SOMMELIER_CLIPBOARD_MANAGER")
                .map(|v| v != "0")
                .unwrap_or(true);

        // sommelier.cc:4437-4438 — SOMMELIER_SUPPORT_DAMAGE_BUFFER is "1"
        // by default; any other explicit value disables it.
        let damage_buffer = !args.no_support_damage_buffer
            && std::env::var("SOMMELIER_SUPPORT_DAMAGE_BUFFER")
                .map(|v| v == "1")
                .unwrap_or(true);

        let cmd_prefix_env = env_string("SOMMELIER_XWAYLAND_CMD_PREFIX");
        let xwayland_path_env = env_string("SOMMELIER_XWAYLAND_PATH");
        let x_auth_env = env_string("SOMMELIER_XAUTH_PATH");
        let x_font_path_env = env_string("SOMMELIER_XFONT_PATH");
        let gl_driver_path = args
            .xwayland_gl_driver_path
            .clone()
            .or_else(|| env_string("SOMMELIER_XWAYLAND_GL_DRIVER_PATH"));

        let x11 = X11Options {
            xwayland_path: args
                .xwayland_path
                .clone()
                .or(xwayland_path_env)
                .unwrap_or_else(|| DEFAULT_XWAYLAND_PATH.to_string()),
            xwayland_cmd_prefix: args
                .xwayland_cmd_prefix
                .as_deref()
                .or(cmd_prefix_env.as_deref())
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            x_display: args.x_display,
            x_auth: args.x_auth.clone().or(x_auth_env),
            x_font_path: args.x_font_path.clone().or(x_font_path_env),
            gl_driver_path,
            enable_xshape: args.enable_xshape,
            clipboard_manager,
            shm: use_shm,
            scale,
            // Mainline X11 app ids are built as
            // org.chromium.guest_os.<vm>.<wmclass|xid|wmclientleader|xprop>.<id>
            // (sommelier-window.cc:29-35). `.wayland.` is only for native
            // Wayland clients, so the WM gets the prefix plus the format kind.
            app_id_prefix: format!("org.chromium.guest_os.{}.", vm_identifier),
            application_id: args.application_id.clone(),
            application_id_property: args.application_id_x11_property.clone(),
            frame_color: args
                .frame_color
                .clone()
                .unwrap_or_else(|| "#f2f2f2".to_string()),
            dark_frame_color: args
                .dark_frame_color
                .clone()
                .unwrap_or_else(|| "#323639".to_string()),
            allow_randr_emu: args.allow_xwayland_emulate_screen_pos_size,
        };

        let quirks_env = env_string("SOMMELIER_QUIRKS_CONFIG");
        let quirks = match args.quirks_config.as_deref().or(quirks_env.as_deref()) {
            Some(paths) => match Quirks::load_comma_separated_files(paths) {
                Ok(q) => Arc::new(q),
                Err(e) => {
                    log::error!("Failed to load quirks config '{}': {}", paths, e);
                    Arc::new(Quirks::new())
                }
            },
            None => Arc::new(Quirks::new()),
        };

        // Transport resolution. Crostini always has /dev/wl0; a generic VMM
        // has a virtgpu render node instead. `auto` mirrors mainline
        // sommelier's "try virtgpu if asked, else virtwl, else direct socket".
        let virtwl_device = args
            .virtio_wl
            .clone()
            .or_else(|| env_string("SOMMELIER_VIRTWL_DEVICE"))
            .unwrap_or_else(|| DEFAULT_VIRTWL_DEVICE.to_string());

        // sommelier.cc:4464-4466 — an explicit --display means "connect to a
        // real host compositor socket", i.e. no virtual Wayland context.
        let host_display = args.display.clone().filter(|s| !s.is_empty());
        let local_compositor = if let Some(name) = &host_display {
            Some(if name.starts_with('/') {
                name.clone()
            } else {
                format!("{}/{}", xdg_runtime, name)
            })
        } else {
            args.local_compositor.clone().or_else(|| {
                env_string("WAYLAND_DISPLAY")
                    .filter(|_| args.transport == "unix" || args.noop_driver)
                    .map(|d| {
                        if d.starts_with('/') {
                            d
                        } else {
                            format!("{}/{}", xdg_runtime, d)
                        }
                    })
            })
        };

        let transport = if host_display.is_some() {
            TransportKind::Unix
        } else {
            resolve_transport(
                &args.transport,
                args.virtgpu_channel || env_bool("SOMMELIER_VIRTGPU_CHANNEL"),
                args.noop_driver,
                &virtwl_device,
                local_compositor.as_deref(),
            )
        };

        Self {
            socket_name,
            socket_path,
            transport,
            virtwl_device,
            local_compositor,
            gpu_accel: args.gpu_accel || args.enable_linux_dmabuf,
            xdg_decoration: !args.disable_xdg_decoration,
            damage_buffer,
            scale,
            desired_scale,
            scaling_mode,
            dpi_buckets,
            viewport_resize: args.viewport_resize,
            vm_identifier,
            application_id: args.application_id.clone(),
            application_id_property: args.application_id_x11_property.clone(),
            fullscreen_mode: args
                .fullscreen_mode
                .clone()
                .unwrap_or_else(|| "immersive".to_string()),
            force_drm_device: args.force_drm_device.clone(),
            explicit_fence: args.explicit_fence,
            x11_enabled,
            x11,
            quirks,
            log_level: args.log_level.unwrap_or(0),
            parent: args.parent,
            exit_with_child: !args.no_exit_with_child,
            sd_notify: args.sd_notify.is_some(),
            runprog: args.runprog.clone(),
        }
    }

    /// Mainline rejects an unrecognised `--fullscreen-mode` with a fatal
    /// error (sommelier.cc:4446-4456). Do the same before starting the proxy.
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.fullscreen_mode.as_str(), "immersive" | "plain") {
            return Err(format!(
                "unrecognised --fullscreen-mode '{}' (expected immersive|plain)",
                self.fullscreen_mode
            ));
        }
        Ok(())
    }

    pub fn transform(&self) -> Transform {
        Transform::new(self.scale, self.scaling_mode, self.dpi_buckets.clone())
    }

    /// Adjust the effective transport using actual device availability.
    /// Called once at startup after logging.
    pub fn validate_transport(&mut self) {
        match self.transport {
            TransportKind::VirtWl => {
                if !std::path::Path::new(&self.virtwl_device).exists() {
                    log::warn!(
                        "{} not found; falling back to {}",
                        self.virtwl_device,
                        if std::path::Path::new("/dev/dri/renderD128").exists() {
                            "virtio-gpu cross-domain"
                        } else {
                            "local compositor socket"
                        }
                    );
                    self.transport = if std::path::Path::new("/dev/dri/renderD128").exists() {
                        TransportKind::VirtGpu
                    } else {
                        TransportKind::Unix
                    };
                }
            }
            TransportKind::VirtGpu => {
                if !std::path::Path::new("/dev/dri/renderD128").exists() {
                    log::warn!("No render node; falling back to {}", self.virtwl_device);
                    self.transport = TransportKind::VirtWl;
                }
            }
            TransportKind::Unix => {
                if self.local_compositor.is_none() {
                    log::warn!(
                        "--transport=unix without --local-compositor; proxy will have no host"
                    );
                }
            }
        }
    }
}

fn resolve_transport(
    requested: &str,
    prefer_virtgpu: bool,
    noop: bool,
    virtwl_device: &str,
    local: Option<&str>,
) -> TransportKind {
    if noop || requested == "unix" {
        return TransportKind::Unix;
    }
    if requested == "virtwl" {
        return TransportKind::VirtWl;
    }
    if requested == "virtgpu" || prefer_virtgpu {
        return TransportKind::VirtGpu;
    }
    // auto
    if std::path::Path::new(virtwl_device).exists() {
        TransportKind::VirtWl
    } else if std::path::Path::new("/dev/dri/renderD128").exists() {
        TransportKind::VirtGpu
    } else if local.is_some() {
        TransportKind::Unix
    } else {
        // Default to virtwl: on Crostini the device appears slightly after
        // sommelier starts (systemd socket ordering), so failing hard here
        // would be wrong. `validate_transport` warns and falls back.
        TransportKind::VirtWl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Args {
        let mut v = vec!["sommelier-rs-r"];
        v.extend_from_slice(extra);
        Args::parse_from(v)
    }

    #[test]
    fn mainline_string_envs_are_not_bound_to_bool_flags() {
        use clap::CommandFactory;
        // Crostini's systemd units export SOMMELIER_GLAMOR="1" and may export
        // SOMMELIER_CLIPBOARD_MANAGER="1". Mainline treats these as strings,
        // not booleans. Binding them straight to a clap bool makes clap abort
        // startup with: invalid value '1' ... [possible values: true, false].
        let cmd = Args::command();
        for id in [
            "glamor",
            "no_clipboard_manager",
            "direct_scale",
            "stable_scaling",
            "x11",
            "virtgpu_channel",
        ] {
            let arg = cmd
                .get_arguments()
                .find(|a| a.get_id() == id)
                .unwrap_or_else(|| panic!("missing clap arg {id}"));
            assert!(
                arg.get_env().is_none(),
                "{id} must not bind a string env var to a bool flag"
            );
        }
        // The flags themselves remain usable.
        assert!(args(&["--glamor"]).glamor);
        assert!(!args(&[]).glamor);
    }

    #[test]
    fn x_display_implies_x11() {
        let cfg = Config::from_args(&args(&["--x-display=3"]));
        assert!(cfg.x11_enabled);
        assert_eq!(cfg.x11.x_display, Some(3));
    }

    #[test]
    fn socket_and_display_follow_mainline_semantics() {
        assert_eq!(Config::from_args(&args(&[])).socket_name, "wayland-0");
        assert_eq!(
            Config::from_args(&args(&["--socket=wl-7"])).socket_name,
            "wl-7"
        );
        // --display is the HOST display, not the listening socket.
        let cfg = Config::from_args(&args(&["--display=wayland-9"]));
        assert_eq!(cfg.socket_name, "wayland-0");
        assert_eq!(cfg.transport, TransportKind::Unix);
        assert!(cfg
            .local_compositor
            .as_deref()
            .is_some_and(|p| p.ends_with("/wayland-9")));
        // Mainline stops option parsing at the first positional argument.
        let cfg = Config::from_args(&args(&["--scale=1.5", "weston-terminal", "--foo"]));
        assert_eq!(cfg.runprog, vec!["weston-terminal", "--foo"]);
        assert_eq!(cfg.socket_name, "wayland-0");
    }

    #[test]
    fn mainline_flags_are_accepted() {
        let cfg = Config::from_args(&args(&[
            "--enable-linux-dmabuf",
            "--application-id=org.chromium.test",
            "--application-id-x11-property=STEAM_GAME_ID",
            "--peer-pid=42",
            "--client-fd=7",
            "--sd-notify",
            "--windowed_accelerators=<Control><Alt>F",
            "--ignore-stateless-toplevel-configure",
        ]));
        assert!(cfg.gpu_accel);
        assert!(cfg.sd_notify);
        assert_eq!(cfg.application_id.as_deref(), Some("org.chromium.test"));
    }

    #[test]
    fn scale_is_clamped() {
        let cfg = Config::from_args(&args(&["--scale=99"]));
        assert_eq!(cfg.scale, 10.0);
        let cfg = Config::from_args(&args(&["--scale=0.001"]));
        assert_eq!(cfg.scale, 0.1);
    }

    #[test]
    fn scale_rounds_to_integer_unless_direct() {
        // sommelier.cc:4408-4411: round() before wp_viewporter is seen.
        let cfg = Config::from_args(&args(&["--scale=1.5"]));
        assert_eq!(cfg.scale, 2.0);
        assert_eq!(cfg.desired_scale, 1.5);
        let cfg = Config::from_args(&args(&["--scale=1.5", "--direct-scale"]));
        assert_eq!(cfg.scale, 1.5);
    }

    #[test]
    fn fullscreen_mode_is_validated() {
        assert!(Config::from_args(&args(&["--fullscreen-mode=plain"]))
            .validate()
            .is_ok());
        assert!(Config::from_args(&args(&["--fullscreen-mode=nonsense"]))
            .validate()
            .is_err());
    }

    #[test]
    fn scaling_mode_precedence() {
        let cfg = Config::from_args(&args(&["--direct-scale", "--stable-scaling"]));
        assert_eq!(cfg.scaling_mode, ScalingMode::Stable);
        let cfg = Config::from_args(&args(&["--direct-scale"]));
        assert_eq!(cfg.scaling_mode, ScalingMode::Direct);
    }

    #[test]
    fn dpi_uses_mainline_atoi_semantics() {
        // sommelier.cc:4519-4532 — strtok + atoi + clamp(72,9600). A junk
        // token is 0 and therefore becomes the 72 DPI bucket, not dropped.
        assert_eq!(
            parse_dpi("96, 144, nonsense, 3, 99999"),
            vec![96, 144, 72, 72, 9600]
        );
        assert_eq!(parse_dpi("96.5,-20,1e3"), vec![96, 72, 72]);
    }

    #[test]
    fn scale_uses_mainline_atof_semantics() {
        assert_eq!(atof_prefix("1.5"), 1.5);
        assert_eq!(atof_prefix("1.5junk"), 1.5);
        assert_eq!(atof_prefix("  -0.25 "), -0.25);
        assert_eq!(atof_prefix("2e-1"), 0.2);
        assert_eq!(atof_prefix("nonsense"), 0.0);
        let cfg = Config::from_args(&args(&["--scale=1.5"]));
        assert_eq!(cfg.desired_scale, 1.5);
    }

    #[test]
    fn app_id_prefix_uses_vm_identifier() {
        let cfg = Config::from_args(&args(&["--vm-identifier=penguin"]));
        assert_eq!(cfg.x11.app_id_prefix, "org.chromium.guest_os.penguin.");
    }

    #[test]
    fn decoration_defaults_on_and_can_be_disabled() {
        assert!(Config::from_args(&args(&[])).xdg_decoration);
        assert!(!Config::from_args(&args(&["--disable-xdg-decoration"])).xdg_decoration);
    }

    #[test]
    fn glamor_without_render_node_keeps_shm() {
        let cfg = Config::from_args(&args(&["--glamor"]));
        assert!(cfg.x11.shm || std::path::Path::new("/dev/dri/renderD128").exists());
    }
}
