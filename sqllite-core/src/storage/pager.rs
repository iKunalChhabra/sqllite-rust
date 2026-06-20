//! Page cache and transaction management.

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_exact_at, write_exact_at, MemoryFile, OpenFlags, Vfs, VfsFile};
use crate::storage::header::DatabaseHeader;
use crate::types::PageNo;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Journal mode for the database.
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

/// Pager open flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct PagerFlags {
    pub omit_journal: bool,
    pub memory: bool,
    pub read_only: bool,
}

/// A cached database page.
#[derive(Debug, Clone)]
pub struct Page {
    pub pgno: PageNo,
    pub data: Vec<u8>,
    pub dirty: bool,
}

/// The pager manages page-level I/O and transactions.
pub struct Pager {
    file: Mutex<Box<dyn VfsFile>>,
    journal: Mutex<Option<Box<dyn VfsFile>>>,
    page_size: u32,
    header: Mutex<DatabaseHeader>,
    cache: Mutex<HashMap<PageNo, Page>>,
    journal_mode: JournalMode,
    in_transaction: Mutex<bool>,
    read_only: bool,
    db_path: Option<PathBuf>,
    page_count: Mutex<u32>,
}

impl Pager {
    pub fn open(
        vfs: &dyn Vfs,
        path: Option<&Path>,
        flags: PagerFlags,
    ) -> Result<Self> {
        let (mut file, db_path, is_new) = if flags.memory || path.is_none() {
            let mem = MemoryFile::new();
            (Box::new(mem) as Box<dyn VfsFile>, None, true)
        } else {
            let path = path.unwrap();
            let exists = vfs.exists(path);
            let open_flags = OpenFlags {
                read_only: flags.read_only,
                create: !exists,
                memory: false,
            };
            let mut file = vfs.open(path, open_flags)?;
            (file, Some(path.to_path_buf()), !exists)
        };

        let size = file.size()?;
        let is_new = size == 0 || is_new;

        let (page_size, header) = if is_new {
            let ps = DEFAULT_PAGE_SIZE;
            let h = DatabaseHeader::new(ps);
            (ps, h)
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
            page_size,
            header: Mutex::new(header),
            cache: Mutex::new(HashMap::new()),
            journal_mode: JournalMode::Delete,
            in_transaction: Mutex::new(false),
            read_only: flags.read_only,
            db_path,
            page_count: Mutex::new(page_count),
        };

        if is_new {
            pager.initialize_new_database()?;
        }

        Ok(pager)
    }

    fn initialize_new_database(&self) -> Result<()> {
        let mut page = vec![0u8; self.page_size as usize];
        {
            let mut header = self.header.lock();
            header.database_size = 1;
            header.write(&mut page);
        }
        // B-tree page header after database header on page 1
        page[100] = PAGE_TYPE_LEAF_TABLE;
        page[101] = 0; // freeblock offset (0 = none)
        page[102] = 0;
        page[103] = 0; // cell count = 0
        page[104] = 0;
        page[105] = 0;
        // content start offset (empty page)
        let content_start = self.page_size as u16;
        page[106] = (content_start >> 8) as u8;
        page[107] = content_start as u8;
        page[108] = 0; // fragmented free bytes
        self.write_page_direct(ROOT_PAGE, &page)?;
        *self.page_count.lock() = 1;
        Ok(())
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
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
        self.journal_mode
    }

    pub fn set_journal_mode(&mut self, mode: JournalMode) {
        self.journal_mode = mode;
    }

    pub fn in_transaction(&self) -> bool {
        *self.in_transaction.lock()
    }

    /// Read a page from cache or disk.
    pub fn get_page(&self, pgno: PageNo) -> Result<Page> {
        if let Some(page) = self.cache.lock().get(&pgno) {
            return Ok(page.clone());
        }
        let mut data = vec![0u8; self.page_size as usize];
        if pgno == 0 || (pgno as u64) > self.page_count() as u64 {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "invalid page number"));
        }
        let offset = (pgno as u64 - 1) * self.page_size as u64;
        read_exact_at(self.file.lock().as_mut(), offset, &mut data)?;
        let page = Page {
            pgno,
            data,
            dirty: false,
        };
        self.cache.lock().insert(pgno, page.clone());
        Ok(page)
    }

    /// Mark a page as dirty after modification.
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
        let offset = (pgno as u64 - 1) * self.page_size as u64;
        write_exact_at(self.file.lock().as_mut(), offset, data)?;
        Ok(())
    }

    /// Allocate a new page.
    pub fn allocate_page(&self) -> Result<PageNo> {
        let mut count = self.page_count.lock();
        *count += 1;
        let pgno = *count;
        let data = vec![0u8; self.page_size as usize];
        let page = Page {
            pgno,
            data,
            dirty: true,
        };
        self.cache.lock().insert(pgno, page);
        Ok(pgno)
    }

    /// Begin a transaction.
    pub fn begin(&self) -> Result<()> {
        let mut tx = self.in_transaction.lock();
        if *tx {
            return Ok(());
        }
        *tx = true;
        Ok(())
    }

    /// Commit a transaction.
    pub fn commit(&self) -> Result<()> {
        let mut tx = self.in_transaction.lock();
        if !*tx {
            return Ok(());
        }

        // Flush dirty pages to disk
        let dirty_pages: Vec<Page> = self
            .cache
            .lock()
            .values()
            .filter(|p| p.dirty)
            .cloned()
            .collect();

        for page in &dirty_pages {
            self.write_page_direct(page.pgno, &page.data)?;
        }

        // Update header on page 1
        if let Some(page1) = self.cache.lock().get(&ROOT_PAGE).cloned() {
            let mut header_page = page1.data.clone();
            {
                let mut header = self.header.lock();
                header.database_size = *self.page_count.lock();
                header.change_counter += 1;
                header.write(&mut header_page);
            }
            self.write_page_direct(ROOT_PAGE, &header_page)?;
        }

        // Mark pages clean
        for page in self.cache.lock().values_mut() {
            page.dirty = false;
        }

        self.file.lock().as_mut().sync()?;
        *tx = false;
        Ok(())
    }

    /// Rollback a transaction.
    pub fn rollback(&self) -> Result<()> {
        self.cache.lock().clear();
        *self.in_transaction.lock() = false;
        Ok(())
    }

    /// Flush all dirty pages without ending transaction.
    pub fn sync(&self) -> Result<()> {
        let dirty: Vec<Page> = self
            .cache
            .lock()
            .values()
            .filter(|p| p.dirty)
            .cloned()
            .collect();
        for page in dirty {
            self.write_page_direct(page.pgno, &page.data)?;
        }
        self.file.lock().as_mut().sync()?;
        Ok(())
    }
}
