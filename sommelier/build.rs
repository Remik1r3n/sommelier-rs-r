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

//! Protocol codegen driver.
//!
//! Upstream kept a hard-coded list of XML files here and a parallel hard-coded
//! `include!` list in `main.rs`; the `main` and `virtwl` branches disagreed on
//! both. Here the directory is enumerated, so adding a protocol is just
//! dropping an XML into `third_party/protocols/`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use wayland_codegen::aggregate::generate_aggregate;
use wayland_codegen::generator::generate;
use wayland_codegen::parse;
use wayland_codegen::protocol::Protocol;

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let xml_dir = Path::new("../third_party/protocols");

    println!("cargo:rerun-if-changed={}", xml_dir.display());

    let mut paths: Vec<PathBuf> = fs::read_dir(xml_dir)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", xml_dir.display(), e))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    paths.sort();

    let mut entries: Vec<(String, Protocol)> = Vec::new();
    let mut includes = String::new();

    for path in &paths {
        println!("cargo:rerun-if-changed={}", path.display());

        let protocol =
            parse(path).unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

        // `<protocol name="...">` determines the generated Rust module name.
        // Sanitize defensively so an upstream hyphen cannot break compilation.
        let module = protocol.name.replace('-', "_");
        assert!(
            module
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && module
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_'),
            "invalid protocol module name {:?} from {}",
            module,
            path.display()
        );

        let dest = out_dir.join(format!("{}_protocol.rs", module));
        fs::write(&dest, generate(&protocol))
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", dest.display(), e));

        includes.push_str(&format!(
            "include!(concat!(env!(\"OUT_DIR\"), \"/{}_protocol.rs\"));\n",
            module
        ));
        entries.push((module, protocol));
    }

    fs::write(
        out_dir.join("aggregate_protocol.rs"),
        generate_aggregate(&entries),
    )
    .expect("Failed to write aggregate_protocol.rs");
    includes.push_str("include!(concat!(env!(\"OUT_DIR\"), \"/aggregate_protocol.rs\"));\n");
    fs::write(out_dir.join("all_includes.rs"), includes).expect("Failed to write all_includes.rs");

    println!("cargo::metadata=protocol_count={}", entries.len());
}
