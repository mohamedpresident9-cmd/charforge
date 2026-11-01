//! Chunked transport: a manifest of parts, parallel fetch, per-part integrity,
//! whole-payload integrity, cancellation.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::model::{AssetMeta, Quality};

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(hasher.finalize().as_slice())
}

/// What the "server" promises about one payload: its parts and its final digest.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub total_bytes: u64,
    pub chunk_size: u32,
    pub chunk_sha256: Vec<String>,
    pub root_sha256: String,
}

impl Manifest {
    pub fn chunk_count(&self) -> u32 {
        self.chunk_sha256.len() as u32
    }
}

/// A remote origin. The demo origin generates deterministic bytes so the studio
/// runs with no network; a real origin only has to implement these two methods.
pub trait Source: Send + Sync {
    fn manifest(&self, meta: &AssetMeta, quality: Quality) -> Result<Manifest, String>;
    fn fetch_chunk(
        &self,
        meta: &AssetMeta,
        quality: Quality,
        index: u32,
    ) -> Result<Vec<u8>, String>;
}

/// Deterministic, in-process origin: same seed always yields the same payload.
pub struct DemoSource;

impl DemoSource {
    fn chunk_seed(meta: &AssetMeta, quality: Quality, index: u32) -> u64 {
        let mut x = meta.seed
            ^ (quality as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (index as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        // A short splitmix-style scramble keeps neighbouring parts uncorrelated.
        x ^= x >> 30;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    fn generate(seed: u64, len: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(len as usize);
        let mut state = seed | 1;
        while out.len() < len as usize {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bytes = state.to_le_bytes();
            let take = (len as usize - out.len()).min(bytes.len());
            out.extend_from_slice(&bytes[..take]);
        }
        out
    }
}

impl Source for DemoSource {
    fn manifest(&self, meta: &AssetMeta, quality: Quality) -> Result<Manifest, String> {
        let total = meta.bytes(quality);
        let chunk_size = meta.chunk_size(quality);
        let count = meta.chunk_count(quality);

        let mut root = Sha256::new();
        let mut chunk_sha256 = Vec::with_capacity(count as usize);
        for index in 0..count {
            let len = meta.chunk_len(quality, index);
            let part = Self::generate(Self::chunk_seed(meta, quality, index), len);
            root.update(&part);
            chunk_sha256.push(sha256_hex(&part));
        }

        Ok(Manifest {
            total_bytes: total,
            chunk_size,
            chunk_sha256,
            root_sha256: hex(root.finalize().as_slice()),
        })
    }

    fn fetch_chunk(
        &self,
        meta: &AssetMeta,
        quality: Quality,
        index: u32,
    ) -> Result<Vec<u8>, String> {
        if index >= meta.chunk_count(quality) {
            return Err(format!("chunk {index} is out of range"));
        }
        let len = meta.chunk_len(quality, index);
        Ok(Self::generate(Self::chunk_seed(meta, quality, index), len))
    }
}

/// Result of a completed transfer.
#[derive(Clone, Debug)]
pub struct Download {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub chunks: u32,
}

/// Fetch every part in parallel, verify each part, then verify the whole payload.
///
/// `workers` caps how many parts are in flight at once. Parts are independent,
/// so a slow part never blocks the others; the last part is usually shorter and
/// is handled by the same code path.
pub fn download(
    meta: &AssetMeta,
    quality: Quality,
    source: &dyn Source,
    workers: usize,
    cancel: &AtomicBool,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<Download, String> {
    let manifest = source.manifest(meta, quality)?;
    let expected_total = meta.bytes(quality);

    if manifest.total_bytes != expected_total {
        return Err(format!(
            "manifest size {} != catalog size {}",
            manifest.total_bytes, expected_total
        ));
    }

    let count = manifest.chunk_count();
    if count == 0 {
        return Err("manifest has no parts".to_string());
    }

    let parts: Vec<Mutex<Option<Vec<u8>>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    let workers = workers.max(1).min(count as usize);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let index = next.fetch_add(1, Ordering::Relaxed) as u32;
                if index >= count {
                    return;
                }

                match source.fetch_chunk(meta, quality, index) {
                    Ok(bytes) => {
                        let expected_len = meta.chunk_len(quality, index) as usize;
                        if bytes.len() != expected_len {
                            errors.lock().unwrap().push(format!(
                                "part {index}: {} bytes, expected {expected_len}",
                                bytes.len()
                            ));
                            return;
                        }
                        let digest = sha256_hex(&bytes);
                        if digest != manifest.chunk_sha256[index as usize] {
                            errors
                                .lock()
                                .unwrap()
                                .push(format!("part {index}: digest mismatch"));
                            return;
                        }
                        done.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        progress(done.load(Ordering::Relaxed), expected_total);
                        *parts[index as usize].lock().unwrap() = Some(bytes);
                    }
                    Err(error) => {
                        errors.lock().unwrap().push(format!("part {index}: {error}"));
                        return;
                    }
                }
            });
        }
    });

    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }

    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    let mut bytes = Vec::with_capacity(expected_total as usize);
    for (index, slot) in parts.into_iter().enumerate() {
        match slot.into_inner().unwrap() {
            Some(part) => bytes.extend_from_slice(&part),
            None => return Err(format!("part {index} was never delivered")),
        }
    }

    if bytes.len() as u64 != expected_total {
        return Err(format!(
            "assembled {} bytes, expected {expected_total}",
            bytes.len()
        ));
    }

    let root = sha256_hex(&bytes);
    if root != manifest.root_sha256 {
        return Err("assembled payload digest mismatch".to_string());
    }

    Ok(Download {
        bytes,
        sha256: root,
        chunks: count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Catalog;
    use std::sync::atomic::AtomicBool;

    fn meta(id: &str) -> AssetMeta {
        Catalog::load()
            .all()
            .iter()
            .copied()
            .find(|a| a.id.0 == id)
            .expect("catalog entry")
    }

    #[test]
    fn manifest_matches_catalog_geometry() {
        let source = DemoSource;
        for quality in Quality::ALL {
            for entry in Catalog::load().all() {
                let manifest = source.manifest(entry, quality).unwrap();
                assert_eq!(manifest.total_bytes, entry.bytes(quality));
                assert_eq!(manifest.chunk_count(), entry.chunk_count(quality));
                assert_eq!(
                    manifest.chunk_sha256.len(),
                    entry.chunk_count(quality) as usize
                );
            }
        }
    }

    #[test]
    fn download_assembles_exact_payload() {
        let source = DemoSource;
        let entry = meta("animal_horse");
        for quality in Quality::ALL {
            let cancel = AtomicBool::new(false);
            let result = download(&entry, quality, &source, 4, &cancel, &|_, _| {}).unwrap();
            assert_eq!(result.bytes.len() as u64, entry.bytes(quality));
            let manifest = source.manifest(&entry, quality).unwrap();
            assert_eq!(result.sha256, manifest.root_sha256);
            assert_eq!(result.chunks, entry.chunk_count(quality));
        }
    }

    #[test]
    fn determinism_holds_across_runs() {
        let source = DemoSource;
        let entry = meta("building_tower");
        let a = source.manifest(&entry, Quality::High).unwrap();
        let b = source.manifest(&entry, Quality::High).unwrap();
        assert_eq!(a.root_sha256, b.root_sha256);
    }

    #[test]
    fn corrupted_part_is_rejected() {
        struct Corrupt;
        impl Source for Corrupt {
            fn manifest(&self, meta: &AssetMeta, quality: Quality) -> Result<Manifest, String> {
                DemoSource.manifest(meta, quality)
            }
            fn fetch_chunk(
                &self,
                meta: &AssetMeta,
                quality: Quality,
                index: u32,
            ) -> Result<Vec<u8>, String> {
                let mut bytes = DemoSource.fetch_chunk(meta, quality, index)?;
                if index == 0 && !bytes.is_empty() {
                    bytes[0] ^= 0xFF;
                }
                Ok(bytes)
            }
        }

        let entry = meta("human_athlete");
        let cancel = AtomicBool::new(false);
        let error = download(&entry, Quality::Low, &Corrupt, 2, &cancel, &|_, _| {})
            .expect_err("corruption must fail the transfer");
        assert!(error.contains("digest mismatch"), "unexpected: {error}");
    }

    #[test]
    fn cancellation_is_honoured() {
        let entry = meta("building_tower");
        let cancel = AtomicBool::new(true);
        let error = download(&entry, Quality::High, &DemoSource, 4, &cancel, &|_, _| {})
            .expect_err("cancelled transfer must fail");
        assert_eq!(error, "cancelled");
    }
}
