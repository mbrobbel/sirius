//! Compile the cxx bridge against an installed Sirius package.

use std::path::{Path, PathBuf};

enum Linkage {
    Shared,
    Static,
}

impl Linkage {
    fn artifact(&self) -> &'static str {
        match self {
            Self::Shared => "libsirius.so",
            Self::Static => "libsirius.a",
        }
    }
}

struct SiriusInstallation {
    include: PathBuf,
    library: PathBuf,
}

impl SiriusInstallation {
    fn discover(linkage: &Linkage) -> Self {
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let prefix = std::env::var_os("SIRIUS_PREFIX")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("CONDA_PREFIX")
                    .map(PathBuf::from)
                    .filter(|path| path.join("include/sirius/ffi.hpp").is_file())
            })
            .unwrap_or_else(|| manifest.join("../../../build/release/install"));
        let include = prefix.join("include");
        let library = [prefix.join("lib"), prefix.join("lib64")]
            .into_iter()
            .find(|path| path.join(linkage.artifact()).is_file())
            .filter(|_| include.join("sirius/ffi.hpp").is_file())
            .unwrap_or_else(|| {
                panic!(
                    "Sirius headers and {} are required under {}. Install the \
                     sirius_library CMake component and set SIRIUS_PREFIX.",
                    linkage.artifact(),
                    prefix.display()
                )
            });
        Self { include, library }
    }
}

fn main() {
    let linkage = if std::env::var_os("CARGO_FEATURE_STATIC").is_some() {
        Linkage::Static
    } else {
        Linkage::Shared
    };
    let installation = SiriusInstallation::discover(&linkage);
    cxx_build::bridge("src/lib.rs")
        .std("c++20")
        .include(&installation.include)
        .compile("sirius_sys");
    println!(
        "cargo:rustc-link-search=native={}",
        installation.library.display()
    );
    if let Some(conda) = std::env::var_os("CONDA_PREFIX").map(PathBuf::from) {
        for directory in conda_lib_dirs(&conda) {
            println!("cargo:rustc-link-search=native={}", directory.display());
        }
    }
    match linkage {
        Linkage::Shared => println!("cargo:rustc-link-lib=dylib=sirius"),
        Linkage::Static => {
            println!("cargo:rustc-link-lib=static:+whole-archive=sirius");
            let metadata = installation.library.join("cmake/sirius/libsirius.a.cmake");
            let contents = std::fs::read_to_string(&metadata)
                .unwrap_or_else(|error| panic!("read {}: {error}", metadata.display()));
            let libraries = contents
                .trim()
                .strip_prefix("set(SIRIUS_STATIC_SYSTEM_LIBRARIES \"")
                .and_then(|value| value.strip_suffix("\")"))
                .expect("invalid Sirius static link metadata");
            for library in libraries.split(';').filter(|value| !value.is_empty()) {
                assert!(
                    library
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_+.-".contains(&byte)),
                    "invalid native library name in Sirius metadata"
                );
                println!("cargo:rustc-link-lib={library}");
            }
            println!("cargo:rerun-if-changed={}", metadata.display());
        }
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!(
        "cargo:rerun-if-changed={}",
        installation.include.join("sirius/ffi.hpp").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        installation.library.join(linkage.artifact()).display()
    );
    println!("cargo:rerun-if-env-changed=SIRIUS_PREFIX");
    println!("cargo:rerun-if-env-changed=CONDA_PREFIX");
}

fn conda_lib_dirs(conda: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![conda.join("lib")];
    if let Ok(entries) = std::fs::read_dir(conda.join("targets")) {
        for entry in entries.flatten() {
            let lib = entry.path().join("lib");
            dirs.push(lib.join("stubs"));
            dirs.push(lib);
        }
    }
    dirs.retain(|directory| directory.is_dir());
    dirs
}
