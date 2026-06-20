//! Write-Ahead Log (WAL) file format read/write.

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_exact_at, read_u32_be, write_exact_at, write_u32_be, OpenFlags, Vfs, VfsFile};
use crate::types::PageNo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const WAL_FORMAT_VERSION: u32 = 3007000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalHeader {
    pub magic: u32,
    pub version: u32,
    pub page_size: u32,
    pub checkpoint_seq: u32,
    pub salt1: u32,
    pub salt2: u32,
    pub checksum1: u32,
    pub checksum2: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalFrameHeader {
    pub pgno: PageNo,
    pub db_size: u32,
    pub salt1: u32,
    pub salt2: u32,
    pub checksum1: u32,
    pub checksum2: u32,
}

fn wal_checksum(salt: u32, data: &[u32], checksum: &mut [u32; 2]) {
    let mut s1 = checksum[0].wrapping_add(salt).wrapping_add(data.len() as u32);
    let mut s2 = checksum[1].wrapping_add(salt).wrapping_add(data.len() as u32 * 2);
    for &word in data {
        s1 = s1.wrapping_add(word);
        s2 = s2.wrapping_add(s1);
    }
    checksum[0] = s1;
    checksum[1] = s2;
}

fn bytes_to_u32_be(data: &[u8]) -> Vec<u32> {
    data.chunks_exact(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

impl WalHeader {
    pub fn new(page_size: u32, salt1: u32, salt2: u32) -> Self {
        let mut header = Self {
            magic: WAL_MAGIC,
            version: WAL_FORMAT_VERSION,
            page_size,
            checkpoint_seq: 0,
            salt1,
            salt2,
            checksum1: 0,
            checksum2: 0,
        };
        header.update_checksum();
        header
    }

    fn header_words(&self) -> [u32; 8] {
        [
            self.magic,
            self.version,
            self.page_size,
            self.checkpoint_seq,
            self.salt1,
            self.salt2,
            0,
            0,
        ]
    }

    pub fn update_checksum(&mut self) {
        let mut checksum = [0u32, 0u32];
        wal_checksum(0, &self.header_words(), &mut checksum);
        self.checksum1 = checksum[0];
        self.checksum2 = checksum[1];
    }

    pub fn verify_checksum(&self) -> bool {
        let mut checksum = [0u32, 0u32];
        wal_checksum(0, &self.header_words(), &mut checksum);
        checksum[0] == self.checksum1 && checksum[1] == self.checksum2
    }

    pub fn encode(&self) -> [u8; WAL_HEADER_SIZE] {
        let mut buf = [0u8; WAL_HEADER_SIZE];
        write_u32_be(&mut buf, 0, self.magic);
        write_u32_be(&mut buf, 4, self.version);
        write_u32_be(&mut buf, 8, self.page_size);
        write_u32_be(&mut buf, 12, self.checkpoint_seq);
        write_u32_be(&mut buf, 16, self.salt1);
        write_u32_be(&mut buf, 20, self.salt2);
        write_u32_be(&mut buf, 24, self.checksum1);
        write_u32_be(&mut buf, 28, self.checksum2);
        buf
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < WAL_HEADER_SIZE {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "WAL header too short"));
        }
        let header = Self {
            magic: read_u32_be(data, 0),
            version: read_u32_be(data, 4),
            page_size: read_u32_be(data, 8),
            checkpoint_seq: read_u32_be(data, 12),
            salt1: read_u32_be(data, 16),
            salt2: read_u32_be(data, 20),
            checksum1: read_u32_be(data, 24),
            checksum2: read_u32_be(data, 28),
        };
        if header.magic != WAL_MAGIC && header.magic != WAL_MAGIC_LE {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "invalid WAL magic"));
        }
        if header.version != WAL_FORMAT_VERSION {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "unsupported WAL version",
            ));
        }
        if !header.verify_checksum() {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "WAL header checksum mismatch",
            ));
        }
        Ok(header)
    }
}

impl WalFrameHeader {
    pub fn encode(&self) -> [u8; WAL_FRAME_HEADER_SIZE] {
        let mut buf = [0u8; WAL_FRAME_HEADER_SIZE];
        write_u32_be(&mut buf, 0, self.pgno);
        write_u32_be(&mut buf, 4, self.db_size);
        write_u32_be(&mut buf, 8, self.salt1);
        write_u32_be(&mut buf, 12, self.salt2);
        write_u32_be(&mut buf, 16, self.checksum1);
        write_u32_be(&mut buf, 20, self.checksum2);
        buf
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < WAL_FRAME_HEADER_SIZE {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "WAL frame header too short",
            ));
        }
        Ok(Self {
            pgno: read_u32_be(data, 0),
            db_size: read_u32_be(data, 4),
            salt1: read_u32_be(data, 8),
            salt2: read_u32_be(data, 12),
            checksum1: read_u32_be(data, 16),
            checksum2: read_u32_be(data, 20),
        })
    }

    fn frame_words(&self) -> [u32; 6] {
        [self.pgno, self.db_size, self.salt1, self.salt2, 0, 0]
    }

    pub fn compute_checksum(&self, page_data: &[u8], is_le: bool) -> (u32, u32) {
        let mut checksum = [0u32, 0u32];
        wal_checksum(self.salt1, &self.frame_words(), &mut checksum);
        wal_checksum(self.salt2, &bytes_to_u32_be(page_data), &mut checksum);
        if is_le {
            (checksum[0].swap_bytes(), checksum[1].swap_bytes())
        } else {
            (checksum[0], checksum[1])
        }
    }

    pub fn verify_checksum(&self, page_data: &[u8], is_le: bool) -> bool {
        let (c1, c2) = self.compute_checksum(page_data, is_le);
        c1 == self.checksum1 && c2 == self.checksum2
    }
}

pub struct Wal {
    file: Box<dyn VfsFile>,
    header: WalHeader,
    page_size: u32,
    frame_count: u32,
    page_index: HashMap<PageNo, u32>,
    is_le: bool,
}

impl Wal {
    pub fn create(vfs: &dyn Vfs, path: &Path, page_size: u32) -> Result<Self> {
        let mut file = vfs.open(
            path,
            OpenFlags {
                read_only: false,
                create: true,
                memory: false,
            },
        )?;
        let header = WalHeader::new(page_size, 0x12345678, 0x9abcdef0);
        write_exact_at(file.as_mut(), 0, &header.encode())?;
        file.as_mut().sync()?;
        Ok(Self {
            file,
            header,
            page_size,
            frame_count: 0,
            page_index: HashMap::new(),
            is_le: false,
        })
    }

    pub fn open(vfs: &dyn Vfs, path: &Path, page_size: u32) -> Result<Self> {
        if !vfs.exists(path) {
            return Self::create(vfs, path, page_size);
        }
        let mut file = vfs.open(
            path,
            OpenFlags {
                read_only: false,
                create: false,
                memory: false,
            },
        )?;
        let size = file.size()?;
        if size == 0 {
            return Self::create(vfs, path, page_size);
        }
        let mut hdr_buf = [0u8; WAL_HEADER_SIZE];
        read_exact_at(file.as_mut(), 0, &mut hdr_buf)?;
        let header = WalHeader::decode(&hdr_buf)?;
        if header.page_size != page_size {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "WAL page size does not match database",
            ));
        }
        let is_le = header.magic == WAL_MAGIC_LE;
        let mut wal = Self {
            file,
            header,
            page_size,
            frame_count: 0,
            page_index: HashMap::new(),
            is_le,
        };
        wal.rebuild_index()?;
        Ok(wal)
    }

    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    fn frame_offset(&self, frame_idx: u32) -> u64 {
        WAL_HEADER_SIZE as u64
            + (frame_idx as u64) * (WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64)
    }

    fn rebuild_index(&mut self) -> Result<()> {
        self.page_index.clear();
        let size = self.file.size()?;
        if size <= WAL_HEADER_SIZE as u64 {
            self.frame_count = 0;
            return Ok(());
        }
        let frame_size = WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64;
        let n_frames = ((size - WAL_HEADER_SIZE as u64) / frame_size) as u32;
        let mut hdr_buf = [0u8; WAL_FRAME_HEADER_SIZE];
        let mut page_buf = vec![0u8; self.page_size as usize];
        for i in 0..n_frames {
            let offset = self.frame_offset(i);
            read_exact_at(self.file.as_mut(), offset, &mut hdr_buf)?;
            let frame_hdr = WalFrameHeader::decode(&hdr_buf)?;
            if frame_hdr.salt1 != self.header.salt1 || frame_hdr.salt2 != self.header.salt2 {
                break;
            }
            read_exact_at(
                self.file.as_mut(),
                offset + WAL_FRAME_HEADER_SIZE as u64,
                &mut page_buf,
            )?;
            if !frame_hdr.verify_checksum(&page_buf, self.is_le) {
                break;
            }
            if frame_hdr.pgno > 0 {
                self.page_index.insert(frame_hdr.pgno, i);
            }
            self.frame_count = i + 1;
        }
        Ok(())
    }

    pub fn append_frame(&mut self, pgno: PageNo, page_data: &[u8], db_size: u32) -> Result<()> {
        if page_data.len() != self.page_size as usize {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "WAL page size mismatch",
            ));
        }
        let mut frame_hdr = WalFrameHeader {
            pgno,
            db_size,
            salt1: self.header.salt1,
            salt2: self.header.salt2,
            checksum1: 0,
            checksum2: 0,
        };
        let (c1, c2) = frame_hdr.compute_checksum(page_data, self.is_le);
        frame_hdr.checksum1 = c1;
        frame_hdr.checksum2 = c2;
        let offset = self.frame_offset(self.frame_count);
        write_exact_at(self.file.as_mut(), offset, &frame_hdr.encode())?;
        write_exact_at(
            self.file.as_mut(),
            offset + WAL_FRAME_HEADER_SIZE as u64,
            page_data,
        )?;
        self.page_index.insert(pgno, self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    pub fn read_page(&mut self, pgno: PageNo) -> Result<Option<Vec<u8>>> {
        let frame_idx = match self.page_index.get(&pgno) {
            Some(&idx) => idx,
            None => return Ok(None),
        };
        let offset = self.frame_offset(frame_idx);
        let mut hdr_buf = [0u8; WAL_FRAME_HEADER_SIZE];
        read_exact_at(self.file.as_mut(), offset, &mut hdr_buf)?;
        let frame_hdr = WalFrameHeader::decode(&hdr_buf)?;
        let mut page_buf = vec![0u8; self.page_size as usize];
        read_exact_at(
            self.file.as_mut(),
            offset + WAL_FRAME_HEADER_SIZE as u64,
            &mut page_buf,
        )?;
        if !frame_hdr.verify_checksum(&page_buf, self.is_le) {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "WAL frame checksum mismatch",
            ));
        }
        Ok(Some(page_buf))
    }

    pub fn sync(&mut self) -> Result<()> {
        self.file.as_mut().sync()
    }

    pub fn checkpoint(&mut self, db_file: &mut dyn VfsFile) -> Result<()> {
        let frames: Vec<(PageNo, u32)> = self
            .page_index
            .iter()
            .map(|(&pgno, &idx)| (pgno, idx))
            .collect();
        for (pgno, frame_idx) in frames {
            let offset = self.frame_offset(frame_idx);
            let mut page_buf = vec![0u8; self.page_size as usize];
            read_exact_at(
                self.file.as_mut(),
                offset + WAL_FRAME_HEADER_SIZE as u64,
                &mut page_buf,
            )?;
            let db_offset = (pgno as u64 - 1) * self.page_size as u64;
            write_exact_at(db_file, db_offset, &page_buf)?;
        }
        self.header.checkpoint_seq = self.header.checkpoint_seq.wrapping_add(1);
        self.header.salt1 = self.header.salt1.wrapping_add(1);
        self.header.salt2 = self.header.salt2.wrapping_add(1);
        self.header.update_checksum();
        write_exact_at(self.file.as_mut(), 0, &self.header.encode())?;
        self.file.as_mut().truncate(WAL_HEADER_SIZE as u64)?;
        self.frame_count = 0;
        self.page_index.clear();
        self.file.as_mut().sync()?;
        db_file.sync()
    }
}

pub fn wal_path(db_path: &Path) -> PathBuf {
    let mut path = db_path.as_os_str().to_os_string();
    path.push("-wal");
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{MemoryFile, UnixVfs};
    use tempfile::NamedTempFile;

    #[test]
    fn wal_header_roundtrip() {
        let header = WalHeader::new(4096, 1, 2);
        let decoded = WalHeader::decode(&header.encode()).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn wal_append_and_read() {
        let tmp = NamedTempFile::new().unwrap();
        let wal_file = tmp.path().with_extension("db-wal");
        let vfs = UnixVfs;
        let mut wal = Wal::create(&vfs, &wal_file, 4096).unwrap();
        let page = vec![0xAB; 4096];
        wal.append_frame(1, &page, 1).unwrap();
        wal.sync().unwrap();
        let mut wal2 = Wal::open(&vfs, &wal_file, 4096).unwrap();
        assert_eq!(wal2.frame_count(), 1);
        assert_eq!(wal2.read_page(1).unwrap().unwrap(), page);
    }

    #[test]
    fn wal_checkpoint_stub() {
        let tmp = NamedTempFile::new().unwrap();
        let wal_file = tmp.path().with_extension("db-wal");
        let vfs = UnixVfs;
        let mut wal = Wal::create(&vfs, &wal_file, 4096).unwrap();
        let page = vec![0xCD; 4096];
        wal.append_frame(1, &page, 1).unwrap();
        let mut db = MemoryFile::new();
        wal.checkpoint(&mut db).unwrap();
        let mut read = vec![0u8; 4096];
        read_exact_at(&mut db, 0, &mut read).unwrap();
        assert_eq!(read, page);
        assert_eq!(wal.frame_count(), 0);
    }
}
