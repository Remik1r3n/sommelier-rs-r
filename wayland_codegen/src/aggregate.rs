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

//! Generates one aggregate module that unifies dispatch across every protocol
//! XML in the workspace.
//!
//! Upstream `sommelier-rs` had a hand-maintained `if / else if` chain in
//! `proxy.rs` (one arm per protocol) and a hand-maintained safelist in
//! `registry.rs`. Adding a protocol meant editing four files and was the main
//! reason the two branches drifted. Here the build script derives both from the
//! same parsed XML list, so adding a protocol is a single `build.rs` edit.

use crate::protocol::{Interface, Protocol, ProtocolItem};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::collections::HashSet;

/// A protocol whose objects are bound **internally** by the proxy and must
/// never be advertised to, or receive events from, guest clients.
const INTERNAL_ONLY: &[&str] = &[
    // ChromeOS shelf integration: bound internally, translated into
    // xdg_toplevel.set_app_id / zaura_surface.set_application_id.
    "aura_shell",
    // Globals that registry.rs hides from the guest (INTERNAL_GLOBALS) and
    // binds on the host side only. Any host event addressed to these objects
    // carries a host object id the guest has never seen; forwarding it would
    // inject a bogus object into the guest connection.
    "drm",
    "gtk",
    "zwp_linux_explicit_synchronization_unstable_v1",
    "linux_drm_syncobj_v1",
    "stylus_unstable_v2",
    "gaming_input_unstable_v2",
    "text_input_crostini_unstable_v1",
    "text_input_x11_unstable_v1",
    "keyboard_extension_unstable_v1",
    // Deliberately NOT internal-only:
    //  * text_input_unstable_v1 / text_input_extension_unstable_v1 have
    //    hand-written host-event handlers that translate v1 events into
    //    guest text-input-v3 events. Dropping them would break the IME.
    //  * linux_dmabuf_v1 / viewporter / tablet_unstable_v2 are advertised
    //    to and bound by guests normally.
];

/// Protocols for which `proxy.rs` already provides hand-written delegation
/// mappings. The aggregate macro must not generate a second (conflicting) impl.
const EXPLICIT_DELEGATES: &[&str] = &[
    "wayland",
    "xdg_shell",
    "linux_dmabuf_v1",
    "viewporter",
    "text_input_unstable_v3",
    "text_input_unstable_v1",
    "text_input_extension_unstable_v1",
    "xdg_decoration_unstable_v1",
    "fractional_scale_v1",
    "xdg_output_unstable_v1",
    "keyboard_extension_unstable_v1",
    "aura_shell",
    // gtk_shell1/gtk_surface1 are guest-only objects translated onto
    // zaura_shell by handler/gtk_shell.rs.
    "gtk",
];

fn interfaces(p: &Protocol) -> Vec<&Interface> {
    p.items
        .iter()
        .filter_map(|i| match i {
            ProtocolItem::Interface(x) => Some(x),
            _ => None,
        })
        .collect()
}

/// `entries` is `(module_name, protocol)`. Module name always comes from
/// `<protocol name="...">` sanitized to a Rust identifier.
pub fn generate_aggregate(entries: &[(String, Protocol)]) -> String {
    let mut allowed: Vec<&str> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut request_arms: Vec<TokenStream> = Vec::new();
    let mut event_arms: Vec<TokenStream> = Vec::new();
    let mut supertraits: Vec<TokenStream> = Vec::new();
    let mut delegate_calls: Vec<TokenStream> = Vec::new();
    let mut trait_impls: Vec<TokenStream> = Vec::new();

    for (module, protocol) in entries {
        let mod_ident: Ident = format_ident!("{}", module);
        supertraits.push(quote! { super::#mod_ident::ProtocolHandler });

        if !EXPLICIT_DELEGATES.contains(&module.as_str()) {
            delegate_calls.push(quote! {
                $crate::protocols::#mod_ident::impl_sommelier_delegates!($t, {});
            });
            trait_impls.push(quote! {
                impl $crate::protocols::#mod_ident::ProtocolHandler for $t {}
            });
        }

        let internal = INTERNAL_ONLY.contains(&module.as_str());
        for iface in interfaces(protocol) {
            let name = iface.name.as_str();
            if !seen.insert(name.to_string()) {
                continue;
            }
            allowed.push(name);
            request_arms.push(quote! {
                #name => super::#mod_ident::dispatch_request(interface, msg, handler, ctx),
            });
            if internal {
                // Host events for internally-bound objects belong to us, not the
                // guest. Forwarding them would inject bogus object ids.
                event_arms.push(quote! { #name => Ok(None), });
            } else {
                event_arms.push(quote! {
                    #name => super::#mod_ident::dispatch_event(interface, msg, handler, ctx),
                });
            }
        }
    }

    let expanded = quote! {
        pub mod all {
            #![allow(unused_imports)]
            #![allow(dead_code)]
            #![allow(clippy::type_complexity)]

            use crate::state::Context;
            use crate::wire::{Action, MessageBuilder, ProtocolError, WireMessage};
            use std::os::unix::io::RawFd;

            /// Union of every interface known to the codegen. Used by the
            /// dispatcher; the guest-visible safelist in `handler::registry`
            /// is a curated subset of this.
            pub const KNOWN_INTERFACES: &[&str] = &[ #(#allowed),* ];

            pub fn is_known(interface: &str) -> bool {
                KNOWN_INTERFACES.contains(&interface)
            }

            /// Supertrait covering every generated `ProtocolHandler`.
            pub trait AllHandlers: #(#supertraits +)* {}
            impl<T> AllHandlers for T where T: #(#supertraits +)* {}

            pub fn dispatch_request<H: AllHandlers + ?Sized>(
                interface: &str,
                msg: &mut WireMessage,
                handler: &mut H,
                ctx: &mut Context,
            ) -> Result<Option<(Vec<u8>, Vec<RawFd>)>, ProtocolError> {
                match interface {
                    #(#request_arms)*
                    _ => Ok(None),
                }
            }

            pub fn dispatch_event<H: AllHandlers + ?Sized>(
                interface: &str,
                msg: &mut WireMessage,
                handler: &mut H,
                ctx: &mut Context,
            ) -> Result<Option<(Vec<u8>, Vec<RawFd>)>, ProtocolError> {
                match interface {
                    #(#event_arms)*
                    _ => Ok(None),
                }
            }

            /// Implement the (default, pure-forward) handler traits for every
            /// protocol that `proxy.rs` does not map by hand. Invoke once with
            /// `crate::protocols::all::impl_generated_delegates!(SommelierHandler);`.
            macro_rules! impl_generated_delegates {
                ($t:ty) => {
                    #(#delegate_calls)*
                    #(#trait_impls)*
                };
            }
            pub(crate) use impl_generated_delegates;
        }
    };

    crate::generator::format_rust_code(&expanded.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_contains_dispatch_and_safelist() {
        let p = Protocol {
            name: "demo".to_string(),
            items: vec![ProtocolItem::Interface(Interface {
                name: "wl_demo".to_string(),
                version: 1,
                items: vec![],
            })],
        };
        let code = generate_aggregate(&[("demo".to_string(), p)]);
        assert!(code.contains("KNOWN_INTERFACES"));
        assert!(code.contains("wl_demo"));
        assert!(code.contains("dispatch_request"));
        assert!(code.contains("impl_generated_delegates"));
    }
}
