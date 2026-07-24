use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Context;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    cancel,
    progress::{Reporter, format_duration},
};

const HASH_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub fn bytes(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

pub fn file(path: &Path) -> anyhow::Result<String> {
    let input = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(input);
    hash_reader(&mut reader, path)
}

pub fn file_with_progress(
    path: &Path,
    label: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<String> {
    reporter.status(&format!("Hashing {label}: {}", path.display()))?;
    let input = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(input);
    let started = Instant::now();
    let mut last_heartbeat = started;
    let mut bytes_read = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        cancel::check()?;
        let count = reader
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(count as u64);
        hasher.update(&buffer[..count]);
        if last_heartbeat.elapsed() >= HASH_HEARTBEAT_INTERVAL {
            reporter.status(&format!(
                "Still hashing {label}: {} read ({})",
                human_bytes(bytes_read),
                format_duration(started.elapsed())
            ))?;
            last_heartbeat = Instant::now();
        }
    }
    reporter.status(&format!(
        "Hashed {label}: {} ({})",
        human_bytes(bytes_read),
        format_duration(started.elapsed())
    ))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_reader(reader: &mut impl Read, path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn json(value: &impl Serialize) -> anyhow::Result<String> {
    Ok(bytes(serde_json::to_vec(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable() {
        assert_eq!(
            bytes("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            json(&serde_json::json!({"a": 1, "b": 2})).unwrap(),
            bytes(r#"{"a":1,"b":2}"#)
        );
    }
}
