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

//! Output / scaling translation — the Rust counterpart of mainline
//! `sommelier-output.cc` + `sommelier-transform.cc`.
//!
//! `wl_output`, `zxdg_output_v1` and `wp_fractional_scale_v1` events describe
//! the *host* desktop. A guest must instead see geometry expressed in the
//! virtual coordinate system configured by `--scale`, `--dpi`,
//! `--direct-scale` and `--stable-scaling`. This handler drops the original
//! host event and re-emits a translated one via `ctx.host_to_client_queue`,
//! which `proxy::Client::handle_msgs()` flushes in order before the next
//! message is processed.

use std::collections::HashMap;

use crate::protocols::aura_shell::zaura_output::ZauraOutputHandler;
use crate::protocols::fractional_scale_v1::wp_fractional_scale_v1::{
    WpFractionalScaleV1Handler, EVT_PREFERRED_SCALE,
};
use crate::protocols::wayland::wl_output::{
    WlOutputHandler, EVT_DESCRIPTION, EVT_DONE as EVT_OUTPUT_DONE, EVT_GEOMETRY, EVT_MODE,
    EVT_NAME, EVT_SCALE,
};
use crate::protocols::xdg_output_unstable_v1::zxdg_output_v1::{
    ZxdgOutputV1Handler, EVT_LOGICAL_POSITION, EVT_LOGICAL_SIZE,
};
use crate::state::Context;
use crate::transform::OutputGeometry;
use crate::wire::{Action, MessageBuilder};

#[derive(Debug, Clone, Default)]
struct Geometry {
    x: i32,
    y: i32,
    width_mm: i32,
    height_mm: i32,
    subpixel: i32,
    make: String,
    model: String,
    transform: i32,
}

/// Cached state for one host `wl_output` (keyed by the host object id).
#[derive(Debug, Clone)]
struct OutputState {
    geometry: Option<Geometry>,
    /// `(flags, width, height, refresh)` from `wl_output.mode`.
    mode: Option<(u32, i32, i32, i32)>,
    host_scale: f64,
    /// `zaura_output.scale` with the CURRENT/PREFERRED flag, in real units
    /// (wire values are milli-units, sommelier-output.cc:28-33).
    current_scale: f64,
    preferred_scale: f64,
    /// `zaura_output.device_scale_factor`; `None` until the host sends it.
    /// Unlike `wl_output.scale` this may be fractional (e.g. 1.25).
    device_scale_factor: Option<f64>,
    /// `zaura_output.connection == INTERNAL`.
    internal: bool,
    /// `zxdg_output_v1.logical_*` (only bound with `--direct-scale`).
    logical_x: i32,
    logical_y: i32,
    logical_width: i32,
    logical_height: i32,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            geometry: None,
            mode: None,
            host_scale: 1.0,
            current_scale: 1.0,
            preferred_scale: 1.0,
            device_scale_factor: None,
            internal: false,
            logical_x: 0,
            logical_y: 0,
            logical_width: 0,
            logical_height: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct OutputHandler {
    outputs: HashMap<u32, OutputState>,
}

impl OutputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the collected output state back into `Transform` so that
    /// per-output coordinate/DPI lookups (and `virtual_dimensions()`) see
    /// the real host layout instead of an empty list.
    fn sync_transform(&self, ctx: &mut Context) {
        let mut outputs: Vec<OutputGeometry> = self
            .outputs
            .values()
            .filter_map(|state| {
                let g = state.geometry.as_ref()?;
                let (physical_width, physical_height) =
                    state.mode.map(|(_, w, h, _)| (w, h)).unwrap_or((0, 0));
                Some(OutputGeometry {
                    x: g.x,
                    y: g.y,
                    physical_width,
                    physical_height,
                    width_mm: g.width_mm,
                    height_mm: g.height_mm,
                    subpixel: g.subpixel,
                    transform: g.transform,
                    host_scale: state.host_scale,
                    // Mainline seeds host->device_scale_factor to 1000 (1.0)
                    // in sl_create_host_output (sommelier-output.cc:649) and
                    // only replaces it when the aura event arrives. Using the
                    // integer wl_output.scale as a fallback would invent a
                    // different DPI (e.g. 192 for a scale-2 host) and makes
                    // the initial Xwayland state inconsistent with its mode.
                    device_scale_factor: state
                        .device_scale_factor
                        .filter(|d| d.is_finite() && *d > 0.0)
                        .unwrap_or(1.0),
                    logical_width: state.logical_width,
                    logical_height: state.logical_height,
                })
            })
            .collect();
        for state in self.outputs.values() {
            if let Some(g) = &state.geometry {
                log::trace!(
                    "output {} {} at {},{} mode={:?} scale={}",
                    g.make,
                    g.model,
                    g.x,
                    g.y,
                    state.mode,
                    state.host_scale
                );
            }
        }
        // Deterministic ordering: top-to-bottom, then left-to-right.
        outputs.sort_by_key(|o| (o.y, o.x));

        // Feed the same layout to the X11 WM so it can write
        // _XWAYLAND_RANDR_EMU_MONITOR_RECTS (RandR emulation for X11 apps).
        let rects: Vec<(i32, i32, i32, i32)> = outputs
            .iter()
            .map(|o| {
                let (x, y) = ctx.transform.host_to_guest(o.x, o.y);
                let (w, h) = ctx
                    .transform
                    .host_to_guest_size(o.physical_width, o.physical_height);
                (x, y, w.max(1), h.max(1))
            })
            .collect();
        ctx.transform.set_outputs(outputs);
        if let Some(bridge) = &ctx.x11_bridge {
            bridge.set_output_rects(&rects);
        }
    }
}

/// Resolve the guest-side object id for the current host→client event.
fn guest_id(ctx: &Context) -> Option<u32> {
    ctx.shadow_table.get_guest_id(ctx.last_sender_id)
}

fn emit(ctx: &mut Context, guest_id: u32, opcode: u16, builder: MessageBuilder) {
    // These events never carry file descriptors.
    ctx.host_to_client_queue
        .push((builder.build_message(guest_id, opcode), Vec::new()));
}

impl OutputHandler {
    /// Re-emit the complete `wl_output` state for one guest output.
    ///
    /// Mainline `sl_calculate_scale_for_xwayland` (sommelier.cc:3541-3548)
    /// resends output state once the default scale factor is known; the
    /// comment at sommelier.cc:3599-3601 notes this output update is required
    /// to work around an Xwayland issue where DPI is otherwise not applied.
    /// `zaura_output.device_scale_factor` lands after the initial wl_output
    /// events, so we trigger the same update when it arrives. Hosts that never
    /// send it keep the original single-shot behaviour.
    fn resend_output_state(&self, ctx: &mut Context, host_output_id: u32) {
        let Some(gid) = ctx.shadow_table.get_guest_id(host_output_id) else {
            return;
        };
        let Some(state) = self.outputs.get(&host_output_id) else {
            return;
        };
        let Some(g) = &state.geometry else { return };
        let Some((flags, w, h, refresh)) = state.mode else {
            return;
        };

        let (pw, ph) = ctx.transform.physical_size_for_dpi(w, h);
        let mut b = MessageBuilder::new();
        b.write_i32(ctx.transform.virtual_x_for_host_x(g.x));
        b.write_i32(0);
        b.write_i32(pw);
        b.write_i32(ph);
        b.write_i32(g.subpixel);
        b.write_string(&g.make);
        b.write_string(&g.model);
        b.write_i32(g.transform);
        emit(ctx, gid, EVT_GEOMETRY, b);

        let (gw, gh) = ctx.transform.host_to_guest_size(w, h);
        let mut b = MessageBuilder::new();
        b.write_u32(flags);
        b.write_i32(gw);
        b.write_i32(gh);
        b.write_i32(refresh);
        emit(ctx, gid, EVT_MODE, b);

        let guest_scale = if ctx.is_xwayland_client {
            1
        } else {
            ctx.transform.guest_output_scale(state.host_scale.max(1.0))
        };
        let mut b = MessageBuilder::new();
        b.write_i32(guest_scale);
        emit(ctx, gid, EVT_SCALE, b);

        emit(ctx, gid, EVT_OUTPUT_DONE, MessageBuilder::new());
    }

    fn output_for_extension(&self, ctx: &Context) -> Option<u32> {
        ctx.host_output_for_extension(ctx.last_sender_id)
    }
}

/// Host `zaura_output` events. These objects are bound internally, so every
/// event is consumed locally (never forwarded to a guest object id).
impl ZauraOutputHandler for OutputHandler {
    fn on_scale(&mut self, ctx: &mut Context, flags: u32, scale: u32) -> Action {
        let Some(host_output) = self.output_for_extension(ctx) else {
            return Action::Drop;
        };
        let value = scale as f64 / 1000.0;
        if value.is_finite() && value > 0.0 {
            let state = self.outputs.entry(host_output).or_default();
            if flags & 0x2 != 0 {
                state.preferred_scale = value;
            }
            // The CURRENT flag is the last one sent (aura-shell.xml:606-610).
            // aura-shell.xml:557/559 — CURRENT=0x1, PREFERRED=0x2.
            if flags & 0x1 != 0 || flags == 0 {
                state.current_scale = value;
            }
        }
        Action::Drop
    }

    fn on_connection(&mut self, ctx: &mut Context, connection: u32) -> Action {
        if let Some(host_output) = self.output_for_extension(ctx) {
            self.outputs.entry(host_output).or_default().internal = connection == 1;
        }
        Action::Drop
    }

    fn on_device_scale_factor(&mut self, ctx: &mut Context, scale: u32) -> Action {
        let Some(host_output) = self.output_for_extension(ctx) else {
            return Action::Drop;
        };
        let value = scale as f64 / 1000.0;
        if value.is_finite() && value > 0.0 {
            self.outputs
                .entry(host_output)
                .or_default()
                .device_scale_factor = Some(value);
            self.sync_transform(ctx);
            self.resend_output_state(ctx, host_output);
        }
        Action::Drop
    }

    fn on_insets(
        &mut self,
        _ctx: &mut Context,
        _top: i32,
        _left: i32,
        _bottom: i32,
        _right: i32,
    ) -> Action {
        Action::Drop
    }

    fn on_logical_transform(&mut self, _ctx: &mut Context, _transform: i32) -> Action {
        Action::Drop
    }

    fn on_release(&mut self, _ctx: &mut Context) -> Action {
        Action::Drop
    }
}

impl WlOutputHandler for OutputHandler {
    fn on_geometry(
        &mut self,
        ctx: &mut Context,
        x: i32,
        y: i32,
        physical_width: i32,
        physical_height: i32,
        subpixel: i32,
        make: &str,
        model: &str,
        transform: i32,
    ) -> Action {
        let host_id = ctx.last_sender_id;
        self.outputs.entry(host_id).or_default().geometry = Some(Geometry {
            x,
            y,
            width_mm: physical_width,
            height_mm: physical_height,
            subpixel,
            make: make.to_string(),
            model: model.to_string(),
            transform,
        });
        self.sync_transform(ctx);

        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };

        // Mainline `sl_output_send_host_output_state` (sommelier-output.cc:401)
        // sends geometry(virt_x, 0, ...) — outputs are laid out horizontally
        // in virtual space, so y is always 0 and x is the accumulated virtual
        // width of every output to the left.
        let gx = ctx.transform.virtual_x_for_host_x(x);
        let gy = 0;
        // Physical millimetres are derived from the pixel size and the DPI
        // bucket, not from the host's own (host-DPI-specific) value.
        let (pw, ph) = self
            .outputs
            .get(&host_id)
            .and_then(|s| s.mode)
            .map(|(_, w, h, _)| ctx.transform.physical_size_for_dpi(w, h))
            .unwrap_or((physical_width, physical_height));
        let mut b = MessageBuilder::new();
        b.write_i32(gx);
        b.write_i32(gy);
        b.write_i32(pw);
        b.write_i32(ph);
        b.write_i32(subpixel);
        b.write_string(make);
        b.write_string(model);
        b.write_i32(transform);
        emit(ctx, gid, EVT_GEOMETRY, b);
        Action::Drop
    }

    fn on_mode(
        &mut self,
        ctx: &mut Context,
        flags: u32,
        width: i32,
        height: i32,
        refresh: i32,
    ) -> Action {
        let host_id = ctx.last_sender_id;
        self.outputs.entry(host_id).or_default().mode = Some((flags, width, height, refresh));
        self.sync_transform(ctx);

        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let (gw, gh) = ctx.transform.host_to_guest_size(width, height);
        let mut b = MessageBuilder::new();
        b.write_u32(flags);
        b.write_i32(gw);
        b.write_i32(gh);
        b.write_i32(refresh);
        emit(ctx, gid, EVT_MODE, b);
        Action::Drop
    }

    fn on_scale(&mut self, ctx: &mut Context, factor: i32) -> Action {
        let host_id = ctx.last_sender_id;
        self.outputs.entry(host_id).or_default().host_scale = factor.max(1) as f64;
        self.sync_transform(ctx);

        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        // Mainline `sl_output_get_host_output_state` X11 branch
        // (sommelier-output.cc:106-108) forces the advertised integer scale
        // to 1: Xwayland works in the virtual coordinate space already.
        let guest_scale = if ctx.is_xwayland_client {
            1
        } else {
            ctx.transform.guest_output_scale(factor.max(1) as f64)
        };
        let mut b = MessageBuilder::new();
        b.write_i32(guest_scale);
        emit(ctx, gid, EVT_SCALE, b);
        Action::Drop
    }

    fn on_done(&mut self, ctx: &mut Context) -> Action {
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        emit(ctx, gid, EVT_OUTPUT_DONE, MessageBuilder::new());
        Action::Drop
    }

    fn on_name(&mut self, ctx: &mut Context, name: &str) -> Action {
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let mut b = MessageBuilder::new();
        b.write_string(name);
        emit(ctx, gid, EVT_NAME, b);
        Action::Drop
    }

    fn on_description(&mut self, ctx: &mut Context, description: &str) -> Action {
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let mut b = MessageBuilder::new();
        b.write_string(description);
        emit(ctx, gid, EVT_DESCRIPTION, b);
        Action::Drop
    }
    // `on_release` is a client→host request: the generated default forward
    // path already translates the id and removes it from the shadow table.
}

impl WpFractionalScaleV1Handler for OutputHandler {
    fn on_preferred_scale(&mut self, ctx: &mut Context, scale: u32) -> Action {
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let preferred = ctx.transform.preferred_scale(scale);
        let mut b = MessageBuilder::new();
        b.write_u32(preferred);
        emit(ctx, gid, EVT_PREFERRED_SCALE, b);
        Action::Drop
    }
}

impl ZxdgOutputV1Handler for OutputHandler {
    fn on_logical_position(&mut self, ctx: &mut Context, x: i32, y: i32) -> Action {
        if let Some(host_output) = self.output_for_extension(ctx) {
            let state = self.outputs.entry(host_output).or_default();
            state.logical_x = x;
            state.logical_y = y;
            self.sync_transform(ctx);
            return Action::Drop;
        }
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let (gx, gy) = ctx.transform.host_to_guest(x, y);
        let mut b = MessageBuilder::new();
        b.write_i32(gx);
        b.write_i32(gy);
        emit(ctx, gid, EVT_LOGICAL_POSITION, b);
        Action::Drop
    }

    fn on_logical_size(&mut self, ctx: &mut Context, width: i32, height: i32) -> Action {
        if let Some(host_output) = self.output_for_extension(ctx) {
            let state = self.outputs.entry(host_output).or_default();
            state.logical_width = width;
            state.logical_height = height;
            self.sync_transform(ctx);
            return Action::Drop;
        }
        let Some(gid) = guest_id(ctx) else {
            return Action::Forward;
        };
        let (gw, gh) = ctx.transform.host_to_guest_size(width, height);
        let mut b = MessageBuilder::new();
        b.write_i32(gw);
        b.write_i32(gh);
        emit(ctx, gid, EVT_LOGICAL_SIZE, b);
        Action::Drop
    }
    // done / name / description carry no coordinates, so the default
    // forward path is correct and preserves their original payloads.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::{ScalingMode, Transform};

    fn ctx_for(scale: f64) -> Context {
        let mut ctx = Context::new(false, true);
        ctx.transform = Transform::new(scale, ScalingMode::Normal, vec![]);
        ctx
    }

    #[test]
    fn geometry_and_mode_are_scaled_for_the_guest() {
        let mut handler = OutputHandler::new();
        let mut ctx = ctx_for(2.0);
        // Map host output 7 -> guest 3.
        ctx.shadow_table.map_id(3, 7);
        ctx.last_sender_id = 7;

        assert_eq!(
            WlOutputHandler::on_scale(&mut handler, &mut ctx, 2),
            Action::Drop
        );
        assert_eq!(
            handler.on_mode(&mut ctx, 1, 1920, 1080, 60000),
            Action::Drop
        );
        assert_eq!(
            handler.on_geometry(&mut ctx, 100, 50, 300, 200, 0, "make", "model", 0),
            Action::Drop
        );
        // UFCS: both WlOutputHandler and ZxdgOutputV1Handler expose
        // `on_done`, and both traits are in scope inside this test module.
        assert_eq!(
            WlOutputHandler::on_done(&mut handler, &mut ctx),
            Action::Drop
        );
        assert_eq!(ctx.host_to_client_queue.len(), 4);
        assert_eq!((ctx.transform.virtual_dimensions().0 > 0), true);
    }

    #[test]
    fn aura_device_scale_factor_corrects_dpi_and_resends_output_state() {
        let mut handler = OutputHandler::new();
        let mut ctx = ctx_for(1.0);
        ctx.shadow_table.map_id(3, 7);
        ctx.output_extension_to_host.insert(77, 7);
        ctx.last_sender_id = 7;
        handler.on_mode(&mut ctx, 1, 1920, 1080, 60000);
        assert_eq!(
            WlOutputHandler::on_scale(&mut handler, &mut ctx, 2),
            Action::Drop
        );
        handler.on_geometry(&mut ctx, 0, 0, 300, 200, 0, "make", "model", 0);
        // Before aura data arrives mainline's default DSF is 1.0, i.e. 96 DPI
        // (sommelier-output.cc:649), regardless of wl_output.scale.
        let fallback_mm = {
            let last = ctx.host_to_client_queue.last().unwrap();
            i32::from_ne_bytes(last.0[16..20].try_into().unwrap())
        };
        assert_eq!(fallback_mm, 508);
        assert_eq!(
            ctx.transform.physical_size_for_dpi(1920, 1080).0,
            fallback_mm
        );

        ctx.host_to_client_queue.clear();
        ctx.last_sender_id = 77;
        assert_eq!(
            ZauraOutputHandler::on_device_scale_factor(&mut handler, &mut ctx, 1250),
            Action::Drop
        );
        // geometry + mode + scale + done, exactly like mainline's resend.
        assert_eq!(ctx.host_to_client_queue.len(), 4);
        let geometry = &ctx.host_to_client_queue[0].0;
        assert_eq!(u32::from_ne_bytes(geometry[0..4].try_into().unwrap()), 3);
        let mm = i32::from_ne_bytes(geometry[16..20].try_into().unwrap());
        assert_eq!(mm, 406); // 1920px at 96 * 1.25 = 120 dpi
                             // A higher DSF means a higher DPI, hence fewer physical millimetres.
        assert!(mm < fallback_mm);
        assert_eq!(
            ctx.transform.outputs.first().unwrap().device_scale_factor,
            1.25
        );
    }

    /// Lock in the mainline equivalence of the X11 wire state.
    ///
    /// Mainline `sl_calculate_scale_for_xwayland` (sommelier.cc:3507-3549)
    /// sets `ctx->scale = desired_scale * DSF * preferred_scale` and then
    /// `sl_output_get_host_output_state` (sommelier-output.cc:106-110) emits
    /// `width = host_w * ctx->scale / (DSF * current_scale)` with a forced
    /// integer scale of 1. When `preferred_scale == current_scale` the DSF and
    /// zoom terms cancel, leaving `host_w * desired_scale`. Our `Transform`
    /// models DSF separately (in `OutputGeometry`, used for DPI / direct
    /// mode) and computes `host_to_guest_size = host_w * desired_scale`, so
    /// the wire result is identical without ever folding DSF into the scale.
    /// Folding it in here would double-apply DSF and break the verified
    /// 96 -> 120 DPI two-phase sequence above.
    #[test]
    fn xwayland_wire_state_matches_mainline_formula() {
        // Integral desired scales only: mainline rounds --scale to an integer
        // before wp_viewporter is detected (sommelier.cc:4408-4411).
        for desired in [1.0, 2.0, 3.0] {
            let mut handler = OutputHandler::new();
            let mut ctx = ctx_for(desired);
            ctx.is_xwayland_client = true;
            ctx.shadow_table.map_id(3, 7);
            ctx.output_extension_to_host.insert(77, 7);
            ctx.last_sender_id = 7;
            handler.on_mode(&mut ctx, 1, 1920, 1080, 60000);
            WlOutputHandler::on_scale(&mut handler, &mut ctx, 2);
            handler.on_geometry(&mut ctx, 0, 0, 300, 200, 0, "make", "model", 0);
            ctx.host_to_client_queue.clear();

            ctx.last_sender_id = 77;
            ZauraOutputHandler::on_device_scale_factor(&mut handler, &mut ctx, 1250);

            // mainline: applied_scale = 1.25 * 1.0, ctx->scale = desired *
            // 1.25 * 1.0 -> width = 1920 * desired; scale forced to 1.
            let mode = &ctx.host_to_client_queue[1].0;
            let width = i32::from_ne_bytes(mode[12..16].try_into().unwrap());
            let height = i32::from_ne_bytes(mode[16..20].try_into().unwrap());
            assert_eq!(width, (1920.0 * desired) as i32);
            assert_eq!(height, (1080.0 * desired) as i32);
            let scale = &ctx.host_to_client_queue[2].0;
            assert_eq!(i32::from_ne_bytes(scale[8..12].try_into().unwrap()), 1);
        }
    }

    #[test]
    fn internal_xdg_output_logical_size_is_consumed_locally() {
        let mut handler = OutputHandler::new();
        let mut ctx = ctx_for(1.0);
        ctx.transform = Transform::new(1.0, ScalingMode::Direct, vec![]);
        ctx.output_extension_to_host.insert(88, 7);
        ctx.last_sender_id = 7;
        handler.on_geometry(&mut ctx, 0, 0, 300, 200, 0, "make", "model", 0);
        ctx.host_to_client_queue.clear();
        ctx.last_sender_id = 88;
        assert_eq!(
            ZxdgOutputV1Handler::on_logical_size(&mut handler, &mut ctx, 1536, 864),
            Action::Drop
        );
        assert!(ctx.host_to_client_queue.is_empty());
        assert_eq!(handler.outputs[&7].logical_width, 1536);
        assert_eq!(handler.outputs[&7].logical_height, 864);
        assert_eq!(ctx.transform.outputs.first().unwrap().logical_width, 1536);
    }

    #[test]
    fn fractional_scale_is_divided_by_content_scale() {
        let mut handler = OutputHandler::new();
        let mut ctx = ctx_for(2.0);
        ctx.shadow_table.map_id(9, 9);
        ctx.last_sender_id = 9;
        assert_eq!(handler.on_preferred_scale(&mut ctx, 240), Action::Drop);
        assert_eq!(ctx.host_to_client_queue.len(), 1);
    }
}
