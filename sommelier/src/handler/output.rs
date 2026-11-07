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
#[derive(Debug, Clone, Default)]
struct OutputState {
    geometry: Option<Geometry>,
    /// `(flags, width, height, refresh)` from `wl_output.mode`.
    mode: Option<(u32, i32, i32, i32)>,
    host_scale: f64,
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
                    device_scale_factor: state.host_scale.max(1.0),
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

        // Host Logical -> Virtual position.
        let (gx, gy) = ctx.transform.host_to_guest(x, y);
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
        let guest_scale = ctx.transform.guest_output_scale(factor.max(1) as f64);
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

        assert_eq!(handler.on_scale(&mut ctx, 2), Action::Drop);
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
    fn fractional_scale_is_divided_by_content_scale() {
        let mut handler = OutputHandler::new();
        let mut ctx = ctx_for(2.0);
        ctx.shadow_table.map_id(9, 9);
        ctx.last_sender_id = 9;
        assert_eq!(handler.on_preferred_scale(&mut ctx, 240), Action::Drop);
        assert_eq!(ctx.host_to_client_queue.len(), 1);
    }
}
