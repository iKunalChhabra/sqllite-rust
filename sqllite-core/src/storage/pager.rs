//! Page cache and transaction management.

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_exact_at, write_exact_at, MemoryFile, OpenFlags, UnixVfs, Vfs, VfsFile};
use crate::storage::header::DatabaseHeader;
use crate::storage::wal::{wal_path, Wal};
use crate::types::PageNo;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JournalMode {
    #[default]
    Delete,
    Persist,
    Off,
    Truncate,
    Memory,
    Wal,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PagerFlags {
    pub omit_journal: bool,
    pub memory: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub pgno: PageNo,
    pub data: Vec<u8>,
    pub dirty: bool,
}

pub struct Pager {
    file: Mutex<Box<dyn VfsFile>>,
    journal: Mutex<Option<Box<dyn VfsFile>>>,
    wal: Mutex<Option<Wal>>,
    page_size: Mutex<u32>,
    header: Mutex<DatabaseHeader>,
    cache: Mutex<HashMap<PageNo, Page>>,
    journal_mode: Mutex<JournalMode>,
    in_transaction: Mutex<bool>,
    read_only: bool,
    db_path: Option<PathBuf>,
    page_count: Mutex<u32>,
    memory: bool,
}

impl Pager {
    pub fn open(vfs: &dyn Vfs, path: Option<&Path>, flags: PagerFlags) -> Result<Self> {
        let (mut file, db_path, is_new) = if flags.memory || path.is_none() {
            (Box::new(MemoryFile::new()) as Box<dyn VfsFile>, None, true)
        } else {
            let path = path.unwrap();
            let exists = vfs.exists(path);
            let file = vfs.open(
                path,
                OpenFlags {
                    read_only: flags.read_only,
                    create: !exists,
                    memory: false,
                },
            )?;
            (file, Some(path.to_path_buf()), !exists)
        };

        let size = file.size()?;
        let is_new = size == 0 || is_new;
        let (page_size, header) = if is_new {
            (DEFAULT_PAGE_SIZE, DatabaseHeader::new(DEFAULT_PAGE_SIZE))
        } else {
            let mut hdr_buf = vec![0u8; DATABASE_HEADER_SIZE];
            read_exact_at(file.as_mut(), 0, &mut hdr_buf)?;
            let h = DatabaseHeader::parse(&hdr_buf)?;
            (h.page_size, h)
        };

        let page_count = if is_new {
            0
        } else if header.database_size > 0 {
            header.database_size
        } else {
            ((size + page_size as u64 - 1) / page_size as u64) as u32
        };

        let pager = Self {
            file: Mutex::new(file),
            journal: Mutex::new(None),
            wal: Mutex::new(None),
            page_size: Mutex::new(page_size),
            header: Mutex::new(header),
            cache: Mutex::new(HashMap::new()),
            journal_mode: Mutex::new(JournalMode::Delete),
            in_transaction: Mutex::new(false),
            read_only: flags.read_only,
            db_path,
            page_count: Mutex::new(page_count),
            memory: flags.memory || path.is_none(),
        };

        if is_new {
            pager.initialize_new_database()?;
        }
        Ok(pager)
    }

    fn initialize_new_database(&self) -> Result<()> {
        let page_size = *self.page_size.lock();
        let mut page = vec![0u8; page_size as usize];
        {
            let mut header = self.header.lock();
            header.database_size = 1;
            header.write(&mut page);
        }
        page[100] = PAGE_TYPE_LEAF_TABLE;
        page[101] = 0;
        page[102] = 0;
        page[103] = 0;
        page[104] = 0;
        page[105] = 0;
        let content_start = page_size as u16;
        page[106] = (content_start >> 8) as u8;
        page[107] = content_start as u8;
        page[108] = 0;
        self.write_page_direct(ROOT_PAGE, &page)?;
        *self.page_count.lock() = 1;
        Ok(())
    }

    pub fn page_size(&self) -> u32 {
        *self.page_size.lock()
    }

    pub fn header(&self) -> DatabaseHeader {
        self.header.lock().clone()
    }

    pub fn header_mut(&self) -> parking_lot::MutexGuard<'_, DatabaseHeader> {
        self.header.lock()
    }

    pub fn page_count(&self) -> u32 {
        *self.page_count.lock()
    }

    pub fn journal_mode(&self) -> JournalMode {
        *self.journal_mode.lock()
    }

    pub fn set_journal_mode(&self, mode: JournalMode) -> Result<JournalMode> {
        if self.read_only && mode == JournalMode::Wal {
            return Err(SqlliteError::sql(
                ResultCode::ReadOnly,
                "cannot change journal mode on readonly database",
            ));
        }
        if mode == JournalMode::Wal && !self.memory {
            if let Some(ref db_path) = self.db_path {
                let vfs = UnixVfs;
                *self.wal.lock() = Some(Wal::open(&vfs, &wal_path(db_path), self.page_size())?);
            }
        } else if *self.journal_mode.lock() == JournalMode::Wal && mode != JournalMode::Wal {
            *self.wal.lock() = None;
        }
        *self.journal_mode.lock() = mode;
        Ok(mode)
    }

    pub fn set_page_size(&self, size: u32) -> Result<u32> {
        if self.page_count() > 1 {
            return Err(SqlliteError::sql(
                ResultCode::Error,
                "page_size cannot be changed after data has been written",
            ));
        }
        *self.page_size.lock() = size;
        self.header.lock().page_size = size;
        Ok(size)
    }

    pub fn in_transaction(&self) -> bool {
        *self.in_transaction.lock()
    }

    fn ensure_wal(&self) -> Result<()> {
        if self.journal_mode() != JournalMode::Wal || self.memory {
            return Ok(());
        }
        let mut wal_guard = self.wal.lock();
        if wal_guard.is_some() {
            return Ok(());
        }
        if let Some(ref db_path) = self.db_path {
            let vfs = UnixVfs;
            *wal_guard = Some(Wal::open(&vfs, &wal_path(db_path), self.page_size())?);
        }
        Ok(())
    }

    pub fn get_page(&self, pgno: PageNo) -> Result<Page> {
        if let Some(page) = self.cache.lock().get(&pgno) {
            return Ok(page.clone());
        }
        if pgno == 0 || (pgno as u64) > self.page_count() as u64 {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "invalid page number"));
        }

        let page_size = self.page_size();
        if self.journal_mode() == JournalMode::Wal {
            self.ensure_wal()?;
            if let Some(wal) = self.wal.lock().as_mut() {
                if let Some(wal_data) = wal.read_page(pgno)? {
                    let page = Page {
                        pgno,
                        data: wal_data,
                        dirty: false,
                    };
                    self.cache.lock().insert(pgno, page.clone());
                    return Ok(page);
                }
            }
        }

        let mut data = vec![0u8; page_size as usize];
        let offset = (pgno as u64 - 1) * page_size as u64;
        read_exact_at(self.file.lock().as_mut(), offset, &mut data)?;
        let page = Page {
            pgno,
            data,
            dirty: false,
        };
        self.cache.lock().insert(pgno, page.clone());
        Ok(page)
    }

    pub fn write_page(&self, page: &mut Page) -> Result<()> {
        if self.read_only {
            return Err(SqlliteError::sql(
                ResultCode::ReadOnly,
                "attempt to write a readonly database",
            ));
        }
        page.dirty = true;
        self.cache.lock().insert(page.pgno, page.clone());
        Ok(())
    }

    pub(crate) fn write_page_direct(&self, pgno: PageNo, data: &[u8]) -> Result<()> {
        let offset = (pgno as u64 - 1) * self.page_size() as u64;
        write_exact_at(self.file.lock().as_mut(), offset, data)?;
        Ok(())
    }

    pub fn allocate_page(&self) -> Result<PageNo> {
        let mut count = self.page_count.lock();
        *count += 1;
        let pgno = *count;
        self.cache.lock().insert(
            pgno,
            Page {
                pgno,
                data: vec![0u8; self.page_size() as usize],
                dirty: true,
            },
        );
        Ok(pgno)
    }

    pub fn begin(&self) -> Result<()> {
        let mut tx = self.in_transaction.lock();
        if !*tx {
            *tx = true;
        }
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let mut tx = self.in_transaction.lock();
        if !*tx {
            return Ok(());
        }

        let dirty_pages: Vec<Page> = self
            .cache
            .lock()
            .values()
            .filter(|p| p.dirty)
            .cloned()
            .collect();
        let db_size = *self.page_count.lock();

        if self.journal_mode() == JournalMode::Wal && !self.memory {
            self.ensure_wal()?;
            if let Some(wal) = self.wal.lock().as_mut() {
                for page in &dirty_pages {
                    wal.append_frame(page.pgno, &page.data, db_size)?;
                }
            }
        } else {
            for page in &dirty_pages {
                self.write_page_direct(page.pgno, &page.data)?;
            }
        }

        if let Some(page1) = self.cache.lock().get(&ROOT_PAGE).cloned() {
            let mut header_page = page1.data.clone();
            {
                let mut header = self.header.lock();
                header.database_size = db_size;
                header.change_counter += 1;
                header.write(&mut header_page);
            }
            if self.journal_mode() == JournalMode::Wal && !self.memory {
                if let Some(wal) = self.wal.lock().as_mut() {
                    wal.append_frame(ROOT_PAGE, &header_page, db_size)?;
                    wal.sync()?;
                }
            } else {
                self.write_page_direct(ROOT_PAGE, &header_page)?;
            }
        }

        for page in self.cache.lock().values_mut() {
            page.dirty = false;
        }
        self.file.lock().as_mut().sync()?;
        *tx = false;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        self.cache.lock().clear();
        *self.in_transaction.lock() = false;
        Ok(())
    }

    pub fn sync(&self) -> Result<()> {
        for page in self
            .cache
            .lock()
            .values()
            .filter(|p| p.dirty)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.write_page_direct(page.pgno, &page.data)?;
        }
        self.file.lock().as_mut().sync()?;
        Ok(())
    }

    pub fn wal_checkpoint(&self) -> Result<()> {
        if self.journal_mode() != JournalMode::Wal {
            return Ok(());
        }
        if let Some(wal) = self.wal.lock().as_mut() {
            let mut file = self.file.lock();
            wal.checkpoint(file.as_mut())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn wal_mode_writes_frames() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path();
        let vfs = UnixVfs;
        let pager = Pager::open(
            &vfs,
            Some(path),
            PagerFlags {
                omit_journal: false,
                memory: false,
                read_only: false,
            },
        )
        .unwrap();
        pager.set_journal_mode(JournalMode::Wal).unwrap();
        pager.begin().unwrap();
        let mut page = pager.get_page(ROOT_PAGE).unwrap();
        page.data[100] = 0x42;
        pager.write_page(&mut page).unwrap();
        pager.commit().unwrap();
        assert!(vfs.exists(&wal_path(path)));
        assert!(pager.wal.lock().as_ref().unwrap().frame_count() > 0);
    }

    #[test]
    fn wal_mode_reads_from_wal() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path();
        let vfs = UnixVfs;
        let pager = Pager::open(
            &vfs,
            Some(path),
            PagerFlags {
                omit_journal: false,
                memory: false,
                read_only: false,
            },
        )
        .unwrap();
        pager.set_journal_mode(JournalMode::Wal).unwrap();
        pager.begin().unwrap();
        let mut page = pager.get_page(ROOT_PAGE).unwrap();
        page.data[100] = 0x55;
        pager.write_page(&mut page).unwrap();
        pager.commit().unwrap();
        pager.cache.lock().clear();
        assert_eq!(pager.get_page(ROOT_PAGE).unwrap().data[100], 0x55);
    }
}
