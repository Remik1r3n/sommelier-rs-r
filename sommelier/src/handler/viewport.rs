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

//! `wp_viewport.set_destination` translation for `--viewport-resize`.
//!
//! A guest that cannot be resized (fixed-size dialogs, legacy toolkits) asks
//! the compositor to scale its buffer into a smaller destination rectangle.
//! Our virtual coordinate system differs from the host's logical one by
//! `--scale`, so the destination must be converted before the request reaches
//! the host. With the flag off (the default) the request is forwarded verbatim.

use crate::protocols::viewporter::wp_viewport::{WpViewportHandler, REQ_SET_DESTINATION};
use crate::protocols::viewporter::wp_viewporter::WpViewporterHandler;
use crate::state::Context;
use crate::wire::{Action, MessageBuilder};

#[derive(Debug, Default)]
pub struct ViewportHandler;

impl WpViewporterHandler for ViewportHandler {}

impl WpViewportHandler for ViewportHandler {
    fn on_set_destination(&mut self, ctx: &mut Context, width: i32, height: i32) -> Action {
        // -1/-1 is the protocol's "unset destination" value and must pass
        // through untouched.
        if !ctx.viewport_resize || width <= 0 || height <= 0 {
            return Action::Forward;
        }
        let (host_w, host_h) = ctx.transform.viewport_scale(width, height);
        if (host_w, host_h) == (width, height) {
            return Action::Forward;
        }
        let guest_id = ctx.last_sender_id;
        let Some(host_id) = ctx.shadow_table.get_host_id(guest_id) else {
            log::debug!("viewport.set_destination on unmapped object {}", guest_id);
            return Action::Drop;
        };
        let mut builder = MessageBuilder::new();
        builder.write_i32(host_w);
        builder.write_i32(host_h);
        let msg = builder.build_message(host_id, REQ_SET_DESTINATION as u16);
        ctx.client_to_host_queue.push((msg, Vec::new()));
        Action::Drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::{ScalingMode, Transform};

    fn msg_i32(msg: &[u8], off: usize) -> i32 {
        i32::from_ne_bytes(msg[8 + off..12 + off].try_into().unwrap())
    }

    fn ctx_for(scale: f64, enabled: bool) -> Context {
        let mut ctx = Context::new_for_test(false, false, vec![]);
        ctx.viewport_resize = enabled;
        ctx.transform = Transform::new(scale, ScalingMode::Normal, vec![]);
        ctx.shadow_table.map_id(12, 34);
        ctx.last_sender_id = 12;
        ctx
    }

    #[test]
    fn destination_is_scaled_when_enabled() {
        let mut ctx = ctx_for(2.0, true);
        let mut handler = ViewportHandler;
        assert_eq!(handler.on_set_destination(&mut ctx, 100, 50), Action::Drop);
        assert_eq!(ctx.client_to_host_queue.len(), 1);
        let msg = &ctx.client_to_host_queue[0].0;
        assert_eq!(u32::from_ne_bytes(msg[0..4].try_into().unwrap()), 34);
        let opcode = (u32::from_ne_bytes(msg[4..8].try_into().unwrap()) & 0xffff) as u16;
        assert_eq!(opcode, REQ_SET_DESTINATION as u16);
        assert_eq!((msg_i32(msg, 0), msg_i32(msg, 4)), (50, 25));
    }

    #[test]
    fn unset_and_disabled_are_forwarded_verbatim() {
        let mut handler = ViewportHandler;
        let mut ctx = ctx_for(2.0, true);
        assert_eq!(
            handler.on_set_destination(&mut ctx, -1, -1),
            Action::Forward
        );
        assert!(ctx.client_to_host_queue.is_empty());

        let mut ctx = ctx_for(2.0, false);
        assert_eq!(
            handler.on_set_destination(&mut ctx, 100, 50),
            Action::Forward
        );
        assert!(ctx.client_to_host_queue.is_empty());
    }

    #[test]
    fn identity_scale_uses_the_fast_forward_path() {
        let mut ctx = ctx_for(1.0, true);
        let mut handler = ViewportHandler;
        assert_eq!(
            handler.on_set_destination(&mut ctx, 100, 50),
            Action::Forward
        );
        assert!(ctx.client_to_host_queue.is_empty());
    }
}
