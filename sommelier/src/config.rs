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
    /// Wayland display name (socket under $XDG_RUNTIME_DIR) to listen on.
    #[arg(long = "display")]
    pub display: Option<String>,

    /// Positional form of `--display`, matching both mainline sommelier and
    /// the `virtwl` branch (`sommelier-rs-r [options] wayland-0`).
    #[arg(value_name = "DISPLAY")]
    pub display_pos: Option<String>,

    // ---- host transport -------------------------------------------------
    /// Host transport: auto | virtwl | virtgpu | unix.
    /// `auto` tries /dev/wl0 first (Crostini), then virtio-gpu cross-domain,
    /// then a local compositor socket.
    #[arg(long, default_value = "auto", env = "SOMMELIER_TRANSPORT")]
    pub transport: String,

    /// virtio_wl device path (default /dev/wl0).
    #[arg(long, env = "SOMMELIER_VIRTWL_DEVICE")]
    pub virtio_wl: Option<String>,

    /// Connect directly to a host Wayland compositor socket (debug only).
    #[arg(long, env = "SOMMELIER_DISPLAY")]
    pub local_compositor: Option<String>,

    /// Prefer the virtio-gpu cross-domain backend (main branch behaviour).
    #[arg(long, env = "SOMMELIER_VIRTGPU_CHANNEL")]
    pub virtgpu_channel: bool,

    /// Force the no-op driver: no VM channel, require --local-compositor.
    #[arg(long)]
    pub noop_driver: bool,

    /// Force a DRM render node for the virtio-gpu backend.
    #[arg(long)]
    pub force_drm_device: Option<String>,

    // ---- ChromeOS / scaling ---------------------------------------------
    /// Content scale, clamped to [0.1, 10].
    #[arg(long, env = "SOMMELIER_SCALE")]
    pub scale: Option<f64>,

    /// Do not round the scale to an integer (direct-scale mode).
    #[arg(long, env = "SOMMELIER_DIRECT_SCALE")]
    pub direct_scale: bool,

    /// Stable scaling (integer-aligned coordinates).
    #[arg(long, env = "SOMMELIER_STABLE_SCALING")]
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
    #[arg(long, env = "SOMMELIER_WINDOWED_ACCELERATORS")]
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
    #[arg(short = 'X', long, env = "SOMMELIER_X11")]
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
    #[arg(long, env = "SOMMELIER_CLIPBOARD_MANAGER")]
    pub no_clipboard_manager: bool,

    /// Allow X11 clients to position their own windows.
    #[arg(long)]
    pub enable_x11_move_windows: bool,

    /// Allow `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`.
    #[arg(long)]
    pub allow_xwayland_emulate_screen_pos_size: bool,

    /// Use glamor accelerated Xwayland (drop -shm).
    #[arg(long, env = "SOMMELIER_GLAMOR")]
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

    /// systemd readiness notification (NOTIFY_SOCKET).
    #[arg(long)]
    pub sd_notify: bool,

    /// Log level 0..3 (higher = quieter).
    #[arg(long, env = "SOMMELIER_LOG_LEVEL")]
    pub log_level: Option<i32>,

    /// Optional program to launch after the proxy is ready.
    #[arg(last = true)]
    pub runprog: Vec<String>,
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

fn parse_dpi(s: &str) -> Vec<u32> {
    s.split(',')
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .map(|v| v.clamp(72, 9600))
        .collect()
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
    pub scaling_mode: ScalingMode,
    pub dpi_buckets: Vec<u32>,
    pub viewport_resize: bool,
    pub vm_identifier: String,
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
        // Priority: explicit --display > positional DISPLAY > SOMMELIER_SOCKET
        // > default. Keeping the env lookup here (rather than as a clap `env`)
        // is what guarantees "command line overrides environment".
        let socket_name = args
            .display
            .clone()
            .or_else(|| args.display_pos.clone())
            .or_else(|| env_string("SOMMELIER_SOCKET"))
            .unwrap_or_else(|| "wayland-0".to_string());
        let socket_path = PathBuf::from(&xdg_runtime).join(&socket_name);

        let scale = args
            .scale
            .or_else(|| env_string("SOMMELIER_SCALE").and_then(|s| s.parse().ok()))
            .unwrap_or(1.0)
            .clamp(0.1, 10.0);

        let direct = args.direct_scale || env_bool("SOMMELIER_DIRECT_SCALE");
        let stable = args.stable_scaling || env_bool("SOMMELIER_STABLE_SCALING");
        let scaling_mode = if stable {
            ScalingMode::Stable
        } else if direct {
            ScalingMode::Direct
        } else {
            ScalingMode::Normal
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

        let glamor = args.glamor || env_bool("SOMMELIER_GLAMOR");
        let has_render_node = std::path::Path::new("/dev/dri/renderD128").exists();
        let use_shm = !(glamor && has_render_node);

        let cmd_prefix_env = env_string("SOMMELIER_XWAYLAND_CMD_PREFIX");
        let xwayland_path_env = env_string("SOMMELIER_XWAYLAND_PATH");
        let x_auth_env = env_string("SOMMELIER_XAUTH_PATH");
        let x_font_path_env = env_string("SOMMELIER_XFONT_PATH");

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
            gl_driver_path: args.xwayland_gl_driver_path.clone(),
            enable_xshape: args.enable_xshape,
            clipboard_manager: x11_enabled && !args.no_clipboard_manager,
            shm: use_shm,
            scale,
            app_id_prefix: format!("org.chromium.guest_os.{}.wayland.", vm_identifier),
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

        let local_compositor = args
            .local_compositor
            .clone()
            .or_else(|| env_string("WAYLAND_DISPLAY").map(|d| format!("{}/{}", xdg_runtime, d)));

        let transport = resolve_transport(
            &args.transport,
            args.virtgpu_channel,
            args.noop_driver,
            &virtwl_device,
            local_compositor.as_deref(),
        );

        Self {
            socket_name,
            socket_path,
            transport,
            virtwl_device,
            local_compositor,
            gpu_accel: args.gpu_accel,
            xdg_decoration: !args.disable_xdg_decoration,
            damage_buffer: !args.no_support_damage_buffer,
            scale,
            scaling_mode,
            dpi_buckets,
            viewport_resize: args.viewport_resize,
            vm_identifier,
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
            sd_notify: args.sd_notify,
            runprog: args.runprog.clone(),
        }
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
    fn x_display_implies_x11() {
        let cfg = Config::from_args(&args(&["--x-display=3"]));
        assert!(cfg.x11_enabled);
        assert_eq!(cfg.x11.x_display, Some(3));
    }

    #[test]
    fn display_accepts_named_and_positional_forms() {
        assert_eq!(Config::from_args(&args(&[])).socket_name, "wayland-0");
        assert_eq!(
            Config::from_args(&args(&["--display=wl-7"])).socket_name,
            "wl-7"
        );
        assert_eq!(Config::from_args(&args(&["wl-3"])).socket_name, "wl-3");
        // Named form wins over the positional alias.
        assert_eq!(
            Config::from_args(&args(&["wl-3", "--display=wl-7"])).socket_name,
            "wl-7"
        );
    }

    #[test]
    fn scale_is_clamped() {
        let cfg = Config::from_args(&args(&["--scale=99"]));
        assert_eq!(cfg.scale, 10.0);
        let cfg = Config::from_args(&args(&["--scale=0.001"]));
        assert_eq!(cfg.scale, 0.1);
    }

    #[test]
    fn scaling_mode_precedence() {
        let cfg = Config::from_args(&args(&["--direct-scale", "--stable-scaling"]));
        assert_eq!(cfg.scaling_mode, ScalingMode::Stable);
        let cfg = Config::from_args(&args(&["--direct-scale"]));
        assert_eq!(cfg.scaling_mode, ScalingMode::Direct);
    }

    #[test]
    fn dpi_is_clamped_and_filtered() {
        assert_eq!(
            parse_dpi("96, 144, nonsense, 3, 99999"),
            vec![96, 144, 72, 9600]
        );
    }

    #[test]
    fn app_id_prefix_uses_vm_identifier() {
        let cfg = Config::from_args(&args(&["--vm-identifier=penguin"]));
        assert_eq!(
            cfg.x11.app_id_prefix,
            "org.chromium.guest_os.penguin.wayland."
        );
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
