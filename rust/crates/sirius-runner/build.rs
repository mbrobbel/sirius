fn main() {
    // include_dir! doesn't track directory membership on stable Rust; rerun
    // when suites/ changes so added/removed files land in the embedded set.
    println!("cargo::rerun-if-changed=suites");
}
