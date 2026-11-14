//! Names the engine build, for the script cache's files on disk.
//!
//! Bytecode is only good for the QuickJS it was written by. The cache's
//! keys and files carry `DASHER_ENGINE_ID`, a hash of the engine's sources
//! (`vendor/rquickjs-sys/quickjs`), this crate's version and the target,
//! so a cache directory that outlives a build is not read by the next one.

use std::fs;
use std::path::Path;

fn main() {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/rquickjs-sys/quickjs");
    println!("cargo:rerun-if-changed={}", engine.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut hash = Fnv::default();
    hash.write(env!("CARGO_PKG_VERSION").as_bytes());
    hash.write(std::env::var("TARGET").unwrap_or_default().as_bytes());
    let mut files: Vec<_> = fs::read_dir(&engine)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    for file in files {
        if let Ok(bytes) = fs::read(&file) {
            hash.write(file.file_name().unwrap_or_default().as_encoded_bytes());
            hash.write(&bytes);
        }
    }
    println!(
        "cargo:rustc-env=DASHER_ENGINE_ID={:016x}{:016x}",
        hash.a, hash.b
    );
}

/// Two FNV-1a hashes with different offsets: 128 bits, enough to tell
/// builds apart.
struct Fnv {
    a: u64,
    b: u64,
}

impl Default for Fnv {
    fn default() -> Self {
        Self {
            a: 0xcbf2_9ce4_8422_2325,
            b: 0x6c62_272e_07bb_0142,
        }
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes.iter().chain(&[0xff]) {
            self.a = (self.a ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            self.b = (self.b ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}
