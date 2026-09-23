use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

pub fn runtime_path() -> Result<PathBuf> {
    let maps = fs::read_to_string("/proc/self/maps")?;
    let path = maps
        .lines()
        .filter_map(|s| s.split_whitespace().last())
        .find(|s| s.contains("/libduckdb.so"))
        .context("cannot locate dynamically loaded libduckdb.so")?;
    Ok(PathBuf::from(path))
}
