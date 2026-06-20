//! B-tree storage engine.

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_u16_be, write_u16_be, Vfs, VfsFile};
use crate::record::{decode_record, encode_record};
use crate::storage::pager::{Page, Pager};
use crate::types::{PageNo, Value};
use crate::varint::{read_varint, write_varint};

/// B-tree open flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct BtreeFlags {
    pub intkey: bool,
    pub blobkey: bool,
}

/// A B-tree table or index.
pub struct Btree {
    pager: std::sync::Arc<Pager>,
    root_page: PageNo,
    flags: BtreeFlags,
}

/// Cursor for traversing B-tree pages.
pub struct BtreeCursor {
    btree: Btree,
    pgno: PageNo,
    cell_index: usize,
    end_of_table: bool,
    current_key: Option<i64>,
    current_record: Option<Vec<u8>>,
}

impl Btree {
    pub fn new(pager: std::sync::Arc<Pager>, root_page: PageNo, flags: BtreeFlags) -> Self {
        Self {
            pager,
            root_page,
            flags,
        }
    }

    pub fn pager(&self) -> &Pager {
        &self.pager
    }

    pub fn root_page(&self) -> PageNo {
        self.root_page
    }

    pub fn create_table(pager: std::sync::Arc<Pager>) -> Result<(Self, PageNo)> {
        let pgno = pager.allocate_page()?;
        let page_size = pager.page_size() as usize;
        let mut data = vec![0u8; page_size];
        let header_offset = if pgno == ROOT_PAGE {
            DATABASE_HEADER_SIZE
        } else {
            0
        };
        data[header_offset] = PAGE_TYPE_LEAF_TABLE;
        let content_start = page_size as u16;
        data[header_offset + 5] = (content_start >> 8) as u8;
        data[header_offset + 6] = content_start as u8;

        let mut page = Page {
            pgno,
            data,
            dirty: true,
        };
        pager.write_page(&mut page)?;
        pager.write_page_direct(pgno, &page.data)?;

        Ok((
            Self::new(pager, pgno, BtreeFlags {
                intkey: true,
                blobkey: false,
            }),
            pgno,
        ))
    }

    pub fn cursor(&self) -> BtreeCursor {
        BtreeCursor {
            btree: Btree {
                pager: self.pager.clone(),
                root_page: self.root_page,
                flags: self.flags,
            },
            pgno: self.root_page,
            cell_index: 0,
            end_of_table: false,
            current_key: None,
            current_record: None,
        }
    }

    pub fn insert(&self, key: i64, record: &[u8]) -> Result<()> {
        let mut page = self.pager.get_page(self.root_page)?;
        let header_offset = page_header_offset(self.root_page);
        let page_type = page.data[header_offset];
        if page_type != PAGE_TYPE_LEAF_TABLE {
            return Err(SqlliteError::sql(
                ResultCode::Internal,
                "only leaf table pages supported",
            ));
        }

        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut cell_ptrs = Vec::with_capacity(cell_count + 1);
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, header_offset + 8 + i * 2) as usize;
            cell_ptrs.push(ptr);
        }

        // Build new cell
        let mut cell = Vec::new();
        let mut buf = [0u8; 9];
        let n = write_varint(key as u64, &mut buf);
        cell.extend_from_slice(&buf[..n]);
        let n = write_varint(record.len() as u64, &mut buf);
        cell.extend_from_slice(&buf[..n]);
        cell.extend_from_slice(record);

        // Find insertion position
        let mut insert_idx = cell_count;
        for (i, &ptr) in cell_ptrs.iter().enumerate() {
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if key < rowid {
                insert_idx = i;
                break;
            }
            if key == rowid {
                return Err(SqlliteError::sql(
                    ResultCode::Constraint,
                    "UNIQUE constraint failed",
                ));
            }
        }

        // Insert cell at end of cell content area (simplified: append)
        let content_start = read_u16_be(&page.data, header_offset + 5) as usize;
        let new_content_start = content_start - cell.len();
        if new_content_start < header_offset + 8 + (cell_count + 1) * 2 + 1 {
            return Err(SqlliteError::sql(ResultCode::Full, "database or disk is full"));
        }

        page.data[new_content_start..new_content_start + cell.len()].copy_from_slice(&cell);

        // Shift cell pointers and insert new one
        let new_ptr = new_content_start as u16;
        for i in (insert_idx..cell_count).rev() {
            let old_ptr = read_u16_be(&page.data, header_offset + 8 + i * 2);
            write_u16_be(&mut page.data, header_offset + 10 + i * 2, old_ptr);
        }
        write_u16_be(&mut page.data, header_offset + 8 + insert_idx * 2, new_ptr);
        write_u16_be(&mut page.data, header_offset + 5, new_content_start as u16);
        write_u16_be(&mut page.data, header_offset + 3, (cell_count + 1) as u16);

        self.pager.write_page(&mut page)?;
        Ok(())
    }

    pub fn delete(&self, key: i64) -> Result<bool> {
        let mut page = self.pager.get_page(self.root_page)?;
        let header_offset = page_header_offset(self.root_page);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;

        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, header_offset + 8 + i * 2) as usize;
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if rowid == key {
                // Remove cell by shifting pointers
                for j in i..cell_count - 1 {
                    let next_ptr = read_u16_be(&page.data, header_offset + 8 + (j + 1) * 2);
                    write_u16_be(&mut page.data, header_offset + 8 + j * 2, next_ptr);
                }
                write_u16_be(&mut page.data, header_offset + 3, (cell_count - 1) as u16);
                self.pager.write_page(&mut page)?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl BtreeCursor {
    pub fn first(&mut self) -> Result<bool> {
        self.cell_index = 0;
        self.end_of_table = false;
        self.read_current()
    }

    pub fn next(&mut self) -> Result<bool> {
        if self.end_of_table {
            return Ok(false);
        }
        self.cell_index += 1;
        self.read_current()
    }

    pub fn seek(&mut self, key: i64) -> Result<bool> {
        let page = self.btree.pager.get_page(self.btree.root_page)?;
        let header_offset = page_header_offset(self.btree.root_page);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;

        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, header_offset + 8 + i * 2) as usize;
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if rowid >= key {
                self.cell_index = i;
                return self.read_current();
            }
        }
        self.end_of_table = true;
        self.current_key = None;
        self.current_record = None;
        Ok(false)
    }

    fn read_current(&mut self) -> Result<bool> {
        let page = self.btree.pager.get_page(self.btree.root_page)?;
        let header_offset = page_header_offset(self.btree.root_page);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;

        if self.cell_index >= cell_count {
            self.end_of_table = true;
            self.current_key = None;
            self.current_record = None;
            return Ok(false);
        }

        let ptr = read_u16_be(&page.data, header_offset + 8 + self.cell_index * 2) as usize;
        let (rowid, payload_offset) = read_cell_rowid(&page.data, ptr)?;
        let (payload_len, n) = read_varint(&page.data, payload_offset)?;
        let payload_start = payload_offset + n;
        let payload_end = payload_start + payload_len as usize;

        self.current_key = Some(rowid);
        self.current_record = Some(page.data[payload_start..payload_end].to_vec());
        Ok(true)
    }

    pub fn key(&self) -> Option<i64> {
        self.current_key
    }

    pub fn record(&self) -> Option<&[u8]> {
        self.current_record.as_deref()
    }

    pub fn values(&self) -> Result<Vec<Value>> {
        match &self.current_record {
            Some(r) => decode_record(r),
            None => Ok(vec![]),
        }
    }

    pub fn is_eof(&self) -> bool {
        self.end_of_table
    }
}

fn page_header_offset(pgno: PageNo) -> usize {
    if pgno == ROOT_PAGE {
        DATABASE_HEADER_SIZE
    } else {
        0
    }
}

fn read_cell_rowid(data: &[u8], offset: usize) -> Result<(i64, usize)> {
    let (rowid, n) = read_varint(data, offset)?;
    Ok((rowid as i64, offset + n))
}

/// Insert a row into a table B-tree with encoded record.
pub fn btree_insert_row(
    btree: &Btree,
    rowid: i64,
    values: &[Value],
) -> Result<()> {
    let record = encode_record(values);
    btree.insert(rowid, &record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::UnixVfs;
    use crate::storage::pager::PagerFlags;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    #[test]
    fn btree_insert_and_scan() {
        let tmp = NamedTempFile::new().unwrap();
        let vfs = UnixVfs;
        let pager = Arc::new(
            Pager::open(&vfs, Some(tmp.path()), PagerFlags::default()).unwrap(),
        );
        let (btree, _) = Btree::create_table(pager.clone()).unwrap();

        btree_insert_row(&btree, 1, &[Value::Integer(1), Value::Text("hello".into())]).unwrap();
        btree_insert_row(&btree, 2, &[Value::Integer(2), Value::Text("world".into())]).unwrap();
        pager.commit().unwrap();

        let mut cursor = btree.cursor();
        assert!(cursor.first().unwrap());
        assert_eq!(cursor.key(), Some(1));
        assert!(cursor.next().unwrap());
        assert_eq!(cursor.key(), Some(2));
        assert!(!cursor.next().unwrap());
    }
}
