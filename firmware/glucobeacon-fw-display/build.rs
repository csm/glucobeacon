fn main() {
    // esp-hal ships the linker scripts; this just makes cargo rerun when they
    // or the memory layout change.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    println!("cargo:rerun-if-changed=build.rs");
}
