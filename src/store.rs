//! Local library on disk: atomic install, digest verification at rest, index.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{AssetId, AssetMeta, Catalog, Quality};
use crate::transport::{sha256_hex, Download};

/// One installed payload (a specific asset at a specific quality).
#[derive(Clone, Debug)]
pub struct StoredAsset {
    pub id: AssetId,
    pub quality: Quality,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub installed_at: u64,
}

pub struct Store {
    root: PathBuf,
    catalog: Catalog,
    index: Mutex<Vec<StoredAsset>>,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>, catalog: Catalog) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("assets"))?;
        let index = read_index(&root, &catalog)?;
        Ok(Self {
            root,
            catalog,
            index: Mutex::new(index),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn list(&self) -> Vec<StoredAsset> {
        self.index.lock().unwrap().clone()
    }

    pub fn find(&self, id: AssetId, quality: Quality) -> Option<StoredAsset> {
        self.index
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.id == id && row.quality == quality)
            .cloned()
    }

    /// Write the payload to a temporary file, verify it, then publish it by rename.
    ///
    /// A crash between the write and the rename leaves only a `.part` file behind,
    /// never a half-written asset that the index would advertise as usable.
    pub fn install(
        &self,
        meta: &AssetMeta,
        quality: Quality,
        download: &Download,
    ) -> io::Result<StoredAsset> {
        if download.bytes.len() as u64 != meta.bytes(quality) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload size does not match the catalog",
            ));
        }

        let digest = sha256_hex(&download.bytes);
        if digest != download.sha256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "payload changed before it was written",
            ));
        }

        let file_name = format!("{}_{}.bin", meta.id.0, quality.key());
        let final_path = self.root.join("assets").join(&file_name);
        let part_path = self.root.join("assets").join(format!("{file_name}.part"));

        {
            let mut file = fs::File::create(&part_path)?;
            file.write_all(&download.bytes)?;
            file.sync_all()?;
        }

        // Re-read what actually landed on disk: this is the integrity check at rest.
        if sha256_hex(&fs::read(&part_path)?) != digest {
            let _ = fs::remove_file(&part_path);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "written payload failed verification",
            ));
        }

        if final_path.exists() {
            fs::remove_file(&final_path)?;
        }
        fs::rename(&part_path, &final_path)?;

        let row = StoredAsset {
            id: meta.id,
            quality,
            path: final_path,
            sha256: digest,
            bytes: download.bytes.len() as u64,
            installed_at: now_secs(),
        };

        {
            let mut index = self.index.lock().unwrap();
            index.retain(|existing| !(existing.id == meta.id && existing.quality == quality));
            index.push(row.clone());
        }
        self.write_index()?;

        Ok(row)
    }

    pub fn remove(&self, id: AssetId, quality: Quality) -> io::Result<()> {
        let removed = {
            let mut index = self.index.lock().unwrap();
            index
                .iter()
                .position(|row| row.id == id && row.quality == quality)
                .map(|position| index.remove(position))
        };

        if let Some(row) = removed {
            if row.path.exists() {
                fs::remove_file(&row.path)?;
            }
            self.write_index()?;
        }
        Ok(())
    }

    /// True when the file is present and still matches its recorded digest.
    pub fn verify(&self, row: &StoredAsset) -> bool {
        fs::read(&row.path)
            .map(|bytes| sha256_hex(&bytes) == row.sha256)
            .unwrap_or(false)
    }

    fn write_index(&self) -> io::Result<()> {
        let body = {
            let index = self.index.lock().unwrap();
            let mut body = String::from("# id\tquality\tsha256\tbytes\tpath\n");
            for row in index.iter() {
                body.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    row.id.0,
                    row.quality.key(),
                    row.sha256,
                    row.bytes,
                    row.path.display()
                ));
            }
            body
        };

        let temp = self.root.join("index.tsv.part");
        fs::write(&temp, body)?;
        fs::rename(temp, self.root.join("index.tsv"))?;
        Ok(())
    }
}

fn read_index(root: &Path, catalog: &Catalog) -> io::Result<Vec<StoredAsset>> {
    let path = root.join("index.tsv");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(id), Some(quality), Some(sha), Some(bytes), Some(file)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        // Only ids that still exist in the catalog are admitted.
        let Some(known) = catalog.all().iter().find(|a| a.id.0 == id) else {
            continue;
        };
        let quality = match quality {
            "low" => Quality::Low,
            "high" => Quality::High,
            _ => continue,
        };
        let Ok(bytes) = bytes.parse::<u64>() else {
            continue;
        };
        let file_path = PathBuf::from(file);
        if !file_path.exists() {
            continue; // stale row: the file is gone
        }
        rows.push(StoredAsset {
            id: known.id,
            quality,
            path: file_path,
            sha256: sha.to_string(),
            bytes,
            installed_at: 0,
        });
    }
    Ok(rows)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{download, DemoSource};
    use std::sync::atomic::AtomicBool;

    fn temp_root(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "charforge_test_{}_{}_{}",
            tag,
            std::process::id(),
            now_secs()
        ));
        path
    }

    fn meta(id: &str) -> AssetMeta {
        Catalog::load()
            .all()
            .iter()
            .copied()
            .find(|a| a.id.0 == id)
            .expect("catalog entry")
    }

    #[test]
    fn install_persists_and_reloads() {
        let root = temp_root("install");
        let store = Store::open(&root, Catalog::load()).unwrap();
        let entry = meta("car_sedan");
        let cancel = AtomicBool::new(false);
        let payload = download(&entry, Quality::Low, &DemoSource, 4, &cancel, &|_, _| {}).unwrap();

        let stored = store.install(&entry, Quality::Low, &payload).unwrap();
        assert!(stored.path.exists());
        assert!(store.verify(&stored));
        assert_eq!(
            store.find(entry.id, Quality::Low).unwrap().sha256,
            payload.sha256
        );

        let reopened = Store::open(&root, Catalog::load()).unwrap();
        assert_eq!(reopened.list().len(), 1);
        assert!(reopened.find(entry.id, Quality::Low).is_some());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_deletes_file_and_index_row() {
        let root = temp_root("remove");
        let store = Store::open(&root, Catalog::load()).unwrap();
        let entry = meta("cloth_jacket");
        let cancel = AtomicBool::new(false);
        let payload = download(&entry, Quality::High, &DemoSource, 2, &cancel, &|_, _| {}).unwrap();
        store.install(&entry, Quality::High, &payload).unwrap();

        store.remove(entry.id, Quality::High).unwrap();
        assert!(store.find(entry.id, Quality::High).is_none());
        assert!(store.list().is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_file_fails_verification() {
        let root = temp_root("tamper");
        let store = Store::open(&root, Catalog::load()).unwrap();
        let entry = meta("animal_bird");
        let cancel = AtomicBool::new(false);
        let payload = download(&entry, Quality::Low, &DemoSource, 2, &cancel, &|_, _| {}).unwrap();
        let stored = store.install(&entry, Quality::Low, &payload).unwrap();

        let mut bytes = std::fs::read(&stored.path).unwrap();
        bytes[10] ^= 0xFF;
        std::fs::write(&stored.path, bytes).unwrap();

        assert!(!store.verify(&stored));

        let _ = std::fs::remove_dir_all(&root);
    }
}
