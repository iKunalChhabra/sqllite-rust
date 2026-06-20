//! Virtual file system abstraction.

mod unix;

pub use unix::UnixVfs;

use crate::error::{Result, ResultCode, SqlliteError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Open flags for database files.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenFlags {
    pub read_only: bool,
    pub create: bool,
    pub memory: bool,
}

/// A file handle opened through the VFS.
pub trait VfsFile: Send {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize>;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize>;
    fn truncate(&mut self, size: u64) -> Result<()>;
    fn sync(&mut self) -> Result<()>;
    fn size(&self) -> Result<u64>;
    fn path(&self) -> Option<&Path>;
}

/// Virtual file system trait.
pub trait Vfs: Send + Sync {
    fn open(&self, path: &Path, flags: OpenFlags) -> Result<Box<dyn VfsFile>>;
    fn delete(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn name(&self) -> &str;
}

/// In-memory file for testing and :memory: databases.
pub struct MemoryFile {
    data: Vec<u8>,
    path: Option<PathBuf>,
}

impl MemoryFile {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            path: None,
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self {
            data: Vec::new(),
            path: Some(path),
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl Default for MemoryFile {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsFile for MemoryFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let offset = offset as usize;
        if offset >= self.data.len() {
            return Ok(0);
        }
        let end = (offset + buf.len()).min(self.data.len());
        let n = end - offset;
        buf[..n].copy_from_slice(&self.data[offset..end]);
        Ok(n)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
        let offset = offset as usize;
        let end = offset + buf.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[offset..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn truncate(&mut self, size: u64) -> Result<()> {
        self.data.truncate(size as usize);
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        Ok(self.data.len() as u64)
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// Helper to read an exact number of bytes.
pub fn read_exact_at(file: &mut dyn VfsFile, offset: u64, buf: &mut [u8]) -> Result<()> {
    let mut read = 0;
    while read < buf.len() {
        let n = file.read_at(offset + read as u64, &mut buf[read..])?;
        if n == 0 {
            return Err(SqlliteError::sql(ResultCode::IoErr, "unexpected EOF"));
        }
        read += n;
    }
    Ok(())
}

/// Helper to write an exact number of bytes.
pub fn write_exact_at(file: &mut dyn VfsFile, offset: u64, buf: &[u8]) -> Result<()> {
    let mut written = 0;
    while written < buf.len() {
        let n = file.write_at(offset + written as u64, &buf[written..])?;
        if n == 0 {
            return Err(SqlliteError::sql(ResultCode::IoErr, "write failed"));
        }
        written += n;
    }
    Ok(())
}

/// Read a big-endian u16 from a buffer.
pub fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// Write a big-endian u16 to a buffer.
pub fn write_u16_be(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

/// Read a big-endian u32 from a buffer.
pub fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Write a big-endian u32 to a buffer.
pub fn write_u32_be(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
