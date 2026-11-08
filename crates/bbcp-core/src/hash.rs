//! Streaming content hashes.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// SHA-256 of the whole file. Memory use stays constant.
pub fn sha256_file(path: &Path) -> Result<String> {
    let f = File::open(path).with_context(|| format!("abriendo {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1 << 20, f);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("leyendo {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Cheap fingerprint of the first 256 bytes, used only as a pre-filter.
pub fn prefix_hash(path: &Path) -> Result<String> {
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut buf = [0u8; 256];
    let n = reader.read(&mut buf)?;
    let mut hasher = Sha256::new();
    hasher.update(&buf[..n]);
    Ok(hex::encode(hasher.finalize()))
}
