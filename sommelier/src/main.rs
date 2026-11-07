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

use clap::Parser;
use std::os::unix::net::UnixDatagram;
use std::process::{Command, Stdio};

mod accelerator;
mod allocator;
mod config;
mod connection;
mod handler;
mod proxy;
mod quirks;
mod state;
mod transform;
mod virtgpu_channel;
mod virtwl;
mod virtwl_channel;
mod wire;
mod x11;

mod protocols {
    #![allow(unused_macros)]
    include!(concat!(env!("OUT_DIR"), "/all_includes.rs"));
}

/// Map mainline sommelier's `--log-level` (higher = quieter) onto an
/// `env_logger` filter. `RUST_LOG`, when present, still wins.
fn log_filter(log_level: i32) -> &'static str {
    match log_level {
        ..=0 => "info",
        1 => "warn",
        2 => "error",
        _ => "off",
    }
}

/// Notify systemd that the compositor socket is ready. Called from
/// `proxy::run` *after* the listener has been bound.
pub fn notify_systemd() {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    match UnixDatagram::unbound().and_then(|sock| sock.send_to(b"READY=1", &path).map(|_| ())) {
        Ok(()) => log::debug!("signalled READY=1 to {}", path),
        Err(e) => log::warn!("sd_notify to {} failed: {}", path, e),
    }
}

/// Launch the trailing `runprog`, if any, with the environment pointing at
/// our own socket / X display.
fn spawn_runprog(cfg: &config::Config) -> Option<std::process::Child> {
    let (program, args) = cfg.runprog.split_first()?;
    let mut cmd = Command::new(program);
    cmd.args(args);
    if cfg.x11_enabled {
        // Mainline sommelier.cc:3630-3635 — X11 programs must not accidentally
        // fall back to a Wayland display, so a guaranteed-invalid name is used.
        cmd.env("WAYLAND_DISPLAY", ".");
    } else {
        // Mainline sommelier.cc:4610-4613 — without X11 forwarding DISPLAY is
        // unset so clients do not try to connect to a non-existent X server.
        cmd.env_remove("DISPLAY");
        cmd.env("WAYLAND_DISPLAY", &cfg.socket_name);
    }
    if let Some(parent) = cfg.socket_path.parent() {
        cmd.env("XDG_RUNTIME_DIR", parent);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    match cmd.spawn() {
        Ok(child) => {
            log::info!("Launched child {} (pid {})", program, child.id());
            Some(child)
        }
        Err(e) => {
            log::error!("Failed to launch {}: {}", program, e);
            None
        }
    }
}

// SAFETY INVARIANT for `unsafe impl Send for KeyboardHandler` (keyboard.rs):
// KeyboardHandler holds xkb::Context/Keymap/State which are !Send. The impl is
// sound only when client tasks are never migrated across OS threads. We enforce
// this by using a single-threaded Tokio runtime — all tasks run on one thread,
// so no cross-thread migration can occur.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = config::Args::parse();
    let mut cfg = config::Config::from_args(&args);

    let env = env_logger::Env::default().default_filter_or(log_filter(cfg.log_level));
    env_logger::Builder::from_env(env).init();

    if let Err(e) = cfg.validate() {
        eprintln!("sommelier-rs-r: {}", e);
        std::process::exit(1);
    }
    // Mainline clears DISPLAY entirely when X11 forwarding is off.
    if !cfg.x11_enabled {
        std::env::remove_var("DISPLAY");
    }

    // Diagnostic mode used by ChromeOS integration tests / quirk debugging.
    if let Some(steam_game_id) = args.print_enabled_features {
        println!("{}", cfg.quirks.print_features(steam_game_id));
        return;
    }

    cfg.validate_transport();
    log::info!(
        "sommelier-rs-r: socket={} transport={} scale={} mode={:?} x11={} quirks={}",
        cfg.socket_path.display(),
        cfg.transport.as_str(),
        cfg.scale,
        cfg.scaling_mode,
        cfg.x11_enabled,
        cfg.quirks.enabled_features(0).len()
    );
    log::info!(
        "feature flags: damage_buffer={} viewport_resize={} fullscreen_mode={} explicit_fence={} force_drm_device={:?} application_id={:?} application_id_property={:?}",
        cfg.damage_buffer,
        cfg.viewport_resize,
        cfg.fullscreen_mode,
        cfg.explicit_fence,
        cfg.force_drm_device,
        cfg.application_id,
        cfg.application_id_property
    );

    // ---- X11 / XWayland -------------------------------------------------
    // A failure to start Xwayland must not take down the Wayland proxy: this
    // mirrors mainline sommelier, which keeps serving Wayland clients when
    // the X11 half is unavailable (e.g. missing Xwayland binary).
    let (bridge, wm_commands, xwayland_client_fd) = if cfg.x11_enabled {
        let (b, rx) = x11::X11Bridge::new();
        match x11::start_x11(cfg.x11.clone(), b.clone()) {
            Ok(runtime) => {
                let x11::X11Runtime {
                    display,
                    pid,
                    xwayland_client_fd,
                    wm_thread,
                } = runtime;
                log::info!(
                    "X11 forwarding ready on DISPLAY={} (Xwayland pid {})",
                    display,
                    pid
                );
                // Keep the WM thread joined by a watchdog so a panic in the
                // WM loop is reported instead of silently degrading X11.
                // Wayland clients keep working either way.
                if let Some(handle) = wm_thread {
                    let _ = std::thread::Builder::new()
                        .name("sommelier-x11-wm-watchdog".to_string())
                        .spawn(move || {
                            if handle.join().is_err() {
                                log::error!(
                                    "X11 WM thread panicked; X11 windows are no longer managed"
                                );
                            } else {
                                log::warn!("X11 WM thread exited");
                            }
                        });
                }
                (Some(b), Some(rx), Some(xwayland_client_fd))
            }
            Err(e) => {
                log::error!(
                    "X11 forwarding unavailable ({}); continuing Wayland-only",
                    e
                );
                (None, None, None)
            }
        }
    } else {
        (None, None, None)
    };

    // ---- trailing runprog -----------------------------------------------
    if let Some(mut child) = spawn_runprog(&cfg) {
        let exit_with_child = cfg.exit_with_child;
        tokio::task::spawn_blocking(move || {
            match child.wait() {
                Ok(status) => log::info!("Child exited with {}", status),
                Err(e) => log::warn!("Waiting for child failed: {}", e),
            }
            if exit_with_child {
                std::process::exit(0);
            }
        });
    }

    if cfg.parent {
        log::debug!("--parent: per-client tasks are spawned on the shared runtime");
    }

    proxy::run(cfg, bridge, wm_commands, xwayland_client_fd).await;
}

mod test_xkb;
