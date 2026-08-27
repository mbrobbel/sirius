//! Build script for `sirius-sys`.
//!
//! Two jobs:
//!  1. Compile the cxx bridge glue against Sirius's lightweight public FFI header
//!     (`src/include/sirius_ffi.hpp`) — no internal Sirius headers are needed, so
//!     this is the only include directory.
//!  2. Link the Sirius artifact that exports the FFI symbols.
//!     `cargo:rustc-link-{lib,search}` propagate to downstream binaries, e.g.
//!     the `sirius` crate's `cargo test`.
//!
//! The artifact lives under the CMake build tree (`$SIRIUS_BUILD_DIR`, default
//! `build/release`) or an installed prefix (`$SIRIUS_PREFIX`). Linkage is selectable:
//!  * default          — `libsirius.so` (shared; records its deps via DT_NEEDED).
//!  * `static` feature — `libsirius.a` (self-contained; no runtime deps — the
//!    fully static vcpkg build).

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest
        .join("../../..")
        .canonicalize()
        .expect("resolve repo root from CARGO_MANIFEST_DIR");
    let build_dir = std::env::var_os("SIRIUS_BUILD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("build/release"));
    let prefix = std::env::var_os("SIRIUS_PREFIX").map(PathBuf::from);
    let static_link = std::env::var_os("CARGO_FEATURE_STATIC").is_some();
    let include_dir = resolve_include_dir(&repo, prefix.as_deref());
    let ffi_header = include_dir.join("sirius_ffi.hpp");

    // 1. Compile the cxx glue against the public FFI header only.
    cxx_build::bridge("src/lib.rs")
        .std("c++20")
        .include(&include_dir)
        .compile("sirius_sys");

    // 2. Link the single Sirius artifact.
    let lib_dir = resolve_lib_dir(&build_dir, prefix.as_deref(), static_link);
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if static_link {
        println!("cargo:rustc-link-lib=static:+whole-archive=sirius");
        for library in [
            "cuda",
            "nvidia-ml",
            "stdc++",
            "gcc_s",
            "dl",
            "rt",
            "pthread",
            "m",
        ] {
            println!("cargo:rustc-link-lib=dylib={library}");
        }
    } else {
        // The shared lib pulls its own deps in via DT_NEEDED; make them findable
        // at link time by searching the conda/CUDA lib dirs.
        if let Some(conda) = std::env::var_os("CONDA_PREFIX").map(PathBuf::from) {
            for dir in conda_lib_dirs(&conda) {
                println!("cargo:rustc-link-search=native={}", dir.display());
            }
        }
        println!("cargo:rustc-link-lib=dylib=sirius");
    }

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed={}", ffi_header.display());
    println!("cargo:rerun-if-changed={}", lib_dir.display());
    println!("cargo:rerun-if-env-changed=SIRIUS_BUILD_DIR");
    println!("cargo:rerun-if-env-changed=SIRIUS_PREFIX");
    println!("cargo:rerun-if-env-changed=SIRIUS_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=CONDA_PREFIX");
}

/// Directory holding the Sirius artifact to link (`libsirius.{a,so}`).
///
/// Panics with actionable guidance if nothing is found — the common cause is
/// forgetting to build Sirius first.
fn resolve_lib_dir(build_dir: &Path, prefix: Option<&Path>, static_link: bool) -> PathBuf {
    let mut candidates = vec![build_dir.join("extension/sirius"), build_dir.to_path_buf()];
    if let Some(prefix) = prefix {
        candidates.push(prefix.join("lib/sirius-static"));
        candidates.push(prefix.join("lib"));
    }
    if let Some(conda) = std::env::var_os("CONDA_PREFIX").map(PathBuf::from) {
        candidates.push(conda.join("lib/sirius-static"));
        candidates.push(conda.join("lib"));
    }
    let file = if static_link {
        "libsirius.a"
    } else {
        "libsirius.so"
    };

    if let Some(dir) = candidates.iter().find(|d| d.join(file).is_file()) {
        return dir.clone();
    }

    let looked_for = if static_link {
        "libsirius.a (a self-contained static bundle)".to_string()
    } else {
        file.to_string()
    };
    panic!(
        "could not find {looked_for} under {} — build Sirius first (`pixi run make`), or point \
         SIRIUS_BUILD_DIR at the build tree",
        build_dir.display()
    );
}

fn resolve_include_dir(repo: &Path, prefix: Option<&Path>) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(include_dir) = std::env::var_os("SIRIUS_INCLUDE_DIR").map(PathBuf::from) {
        candidates.push(include_dir);
    }
    if let Some(prefix) = prefix {
        candidates.push(prefix.join("include/sirius"));
    }
    if let Some(conda) = std::env::var_os("CONDA_PREFIX").map(PathBuf::from) {
        candidates.push(conda.join("include/sirius"));
    }
    candidates.push(repo.join("src/include"));

    candidates
        .into_iter()
        .find(|dir| dir.join("sirius_ffi.hpp").is_file())
        .unwrap_or_else(|| {
            panic!("could not find sirius_ffi.hpp; set SIRIUS_INCLUDE_DIR or SIRIUS_PREFIX")
        })
}

/// conda env lib dirs (`lib`, `targets/<arch>/lib[/stubs]`) for the shared lib's
/// transitive deps.
fn conda_lib_dirs(conda: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![conda.join("lib")];
    if let Ok(entries) = std::fs::read_dir(conda.join("targets")) {
        for entry in entries.flatten() {
            let lib = entry.path().join("lib");
            dirs.push(lib.join("stubs"));
            dirs.push(lib);
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}
