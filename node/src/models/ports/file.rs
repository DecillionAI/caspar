use anyhow::Result;
use std::io::Read;

use crate::compat::multipart::FileHeader;

/// The file storage driver interface.
pub trait IFile: Send + Sync {
    fn check_file_from_storage(&self, storage_root: &str, store_id: &str, key: &str) -> bool;
    fn save_file_to_storage(
        &self,
        storage_root: &str,
        fh: &FileHeader,
        store_id: &str,
        key: &str,
    ) -> Result<()>;
    fn save_data_to_storage(
        &self,
        storage_root: &str,
        data: &[u8],
        store_id: &str,
        key: &str,
        flag: &[bool],
    ) -> Result<()>;
    /// Save a single tar entry; the reader is expected to be already
    /// positioned at the entry's data.
    fn save_tar_file_item_to_storage(
        &self,
        storage_root: &str,
        reader: &mut dyn Read,
        store_id: &str,
        key: &str,
    ) -> Result<()>;
    fn read_file_from_storage(
        &self,
        storage_root: &str,
        store_id: &str,
        key: &str,
    ) -> Result<Vec<u8>>;
    fn check_file_from_global_storage(&self, storage_root: &str, key: &str) -> bool;
    fn read_file_from_global_storage(&self, storage_root: &str, key: &str) -> Result<String>;
    fn save_file_to_global_storage(
        &self,
        storage_root: &str,
        fh: &FileHeader,
        key: &str,
        overwrite: bool,
    ) -> Result<()>;
    fn save_data_to_global_storage(
        &self,
        storage_root: &str,
        data: &[u8],
        key: &str,
        overwrite: bool,
    ) -> Result<()>;
    fn delete_file_from_global_storage(
        &self,
        storage_root: &str,
        key: &str,
        overwrite: bool,
    ) -> Result<()>;
    fn read_file_by_path(&self, path: &str) -> Result<Vec<u8>>;
}
