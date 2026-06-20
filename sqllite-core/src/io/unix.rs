//! Unix platform VFS implementation.

use super::{OpenFlags, Vfs, VfsFile};
use crate::error::{Result, ResultCode, SqlliteError};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Default Unix VFS.
pub struct UnixVfs;

impl Vfs for UnixVfs {
    fn open(&self, path: &Path, flags: OpenFlags) -> Result<Box<dyn VfsFile>> {
        let mut opts = OpenOptions::new();
        if flags.read_only {
            opts.read(true);
        } else {
            opts.read(true).write(true);
            if flags.create {
                opts.create(true);
            }
        }
        let file = opts.open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SqlliteError::sql(ResultCode::CantOpen, format!("unable to open database file: {e}"))
            } else {
                SqlliteError::Io(e)
            }
        })?;
        Ok(Box::new(UnixFile {
            file,
            path: path.to_path_buf(),
        }))
    }

    fn delete(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).map_err(SqlliteError::Io)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn name(&self) -> &str {
        "unix"
    }
}

struct UnixFile {
    file: File,
    path: PathBuf,
}

impl VfsFile for UnixFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset)).map_err(SqlliteError::Io)?;
        self.file.read(buf).map_err(SqlliteError::Io)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<usize> {
        self.file.seek(SeekFrom::Start(offset)).map_err(SqlliteError::Io)?;
        self.file.write(buf).map_err(SqlliteError::Io)
    }

    fn truncate(&mut self, size: u64) -> Result<()> {
        self.file.set_len(size).map_err(SqlliteError::Io)
    }

    fn sync(&mut self) -> Result<()> {
        self.file.sync_all().map_err(SqlliteError::Io)
    }

    fn size(&self) -> Result<u64> {
        Ok(self.file.metadata().map_err(SqlliteError::Io)?.len())
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}
