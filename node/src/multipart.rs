//! Minimal in-memory equivalent of Go's `mime/multipart.FileHeader`,
//! covering the parts the Caspar node relies on.

use std::collections::HashMap;
use std::io::Cursor;

/// Describes a file uploaded as part of a multipart request.
#[derive(Debug, Clone, Default)]
pub struct FileHeader {
    pub filename: String,
    pub header: HashMap<String, Vec<String>>,
    pub size: i64,
    /// Full file contents held in memory (Go keeps either a temp file or an
    /// in-memory buffer; the node always reads the whole file, so we buffer it).
    pub content: Vec<u8>,
}

impl FileHeader {
    /// Equivalent of Go's `(*FileHeader).Open()`.
    pub fn open(&self) -> std::io::Result<Cursor<Vec<u8>>> {
        Ok(Cursor::new(self.content.clone()))
    }
}
