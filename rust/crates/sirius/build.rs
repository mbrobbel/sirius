//! Build script for `sirius`.
//!
//! Native libraries propagate from sirius-sys, but linker arguments must be
//! emitted by the crate that builds the final binary or test executable.

fn main() {
    if std::env::var_os("CARGO_FEATURE_STATIC").is_some() {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol=InitializeInjectionNvtx2");
        println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol=dlopen");
    }
}
