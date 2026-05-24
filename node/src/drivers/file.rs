//! Translation of `drivers/file/file.go`.
//!
//! `File` implements the [`IFile`] driver — filesystem-backed storage for
//! tar entries, uploaded `FileHeader`s, and arbitrary byte blobs. The
//! "topic-id" scoped variants live under `<storage_root>/files/<topic>/`; the
//! "global" variants live directly under `<storage_root>/`.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{anyhow, Result};

use crate::models::ports::file::IFile;
use crate::compat::multipart::FileHeader;

/// Filesystem-backed [`IFile`] implementation. Stateless apart from the
/// constructor ensuring `<storage_root>/files` exists.
#[derive(Debug, Default)]
pub struct File;

impl File {
    /// `NewFileTool` — pre-creates `<storage_root>/files`.
    pub fn new(storage_root: &str) -> Self {
        let dir = format!("{}/files", storage_root);
        let _ = fs::create_dir_all(&dir);
        File
    }
}

fn topic_dir(storage_root: &str, topic_id: &str) -> String {
    format!("{}/files/{}", storage_root, topic_id)
}

fn write_file(path: &str, data: &[u8], overwrite: bool) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if overwrite {
        opts.truncate(true);
    } else {
        opts.append(true);
    }
    let mut f = opts.open(path).map_err(|e| anyhow!("open {}: {}", path, e))?;
    f.write_all(data)
        .map_err(|e| anyhow!("write {}: {}", path, e))?;
    Ok(())
}

impl IFile for File {
    fn check_file_from_storage(&self, storage_root: &str, topic_id: &str, key: &str) -> bool {
        Path::new(&format!("{}/{}", topic_dir(storage_root, topic_id), key)).exists()
    }

    fn save_file_to_storage(
        &self,
        storage_root: &str,
        fh: &FileHeader,
        topic_id: &str,
        key: &str,
    ) -> Result<()> {
        let dir = topic_dir(storage_root, topic_id);
        fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {}", dir, e))?;
        write_file(&format!("{}/{}", dir, key), &fh.content, false)
    }

    fn save_data_to_storage(
        &self,
        storage_root: &str,
        data: &[u8],
        topic_id: &str,
        key: &str,
        flag: &[bool],
    ) -> Result<()> {
        let dir = topic_dir(storage_root, topic_id);
        fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {}", dir, e))?;
        let overwrite = matches!(flag.first(), Some(true));
        write_file(&format!("{}/{}", dir, key), data, overwrite)
    }

    fn save_tar_file_item_to_storage(
        &self,
        storage_root: &str,
        reader: &mut dyn Read,
        topic_id: &str,
        key: &str,
    ) -> Result<()> {
        let dir = topic_dir(storage_root, topic_id);
        fs::create_dir_all(&dir).map_err(|e| anyhow!("mkdir {}: {}", dir, e))?;
        let path = format!("{}/{}", dir, key);
        let mut dest = OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| anyhow!("open {}: {}", path, e))?;
        std::io::copy(reader, &mut dest)
            .map_err(|e| anyhow!("copy {}: {}", path, e))?;
        Ok(())
    }

    fn read_file_from_storage(
        &self,
        storage_root: &str,
        topic_id: &str,
        key: &str,
    ) -> Result<Vec<u8>> {
        fs::read(format!("{}/{}", topic_dir(storage_root, topic_id), key))
            .map_err(|e| anyhow!("read: {}", e))
    }

    fn check_file_from_global_storage(&self, storage_root: &str, key: &str) -> bool {
        Path::new(&format!("{}/{}", storage_root, key)).exists()
    }

    fn read_file_from_global_storage(&self, storage_root: &str, key: &str) -> Result<String> {
        let bytes = fs::read(format!("{}/{}", storage_root, key))
            .map_err(|e| anyhow!("read: {}", e))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn save_file_to_global_storage(
        &self,
        storage_root: &str,
        fh: &FileHeader,
        key: &str,
        overwrite: bool,
    ) -> Result<()> {
        fs::create_dir_all(storage_root)
            .map_err(|e| anyhow!("mkdir {}: {}", storage_root, e))?;
        let path = format!("{}/{}", storage_root, key);
        if self.check_file_from_global_storage(storage_root, key) {
            let _ = fs::remove_file(&path);
        }
        write_file(&path, &fh.content, overwrite)
    }

    fn save_data_to_global_storage(
        &self,
        storage_root: &str,
        data: &[u8],
        key: &str,
        overwrite: bool,
    ) -> Result<()> {
        fs::create_dir_all(storage_root)
            .map_err(|e| anyhow!("mkdir {}: {}", storage_root, e))?;
        let path = format!("{}/{}", storage_root, key);
        if overwrite {
            let _ = fs::remove_file(&path);
        }
        write_file(&path, data, overwrite)
    }

    fn delete_file_from_global_storage(
        &self,
        storage_root: &str,
        key: &str,
        _overwrite: bool,
    ) -> Result<()> {
        let path = format!("{}/{}", storage_root, key);
        fs::remove_file(&path).map_err(|e| anyhow!("remove {}: {}", path, e))?;
        Ok(())
    }

    fn read_file_by_path(&self, path: &str) -> Result<Vec<u8>> {
        fs::read(path).map_err(|e| anyhow!("read {}: {}", path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = format!("/tmp/caspar-file-{}-{}", std::process::id(), nanos);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_topic_data() {
        let root = tmp_root();
        let f = File::new(&root);
        f.save_data_to_storage(&root, b"hello", "topic-1", "k", &[true])
            .unwrap();
        assert!(f.check_file_from_storage(&root, "topic-1", "k"));
        let v = f.read_file_from_storage(&root, "topic-1", "k").unwrap();
        assert_eq!(v, b"hello");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn round_trip_global() {
        let root = tmp_root();
        let f = File::new(&root);
        f.save_data_to_global_storage(&root, b"data", "key.txt", true)
            .unwrap();
        assert!(f.check_file_from_global_storage(&root, "key.txt"));
        assert_eq!(
            f.read_file_from_global_storage(&root, "key.txt").unwrap(),
            "data"
        );
        f.delete_file_from_global_storage(&root, "key.txt", true).unwrap();
        assert!(!f.check_file_from_global_storage(&root, "key.txt"));
        let _ = fs::remove_dir_all(&root);
    }
}
