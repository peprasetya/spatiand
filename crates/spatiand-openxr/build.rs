//! Compile the two shaders the compositor draws quad layers with, to SPIR-V, at build time.
//!
//! In Rust, with `naga`, so that building the runtime needs no shader compiler installed on the
//! machine -- the Deck and the host it streams from both build this, and neither should need
//! `glslc` for two small shaders.

use std::path::PathBuf;

use naga::back::spv;
use naga::front::glsl;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::ShaderStage;

fn compile(source: &str, stage: ShaderStage) -> Vec<u32> {
    let mut frontend = glsl::Frontend::default();
    let module = frontend
        .parse(&glsl::Options::from(stage), source)
        .unwrap_or_else(|e| panic!("could not parse the {stage:?} shader: {e:?}"));
    let info = Validator::new(ValidationFlags::all(), Capabilities::PUSH_CONSTANT)
        .validate(&module)
        .unwrap_or_else(|e| panic!("the {stage:?} shader is not valid: {e:?}"));
    let options = spv::Options {
        // Vulkan, and the framebuffer's y runs down already: the matrices account for it.
        flags: spv::WriterFlags::empty(),
        ..Default::default()
    };
    spv::write_vec(&module, &info, &options, None)
        .unwrap_or_else(|e| panic!("could not write the {stage:?} shader: {e:?}"))
}

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for (name, stage) in [("quad.vert", ShaderStage::Vertex), ("quad.frag", ShaderStage::Fragment)] {
        let path = format!("shaders/{name}");
        println!("cargo:rerun-if-changed={path}");
        let source = std::fs::read_to_string(&path).unwrap();
        let words = compile(&source, stage);
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(out.join(format!("{name}.spv")), bytes).unwrap();
    }
    println!("cargo:rerun-if-changed=build.rs");
}
