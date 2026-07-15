fn main() {
    // include_dir! doesn't track directory membership on stable Rust; rerun
    // when the embedded definitions change so additions/removals land.
    println!("cargo::rerun-if-changed=datasets");
    println!("cargo::rerun-if-changed=suites");
    println!("cargo::rerun-if-changed=benches");
}
