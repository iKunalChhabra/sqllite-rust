//! B-tree storage engine with interior pages and page splits.

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_u16_be, read_u32_be, write_u16_be, write_u32_be};
use crate::record::{decode_record, encode_record, record_byte_length};
use crate::storage::pager::{Page, Pager};
use crate::types::{PageNo, Value};
use crate::varint::{read_varint, write_varint};
use std::cell::Cell;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default)]
pub struct BtreeFlags {
    pub intkey: bool,
    pub blobkey: bool,
}

pub struct Btree {
    pager: Arc<Pager>,
    root_page: Cell<PageNo>,
    flags: BtreeFlags,
}

#[derive(Clone)]
struct CursorFrame {
    pgno: PageNo,
    child_slot: usize,
}

pub struct BtreeCursor {
    btree: Btree,
    stack: Vec<CursorFrame>,
    leaf_pgno: PageNo,
    cell_index: usize,
    end_of_table: bool,
    current_key: Option<i64>,
    current_record: Option<Vec<u8>>,
}

struct LeafSplit {
    left_pgno: PageNo,
    divider_key: i64,
    divider_blob: Option<Vec<u8>>,
    right_pgno: PageNo,
}

impl Btree {
    pub fn new(pager: Arc<Pager>, root_page: PageNo, flags: BtreeFlags) -> Self {
        Self {
            pager,
            root_page: Cell::new(root_page),
            flags,
        }
    }

    pub fn root_page(&self) -> PageNo {
        self.root_page.get()
    }

    pub fn create_table(pager: Arc<Pager>) -> Result<(Self, PageNo)> {
        Self::create_btree(
            pager,
            PAGE_TYPE_LEAF_TABLE,
            BtreeFlags {
                intkey: true,
                blobkey: false,
            },
        )
    }

    pub fn create_index(pager: Arc<Pager>) -> Result<(Self, PageNo)> {
        Self::create_btree(
            pager,
            PAGE_TYPE_LEAF_INDEX,
            BtreeFlags {
                intkey: false,
                blobkey: true,
            },
        )
    }

    fn create_btree(
        pager: Arc<Pager>,
        page_type: u8,
        flags: BtreeFlags,
    ) -> Result<(Self, PageNo)> {
        let pgno = pager.allocate_page()?;
        init_leaf_page(&pager, pgno, page_type)?;
        Ok((Self::new(pager, pgno, flags), pgno))
    }

    pub fn cursor(&self) -> BtreeCursor {
        BtreeCursor {
            btree: Btree {
                pager: self.pager.clone(),
                root_page: Cell::new(self.root_page.get()),
                flags: self.flags,
            },
            stack: Vec::new(),
            leaf_pgno: self.root_page.get(),
            cell_index: 0,
            end_of_table: false,
            current_key: None,
            current_record: None,
        }
    }

    pub fn insert(&self, key: i64, record: &[u8]) -> Result<()> {
        if self.flags.blobkey {
            return Err(SqlliteError::sql(
                ResultCode::Internal,
                "use insert_index for index btrees",
            ));
        }
        self.insert_table_row(key, record)
    }

    pub fn replace(&self, key: i64, record: &[u8]) -> Result<()> {
        if self.flags.blobkey {
            return Err(SqlliteError::sql(
                ResultCode::Internal,
                "replace not supported on index btrees",
            ));
        }
        if self.update_table_row(key, record)? {
            return Ok(());
        }
        let _ = self.delete(key)?;
        self.insert_table_row(key, record)
    }

    pub fn insert_index(&self, key: &[u8], rowid: i64) -> Result<()> {
        if !self.flags.blobkey {
            return Err(SqlliteError::sql(
                ResultCode::Internal,
                "insert_index requires index btree",
            ));
        }
        let mut payload = key.to_vec();
        let mut buf = [0u8; 9];
        let n = write_varint(rowid as u64, &mut buf);
        payload.extend_from_slice(&buf[..n]);
        self.insert_index_payload(&payload)
    }

    fn insert_table_row(&self, key: i64, record: &[u8]) -> Result<()> {
        let cell = build_table_cell(key, record);
        if let Some(split) = self.insert_into_tree(self.root_page.get(), &cell, key)? {
            if split.left_pgno == self.root_page.get() {
                self.promote_root_split(split)?;
            } else {
                self.insert_into_interior_table(self.root_page.get(), split)?;
            }
        }
        Ok(())
    }

    fn insert_index_payload(&self, payload: &[u8]) -> Result<()> {
        let cell = build_index_cell(payload);
        let divider = payload.to_vec();
        if let Some(split) =
            self.insert_index_into_tree(self.root_page.get(), &cell, &divider)?
        {
            if split.left_pgno == self.root_page.get() {
                self.promote_root_split(split)?;
            } else {
                self.insert_into_interior_index(self.root_page.get(), split)?;
            }
        }
        Ok(())
    }

    fn insert_into_tree(
        &self,
        pgno: PageNo,
        cell: &[u8],
        key: i64,
    ) -> Result<Option<LeafSplit>> {
        let page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        let page_type = page.data[header_offset];

        match page_type {
            PAGE_TYPE_LEAF_TABLE => self.insert_into_leaf_table(pgno, cell, key),
            PAGE_TYPE_INTERIOR_TABLE => {
                let child = self.find_child_page_table(&page, header_offset, key)?;
                if let Some(split) = self.insert_into_tree(child, cell, key)? {
                    self.insert_into_interior_table(pgno, split)
                } else {
                    Ok(None)
                }
            }
            _ => Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "unexpected page type in table btree",
            )),
        }
    }

    fn insert_index_into_tree(
        &self,
        pgno: PageNo,
        cell: &[u8],
        divider: &[u8],
    ) -> Result<Option<LeafSplit>> {
        let page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        let page_type = page.data[header_offset];

        match page_type {
            PAGE_TYPE_LEAF_INDEX => self.insert_into_leaf_index(pgno, cell, divider),
            PAGE_TYPE_INTERIOR_INDEX => {
                let child = self.find_child_page_index(&page, header_offset, divider)?;
                if let Some(split) = self.insert_index_into_tree(child, cell, divider)? {
                    self.insert_into_interior_index(pgno, split)
                } else {
                    Ok(None)
                }
            }
            _ => Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "unexpected page type in index btree",
            )),
        }
    }

    fn insert_into_leaf_table(
        &self,
        pgno: PageNo,
        cell: &[u8],
        key: i64,
    ) -> Result<Option<LeafSplit>> {
        let mut page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        if page.data[header_offset] != PAGE_TYPE_LEAF_TABLE {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "expected leaf table page",
            ));
        }

        if let Some(existing) = self.find_cell_in_page_table(&page, header_offset, key)? {
            if existing == key {
                return Err(SqlliteError::sql(
                    ResultCode::Constraint,
                    "UNIQUE constraint failed",
                ));
            }
        }

        if self.try_insert_leaf_cell(&mut page, header_offset, cell, key, false)? {
            self.pager.write_page(&mut page)?;
            return Ok(None);
        }

        let right_pgno = self.pager.allocate_page()?;
        init_leaf_page(&self.pager, right_pgno, PAGE_TYPE_LEAF_TABLE)?;
        let split = self.split_leaf_table(&mut page, header_offset, pgno, right_pgno, cell, key)?;
        self.pager.write_page(&mut page)?;
        Ok(Some(split))
    }

    fn insert_into_leaf_index(
        &self,
        pgno: PageNo,
        cell: &[u8],
        divider: &[u8],
    ) -> Result<Option<LeafSplit>> {
        let mut page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        if page.data[header_offset] != PAGE_TYPE_LEAF_INDEX {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "expected leaf index page",
            ));
        }

        if self.find_cell_in_page_index(&page, header_offset, divider)? {
            return Err(SqlliteError::sql(
                ResultCode::Constraint,
                "UNIQUE constraint failed",
            ));
        }

        if self.try_insert_leaf_cell_index(&mut page, header_offset, cell, divider)? {
            self.pager.write_page(&mut page)?;
            return Ok(None);
        }

        let right_pgno = self.pager.allocate_page()?;
        init_leaf_page(&self.pager, right_pgno, PAGE_TYPE_LEAF_INDEX)?;
        let split =
            self.split_leaf_index(&mut page, header_offset, pgno, right_pgno, cell, divider)?;
        self.pager.write_page(&mut page)?;
        Ok(Some(split))
    }

    fn promote_root_split(&self, split: LeafSplit) -> Result<()> {
        let old_root = self.root_page.get();
        let new_root = self.pager.allocate_page()?;
        let page_size = self.pager.page_size() as usize;
        let mut data = vec![0u8; page_size];
        let header_offset = page_header_offset(new_root);
        let page_type = if self.flags.blobkey {
            PAGE_TYPE_INTERIOR_INDEX
        } else {
            PAGE_TYPE_INTERIOR_TABLE
        };
        data[header_offset] = page_type;
        let content_start = page_size as u16;
        write_u16_be(&mut data, header_offset + 5, content_start);
        write_u32_be(&mut data, header_offset + 8, split.right_pgno);

        let cell = if self.flags.blobkey {
            let divider = split
                .divider_blob
                .as_ref()
                .ok_or_else(|| SqlliteError::sql(ResultCode::Corrupt, "missing index divider"))?;
            build_interior_index_cell(old_root, divider)
        } else {
            build_interior_table_cell(old_root, split.divider_key)
        };
        let cell_start = content_start as usize - cell.len();
        data[cell_start..cell_start + cell.len()].copy_from_slice(&cell);
        write_u16_be(&mut data, header_offset + 3, 1);
        write_u16_be(&mut data, header_offset + 12, cell_start as u16);

        let mut page = Page {
            pgno: new_root,
            data,
            dirty: true,
        };
        self.pager.write_page(&mut page)?;
        self.root_page.set(new_root);
        Ok(())
    }

    fn insert_into_interior_table(
        &self,
        pgno: PageNo,
        split: LeafSplit,
    ) -> Result<Option<LeafSplit>> {
        let mut page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        let rightmost = read_u32_be(&page.data, header_offset + 8);
        if rightmost == split.left_pgno {
            write_u32_be(&mut page.data, header_offset + 8, split.right_pgno);
        }
        let cell = build_interior_table_cell(split.left_pgno, split.divider_key);

        if self.try_insert_interior_cell_table(&mut page, header_offset, &cell, split.divider_key)? {
            self.pager.write_page(&mut page)?;
            return Ok(None);
        }

        // Interior page full: split interior and propagate upward.
        let right_pgno = self.pager.allocate_page()?;
        let page_type = page.data[header_offset];
        init_interior_page(&self.pager, right_pgno, page_type)?;
        let interior_split =
            self.split_interior_table(&mut page, header_offset, pgno, right_pgno, &cell, split.divider_key)?;
        self.pager.write_page(&mut page)?;
        if pgno == self.root_page.get() {
            self.promote_root_split(interior_split)?;
            Ok(None)
        } else {
            Ok(Some(interior_split))
        }
    }

    fn insert_into_interior_index(
        &self,
        pgno: PageNo,
        split: LeafSplit,
    ) -> Result<Option<LeafSplit>> {
        let divider = split
            .divider_blob
            .as_ref()
            .ok_or_else(|| SqlliteError::sql(ResultCode::Corrupt, "missing index divider"))?;
        let mut page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        let rightmost = read_u32_be(&page.data, header_offset + 8);
        if rightmost == split.left_pgno {
            write_u32_be(&mut page.data, header_offset + 8, split.right_pgno);
        }
        let cell = build_interior_index_cell(
            split.left_pgno,
            divider,
        );

        if self.try_insert_interior_cell_index(&mut page, header_offset, &cell, divider)? {
            self.pager.write_page(&mut page)?;
            return Ok(None);
        }

        let right_pgno = self.pager.allocate_page()?;
        init_interior_page(&self.pager, right_pgno, PAGE_TYPE_INTERIOR_INDEX)?;
        let interior_split = self.split_interior_index(
            &mut page,
            header_offset,
            pgno,
            right_pgno,
            &cell,
            divider,
        )?;
        self.pager.write_page(&mut page)?;
        if pgno == self.root_page.get() {
            self.promote_root_split(interior_split)?;
            Ok(None)
        } else {
            Ok(Some(interior_split))
        }
    }

    fn find_child_page_table(&self, page: &Page, header_offset: usize, key: i64) -> Result<PageNo> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let (child, cell_key) = read_interior_table_cell(&page.data, ptr)?;
            if key <= cell_key {
                return Ok(child);
            }
        }
        Ok(read_u32_be(&page.data, header_offset + 8))
    }

    fn find_child_page_index(
        &self,
        page: &Page,
        header_offset: usize,
        key: &[u8],
    ) -> Result<PageNo> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let (child, cell_key) = read_interior_index_cell(&page.data, ptr)?;
            if key <= cell_key.as_slice() {
                return Ok(child);
            }
        }
        Ok(read_u32_be(&page.data, header_offset + 8))
    }

    fn child_page_at_slot(
        &self,
        page: &Page,
        header_offset: usize,
        slot: usize,
    ) -> Result<PageNo> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        if slot < cell_count {
            let ptr =
                read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, slot)) as usize;
            if page.data[header_offset] == PAGE_TYPE_INTERIOR_TABLE {
                Ok(read_interior_table_cell(&page.data, ptr)?.0)
            } else {
                Ok(read_interior_index_cell(&page.data, ptr)?.0)
            }
        } else if slot == cell_count {
            Ok(read_u32_be(&page.data, header_offset + 8))
        } else {
            Err(SqlliteError::sql(ResultCode::Corrupt, "invalid child slot"))
        }
    }

    fn try_insert_leaf_cell(
        &self,
        page: &mut Page,
        header_offset: usize,
        cell: &[u8],
        key: i64,
        _index: bool,
    ) -> Result<bool> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut insert_idx = cell_count;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if key < rowid {
                insert_idx = i;
                break;
            }
        }
        let min_ptr = leaf_cell_ptr_offset(header_offset, cell_count + 1) + 1;
        let content_start = read_u16_be(&page.data, header_offset + 5) as usize;
        let new_content_start = content_start.saturating_sub(cell.len());
        if new_content_start < min_ptr {
            return Ok(false);
        }
        page.data[new_content_start..new_content_start + cell.len()].copy_from_slice(cell);
        let new_ptr = new_content_start as u16;
        for i in (insert_idx..cell_count).rev() {
            let old_ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i));
            write_u16_be(
                &mut page.data,
                leaf_cell_ptr_offset(header_offset, i + 1),
                old_ptr,
            );
        }
        write_u16_be(
            &mut page.data,
            leaf_cell_ptr_offset(header_offset, insert_idx),
            new_ptr,
        );
        write_u16_be(&mut page.data, header_offset + 5, new_content_start as u16);
        write_u16_be(&mut page.data, header_offset + 3, (cell_count + 1) as u16);
        Ok(true)
    }

    fn try_insert_leaf_cell_index(
        &self,
        page: &mut Page,
        header_offset: usize,
        cell: &[u8],
        divider: &[u8],
    ) -> Result<bool> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut insert_idx = cell_count;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let existing = read_index_payload(&page.data, ptr)?;
            if divider < existing.as_slice() {
                insert_idx = i;
                break;
            }
        }
        let min_ptr = leaf_cell_ptr_offset(header_offset, cell_count + 1) + 1;
        let content_start = read_u16_be(&page.data, header_offset + 5) as usize;
        let new_content_start = content_start.saturating_sub(cell.len());
        if new_content_start < min_ptr {
            return Ok(false);
        }
        page.data[new_content_start..new_content_start + cell.len()].copy_from_slice(cell);
        let new_ptr = new_content_start as u16;
        for i in (insert_idx..cell_count).rev() {
            let old_ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i));
            write_u16_be(
                &mut page.data,
                leaf_cell_ptr_offset(header_offset, i + 1),
                old_ptr,
            );
        }
        write_u16_be(
            &mut page.data,
            leaf_cell_ptr_offset(header_offset, insert_idx),
            new_ptr,
        );
        write_u16_be(&mut page.data, header_offset + 5, new_content_start as u16);
        write_u16_be(&mut page.data, header_offset + 3, (cell_count + 1) as u16);
        Ok(true)
    }

    fn try_insert_interior_cell_table(
        &self,
        page: &mut Page,
        header_offset: usize,
        cell: &[u8],
        key: i64,
    ) -> Result<bool> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut insert_idx = cell_count;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let (_, cell_key) = read_interior_table_cell(&page.data, ptr)?;
            if key <= cell_key {
                insert_idx = i;
                break;
            }
        }
        let min_ptr = interior_cell_ptr_offset(header_offset, cell_count + 1) + 1;
        let content_start = read_u16_be(&page.data, header_offset + 5) as usize;
        let new_content_start = content_start.saturating_sub(cell.len());
        if new_content_start < min_ptr {
            return Ok(false);
        }
        page.data[new_content_start..new_content_start + cell.len()].copy_from_slice(cell);
        let new_ptr = new_content_start as u16;
        for i in (insert_idx..cell_count).rev() {
            let old_ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i));
            write_u16_be(
                &mut page.data,
                interior_cell_ptr_offset(header_offset, i + 1),
                old_ptr,
            );
        }
        write_u16_be(
            &mut page.data,
            interior_cell_ptr_offset(header_offset, insert_idx),
            new_ptr,
        );
        write_u16_be(&mut page.data, header_offset + 5, new_content_start as u16);
        write_u16_be(&mut page.data, header_offset + 3, (cell_count + 1) as u16);
        Ok(true)
    }

    fn try_insert_interior_cell_index(
        &self,
        page: &mut Page,
        header_offset: usize,
        cell: &[u8],
        divider: &[u8],
    ) -> Result<bool> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut insert_idx = cell_count;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let (_, cell_key) = read_interior_index_cell(&page.data, ptr)?;
            if divider <= cell_key.as_slice() {
                insert_idx = i;
                break;
            }
        }
        let min_ptr = interior_cell_ptr_offset(header_offset, cell_count + 1) + 1;
        let content_start = read_u16_be(&page.data, header_offset + 5) as usize;
        let new_content_start = content_start.saturating_sub(cell.len());
        if new_content_start < min_ptr {
            return Ok(false);
        }
        page.data[new_content_start..new_content_start + cell.len()].copy_from_slice(cell);
        let new_ptr = new_content_start as u16;
        for i in (insert_idx..cell_count).rev() {
            let old_ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i));
            write_u16_be(
                &mut page.data,
                interior_cell_ptr_offset(header_offset, i + 1),
                old_ptr,
            );
        }
        write_u16_be(
            &mut page.data,
            interior_cell_ptr_offset(header_offset, insert_idx),
            new_ptr,
        );
        write_u16_be(&mut page.data, header_offset + 5, new_content_start as u16);
        write_u16_be(&mut page.data, header_offset + 3, (cell_count + 1) as u16);
        Ok(true)
    }

    fn split_leaf_table(
        &self,
        page: &mut Page,
        header_offset: usize,
        left_pgno: PageNo,
        right_pgno: PageNo,
        new_cell: &[u8],
        new_key: i64,
    ) -> Result<LeafSplit> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut cells = Vec::with_capacity(cell_count + 1);
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let (rowid, payload_off) = read_cell_rowid(&page.data, ptr)?;
            let (payload_len, n) = read_varint(&page.data, payload_off)?;
            let start = payload_off + n;
            let end = start + payload_len as usize;
            let mut cell = Vec::new();
            let mut buf = [0u8; 9];
            let vn = write_varint(rowid as u64, &mut buf);
            cell.extend_from_slice(&buf[..vn]);
            let vn = write_varint(payload_len, &mut buf);
            cell.extend_from_slice(&buf[..vn]);
            cell.extend_from_slice(&page.data[start..end]);
            cells.push((rowid, cell));
        }
        cells.push((new_key, new_cell.to_vec()));
        cells.sort_by_key(|(k, _)| *k);

        let mid = cells.len() / 2;
        assert!(mid > 0, "cannot split page with fewer than 2 cells");
        let divider_key = cells[mid - 1].0;

        self.rebuild_leaf_table_page(page, header_offset, &cells[..mid])?;
        let mut right_page = self.pager.get_page(right_pgno)?;
        self.rebuild_leaf_table_page(
            &mut right_page,
            page_header_offset(right_pgno),
            &cells[mid..],
        )?;
        self.pager.write_page(&mut right_page)?;

        Ok(LeafSplit {
            left_pgno,
            divider_key,
            divider_blob: None,
            right_pgno,
        })
    }

    fn split_leaf_index(
        &self,
        page: &mut Page,
        header_offset: usize,
        left_pgno: PageNo,
        right_pgno: PageNo,
        new_cell: &[u8],
        new_divider: &[u8],
    ) -> Result<LeafSplit> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let mut cells: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(cell_count + 1);
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let payload = read_index_payload(&page.data, ptr)?;
            let mut cell = Vec::new();
            let mut buf = [0u8; 9];
            let n = write_varint(payload.len() as u64, &mut buf);
            cell.extend_from_slice(&buf[..n]);
            cell.extend_from_slice(&payload);
            cells.push((payload, cell));
        }
        cells.push((new_divider.to_vec(), new_cell.to_vec()));
        cells.sort_by(|a, b| a.0.cmp(&b.0));

        let mid = cells.len() / 2;
        let divider_blob = cells[mid - 1].0.clone();

        let left = cells[..mid].iter().map(|(_, c)| c.clone()).collect::<Vec<_>>();
        let right = cells[mid..].iter().map(|(_, c)| c.clone()).collect::<Vec<_>>();

        self.rebuild_leaf_index_page(page, header_offset, &left)?;
        let mut right_page = self.pager.get_page(right_pgno)?;
        self.rebuild_leaf_index_page(
            &mut right_page,
            page_header_offset(right_pgno),
            &right,
        )?;
        self.pager.write_page(&mut right_page)?;

        Ok(LeafSplit {
            left_pgno,
            divider_key: 0,
            divider_blob: Some(divider_blob),
            right_pgno,
        })
    }

    fn split_interior_table(
        &self,
        page: &mut Page,
        header_offset: usize,
        left_pgno: PageNo,
        right_pgno: PageNo,
        new_cell: &[u8],
        new_key: i64,
    ) -> Result<LeafSplit> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let rightmost = read_u32_be(&page.data, header_offset + 8);
        let mut cells = Vec::with_capacity(cell_count + 1);
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let start = ptr;
            let (_, n) = read_varint(&page.data, ptr + 4)?;
            let end = ptr + 4 + n;
            cells.push(page.data[start..end].to_vec());
        }
        cells.push(new_cell.to_vec());
        cells.sort_by(|a, b| {
            let ka = read_interior_table_cell(a, 0).map(|(_, k)| k).unwrap_or(0);
            let kb = read_interior_table_cell(b, 0).map(|(_, k)| k).unwrap_or(0);
            ka.cmp(&kb)
        });

        let mid = cells.len() / 2;
        let divider_key = read_interior_table_cell(&cells[mid - 1], 0)?.1;

        self.rebuild_interior_table_page(page, header_offset, &cells[..mid], rightmost)?;
        let mut right_page = self.pager.get_page(right_pgno)?;
        let new_rightmost = read_u32_be(&page.data, header_offset + 8);
        self.rebuild_interior_table_page(
            &mut right_page,
            page_header_offset(right_pgno),
            &cells[mid..],
            new_rightmost,
        )?;
        self.pager.write_page(&mut right_page)?;

        Ok(LeafSplit {
            left_pgno,
            divider_key,
            divider_blob: None,
            right_pgno,
        })
    }

    fn split_interior_index(
        &self,
        page: &mut Page,
        header_offset: usize,
        left_pgno: PageNo,
        right_pgno: PageNo,
        new_cell: &[u8],
        new_divider: &[u8],
    ) -> Result<LeafSplit> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        let rightmost = read_u32_be(&page.data, header_offset + 8);
        let mut cells = Vec::with_capacity(cell_count + 1);
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, interior_cell_ptr_offset(header_offset, i)) as usize;
            let (_, n) = read_varint(&page.data, ptr + 4)?;
            let len = read_varint(&page.data, ptr + 4)?.0 as usize;
            let end = ptr + 4 + n + len;
            cells.push(page.data[ptr..end].to_vec());
        }
        cells.push(new_cell.to_vec());
        cells.sort_by(|a, b| {
            let ka = read_interior_index_cell(a, 0)
                .map(|(_, k)| k)
                .unwrap_or_default();
            let kb = read_interior_index_cell(b, 0)
                .map(|(_, k)| k)
                .unwrap_or_default();
            ka.cmp(&kb)
        });

        let mid = cells.len() / 2;
        let divider_blob = read_interior_index_cell(&cells[mid - 1], 0)?.1;

        self.rebuild_interior_index_page(page, header_offset, &cells[..mid], rightmost)?;
        let mut right_page = self.pager.get_page(right_pgno)?;
        let new_rightmost = read_u32_be(&page.data, header_offset + 8);
        self.rebuild_interior_index_page(
            &mut right_page,
            page_header_offset(right_pgno),
            &cells[mid..],
            new_rightmost,
        )?;
        self.pager.write_page(&mut right_page)?;

        Ok(LeafSplit {
            left_pgno,
            divider_key: 0,
            divider_blob: Some(divider_blob),
            right_pgno,
        })
    }

    fn rebuild_leaf_table_page(
        &self,
        page: &mut Page,
        header_offset: usize,
        cells: &[(i64, Vec<u8>)],
    ) -> Result<()> {
        let page_size = page.data.len();
        page.data[header_offset + 1] = 0;
        page.data[header_offset + 2] = 0;
        write_u16_be(&mut page.data, header_offset + 3, cells.len() as u16);
        page.data[header_offset + 7] = 0;
        let mut content_start = page_size;
        let mut ptrs = Vec::with_capacity(cells.len());
        for (_, cell) in cells.iter().rev() {
            content_start -= cell.len();
            page.data[content_start..content_start + cell.len()].copy_from_slice(cell);
            ptrs.push(content_start as u16);
        }
        ptrs.reverse();
        write_u16_be(&mut page.data, header_offset + 5, content_start as u16);
        for (i, ptr) in ptrs.iter().enumerate() {
            write_u16_be(&mut page.data, leaf_cell_ptr_offset(header_offset, i), *ptr);
        }
        Ok(())
    }

    fn rebuild_leaf_index_page(
        &self,
        page: &mut Page,
        header_offset: usize,
        cells: &[Vec<u8>],
    ) -> Result<()> {
        let page_size = page.data.len();
        page.data[header_offset + 1] = 0;
        page.data[header_offset + 2] = 0;
        write_u16_be(&mut page.data, header_offset + 3, cells.len() as u16);
        page.data[header_offset + 7] = 0;
        let mut content_start = page_size;
        let mut ptrs = Vec::with_capacity(cells.len());
        for cell in cells.iter().rev() {
            content_start -= cell.len();
            page.data[content_start..content_start + cell.len()].copy_from_slice(cell);
            ptrs.push(content_start as u16);
        }
        ptrs.reverse();
        write_u16_be(&mut page.data, header_offset + 5, content_start as u16);
        for (i, ptr) in ptrs.iter().enumerate() {
            write_u16_be(&mut page.data, leaf_cell_ptr_offset(header_offset, i), *ptr);
        }
        Ok(())
    }

    fn rebuild_interior_table_page(
        &self,
        page: &mut Page,
        header_offset: usize,
        cells: &[Vec<u8>],
        rightmost: PageNo,
    ) -> Result<()> {
        let page_size = page.data.len();
        page.data[header_offset + 1] = 0;
        page.data[header_offset + 2] = 0;
        write_u16_be(&mut page.data, header_offset + 3, cells.len() as u16);
        page.data[header_offset + 7] = 0;
        write_u32_be(&mut page.data, header_offset + 8, rightmost);
        let mut content_start = page_size;
        let mut ptrs = Vec::with_capacity(cells.len());
        for cell in cells.iter().rev() {
            content_start -= cell.len();
            page.data[content_start..content_start + cell.len()].copy_from_slice(cell);
            ptrs.push(content_start as u16);
        }
        ptrs.reverse();
        write_u16_be(&mut page.data, header_offset + 5, content_start as u16);
        for (i, ptr) in ptrs.iter().enumerate() {
            write_u16_be(
                &mut page.data,
                interior_cell_ptr_offset(header_offset, i),
                *ptr,
            );
        }
        Ok(())
    }

    fn rebuild_interior_index_page(
        &self,
        page: &mut Page,
        header_offset: usize,
        cells: &[Vec<u8>],
        rightmost: PageNo,
    ) -> Result<()> {
        self.rebuild_interior_table_page(page, header_offset, cells, rightmost)
    }

    fn find_cell_in_page_table(
        &self,
        page: &Page,
        header_offset: usize,
        key: i64,
    ) -> Result<Option<i64>> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if rowid == key {
                return Ok(Some(rowid));
            }
        }
        Ok(None)
    }

    fn find_cell_in_page_index(
        &self,
        page: &Page,
        header_offset: usize,
        divider: &[u8],
    ) -> Result<bool> {
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let existing = read_index_payload(&page.data, ptr)?;
            if existing.as_slice() == divider {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn update_table_row(&self, key: i64, record: &[u8]) -> Result<bool> {
        let leaf = self.find_leaf_for_key(self.root_page.get(), key)?;
        let mut page = self.pager.get_page(leaf)?;
        let header_offset = page_header_offset(leaf);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let (rowid, payload_offset) = read_cell_rowid(&page.data, ptr)?;
            if rowid != key {
                continue;
            }
            let (old_len, n) = read_varint(&page.data, payload_offset)?;
            let old_start = payload_offset + n;
            let old_end = old_start + old_len as usize;
            if record.len() != old_len as usize {
                return Ok(false);
            }
            page.data[old_start..old_end].copy_from_slice(record);
            self.pager.write_page(&mut page)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn find_leaf_for_key(&self, pgno: PageNo, key: i64) -> Result<PageNo> {
        let page = self.pager.get_page(pgno)?;
        let header_offset = page_header_offset(pgno);
        match page.data[header_offset] {
            PAGE_TYPE_LEAF_TABLE => Ok(pgno),
            PAGE_TYPE_INTERIOR_TABLE => {
                let child = self.find_child_page_table(&page, header_offset, key)?;
                self.find_leaf_for_key(child, key)
            }
            _ => Err(SqlliteError::sql(ResultCode::Corrupt, "invalid page in find_leaf")),
        }
    }

    pub fn delete(&self, key: i64) -> Result<bool> {
        if self.flags.blobkey {
            return Err(SqlliteError::sql(
                ResultCode::Internal,
                "delete by rowid not supported on index btrees",
            ));
        }
        let leaf = self.find_leaf_for_key(self.root_page.get(), key)?;
        let mut page = self.pager.get_page(leaf)?;
        let header_offset = page_header_offset(leaf);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
            if rowid == key {
                for j in i..cell_count - 1 {
                    let next_ptr =
                        read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, j + 1));
                    write_u16_be(
                        &mut page.data,
                        leaf_cell_ptr_offset(header_offset, j),
                        next_ptr,
                    );
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
        self.stack.clear();
        self.leaf_pgno = self.descend_leftmost(self.btree.root_page.get())?;
        self.cell_index = 0;
        self.end_of_table = false;
        self.read_current()
    }

    pub fn next(&mut self) -> Result<bool> {
        if self.end_of_table {
            return Ok(false);
        }
        self.cell_index += 1;
        if self.read_current()? {
            return Ok(true);
        }
        if let Some(leaf) = self.advance_to_next_leaf()? {
            self.leaf_pgno = leaf;
            self.cell_index = 0;
            return self.read_current();
        }
        self.end_of_table = true;
        self.current_key = None;
        self.current_record = None;
        Ok(false)
    }

    pub fn seek(&mut self, key: i64) -> Result<bool> {
        self.stack.clear();
        self.leaf_pgno = self.descend_for_key(self.btree.root_page.get(), key)?;
        let page = self.btree.pager.get_page(self.leaf_pgno)?;
        let header_offset = page_header_offset(self.leaf_pgno);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        self.cell_index = 0;
        self.end_of_table = false;
        for i in 0..cell_count {
            let ptr = read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, i)) as usize;
            if self.btree.flags.blobkey {
                let payload = read_index_payload(&page.data, ptr)?;
                let (_, rowid) = split_index_payload(&payload)?;
                if rowid >= key {
                    self.cell_index = i;
                    return self.read_current();
                }
            } else {
                let (rowid, _) = read_cell_rowid(&page.data, ptr)?;
                if rowid >= key {
                    self.cell_index = i;
                    return self.read_current();
                }
            }
        }
        self.end_of_table = true;
        self.current_key = None;
        self.current_record = None;
        Ok(false)
    }

    fn descend_leftmost(&mut self, mut pgno: PageNo) -> Result<PageNo> {
        loop {
            let page = self.btree.pager.get_page(pgno)?;
            let header_offset = page_header_offset(pgno);
            match page.data[header_offset] {
                PAGE_TYPE_LEAF_TABLE | PAGE_TYPE_LEAF_INDEX => return Ok(pgno),
                PAGE_TYPE_INTERIOR_TABLE | PAGE_TYPE_INTERIOR_INDEX => {
                    let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
                    let child = if cell_count == 0 {
                        read_u32_be(&page.data, header_offset + 8)
                    } else {
                        let ptr = read_u16_be(
                            &page.data,
                            interior_cell_ptr_offset(header_offset, 0),
                        ) as usize;
                        if page.data[header_offset] == PAGE_TYPE_INTERIOR_TABLE {
                            read_interior_table_cell(&page.data, ptr)?.0
                        } else {
                            read_interior_index_cell(&page.data, ptr)?.0
                        }
                    };
                    self.stack.push(CursorFrame {
                        pgno,
                        child_slot: 0,
                    });
                    pgno = child;
                }
                _ => {
                    return Err(SqlliteError::sql(ResultCode::Corrupt, "bad page type"));
                }
            }
        }
    }

    fn descend_for_key(&mut self, mut pgno: PageNo, key: i64) -> Result<PageNo> {
        loop {
            let page = self.btree.pager.get_page(pgno)?;
            let header_offset = page_header_offset(pgno);
            match page.data[header_offset] {
                PAGE_TYPE_LEAF_TABLE | PAGE_TYPE_LEAF_INDEX => return Ok(pgno),
                PAGE_TYPE_INTERIOR_TABLE => {
                    let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
                    let mut child_slot = cell_count;
                    let mut child = read_u32_be(&page.data, header_offset + 8);
                    for i in 0..cell_count {
                        let ptr = read_u16_be(
                            &page.data,
                            interior_cell_ptr_offset(header_offset, i),
                        ) as usize;
                        let (c, cell_key) = read_interior_table_cell(&page.data, ptr)?;
                        if key <= cell_key {
                            child_slot = i;
                            child = c;
                            break;
                        }
                    }
                    self.stack.push(CursorFrame {
                        pgno,
                        child_slot,
                    });
                    pgno = child;
                }
                PAGE_TYPE_INTERIOR_INDEX => {
                    let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
                    let probe = encode_record(&[Value::Integer(key)]);
                    let mut child_slot = cell_count;
                    let mut child = read_u32_be(&page.data, header_offset + 8);
                    for i in 0..cell_count {
                        let ptr = read_u16_be(
                            &page.data,
                            interior_cell_ptr_offset(header_offset, i),
                        ) as usize;
                        let (c, cell_key) = read_interior_index_cell(&page.data, ptr)?;
                        if probe.as_slice() <= cell_key.as_slice() {
                            child_slot = i;
                            child = c;
                            break;
                        }
                    }
                    self.stack.push(CursorFrame {
                        pgno,
                        child_slot,
                    });
                    pgno = child;
                }
                _ => {
                    return Err(SqlliteError::sql(ResultCode::Corrupt, "bad page type"));
                }
            }
        }
    }

    fn advance_to_next_leaf(&mut self) -> Result<Option<PageNo>> {
        while let Some(frame) = self.stack.pop() {
            let page = self.btree.pager.get_page(frame.pgno)?;
            let header_offset = page_header_offset(frame.pgno);
            let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
            let next_slot = frame.child_slot + 1;
            if next_slot <= cell_count {
                self.stack.push(CursorFrame {
                    pgno: frame.pgno,
                    child_slot: next_slot,
                });
                let child = Btree {
                    pager: self.btree.pager.clone(),
                    root_page: Cell::new(self.btree.root_page.get()),
                    flags: self.btree.flags,
                }
                .child_page_at_slot(&page, header_offset, next_slot)?;
                return Ok(Some(self.descend_leftmost_from(child)?));
            }
        }
        Ok(None)
    }

    fn descend_leftmost_from(&mut self, mut pgno: PageNo) -> Result<PageNo> {
        loop {
            let page = self.btree.pager.get_page(pgno)?;
            let header_offset = page_header_offset(pgno);
            match page.data[header_offset] {
                PAGE_TYPE_LEAF_TABLE | PAGE_TYPE_LEAF_INDEX => return Ok(pgno),
                PAGE_TYPE_INTERIOR_TABLE | PAGE_TYPE_INTERIOR_INDEX => {
                    let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
                    let child = if cell_count == 0 {
                        read_u32_be(&page.data, header_offset + 8)
                    } else {
                        let ptr = read_u16_be(
                            &page.data,
                            interior_cell_ptr_offset(header_offset, 0),
                        ) as usize;
                        if page.data[header_offset] == PAGE_TYPE_INTERIOR_TABLE {
                            read_interior_table_cell(&page.data, ptr)?.0
                        } else {
                            read_interior_index_cell(&page.data, ptr)?.0
                        }
                    };
                    self.stack.push(CursorFrame {
                        pgno,
                        child_slot: 0,
                    });
                    pgno = child;
                }
                _ => {
                    return Err(SqlliteError::sql(ResultCode::Corrupt, "bad page type"));
                }
            }
        }
    }

    fn read_current(&mut self) -> Result<bool> {
        let page = self.btree.pager.get_page(self.leaf_pgno)?;
        let header_offset = page_header_offset(self.leaf_pgno);
        let cell_count = read_u16_be(&page.data, header_offset + 3) as usize;
        if self.cell_index >= cell_count {
            return Ok(false);
        }
        let ptr =
            read_u16_be(&page.data, leaf_cell_ptr_offset(header_offset, self.cell_index)) as usize;
        if self.btree.flags.blobkey {
            let payload = read_index_payload(&page.data, ptr)?;
            let (key, rowid) = split_index_payload(&payload)?;
            self.current_key = Some(rowid);
            self.current_record = Some(key);
        } else {
            let (rowid, payload_offset) = read_cell_rowid(&page.data, ptr)?;
            let (payload_len, n) = read_varint(&page.data, payload_offset)?;
            let payload_start = payload_offset + n;
            let payload_end = payload_start + payload_len as usize;
            self.current_key = Some(rowid);
            self.current_record = Some(page.data[payload_start..payload_end].to_vec());
        }
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

fn leaf_cell_ptr_offset(header_offset: usize, index: usize) -> usize {
    header_offset + 8 + index * 2
}

fn interior_cell_ptr_offset(header_offset: usize, index: usize) -> usize {
    header_offset + 12 + index * 2
}

fn init_leaf_page(pager: &Pager, pgno: PageNo, page_type: u8) -> Result<()> {
    let page_size = pager.page_size() as usize;
    let mut data = vec![0u8; page_size];
    let header_offset = page_header_offset(pgno);
    data[header_offset] = page_type;
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
    Ok(())
}

fn init_interior_page(pager: &Pager, pgno: PageNo, page_type: u8) -> Result<()> {
    let page_size = pager.page_size() as usize;
    let mut data = vec![0u8; page_size];
    let header_offset = page_header_offset(pgno);
    data[header_offset] = page_type;
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
    Ok(())
}

fn build_table_cell(key: i64, record: &[u8]) -> Vec<u8> {
    let mut cell = Vec::new();
    let mut buf = [0u8; 9];
    let n = write_varint(key as u64, &mut buf);
    cell.extend_from_slice(&buf[..n]);
    let n = write_varint(record.len() as u64, &mut buf);
    cell.extend_from_slice(&buf[..n]);
    cell.extend_from_slice(record);
    cell
}

fn build_index_cell(payload: &[u8]) -> Vec<u8> {
    let mut cell = Vec::new();
    let mut buf = [0u8; 9];
    let n = write_varint(payload.len() as u64, &mut buf);
    cell.extend_from_slice(&buf[..n]);
    cell.extend_from_slice(payload);
    cell
}

fn build_interior_table_cell(child: PageNo, key: i64) -> Vec<u8> {
    let mut cell = Vec::new();
    cell.extend_from_slice(&child.to_be_bytes());
    let mut buf = [0u8; 9];
    let n = write_varint(key as u64, &mut buf);
    cell.extend_from_slice(&buf[..n]);
    cell
}

fn build_interior_index_cell(child: PageNo, key: &[u8]) -> Vec<u8> {
    let mut cell = Vec::new();
    cell.extend_from_slice(&child.to_be_bytes());
    let mut buf = [0u8; 9];
    let n = write_varint(key.len() as u64, &mut buf);
    cell.extend_from_slice(&buf[..n]);
    cell.extend_from_slice(key);
    cell
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

fn read_interior_table_cell(data: &[u8], offset: usize) -> Result<(PageNo, i64)> {
    let child = read_u32_be(data, offset);
    let (key, _) = read_varint(data, offset + 4)?;
    Ok((child, key as i64))
}

fn read_interior_index_cell(data: &[u8], offset: usize) -> Result<(PageNo, Vec<u8>)> {
    let child = read_u32_be(data, offset);
    let (len, n) = read_varint(data, offset + 4)?;
    let start = offset + 4 + n;
    let end = start + len as usize;
    Ok((child, data[start..end].to_vec()))
}

fn read_index_payload(data: &[u8], offset: usize) -> Result<Vec<u8>> {
    let (payload_len, n) = read_varint(data, offset)?;
    let start = offset + n;
    let end = start + payload_len as usize;
    if end > data.len() {
        return Err(SqlliteError::sql(
            ResultCode::Corrupt,
            "truncated index cell",
        ));
    }
    Ok(data[start..end].to_vec())
}

fn split_index_payload(payload: &[u8]) -> Result<(Vec<u8>, i64)> {
    for key_len in (1..payload.len()).rev() {
        if record_byte_length(&payload[..key_len]).is_ok() {
            let (rowid, n) = read_varint(&payload, key_len)?;
            if key_len + n == payload.len() {
                return Ok((payload[..key_len].to_vec(), rowid as i64));
            }
        }
    }
    Err(SqlliteError::sql(
        ResultCode::Corrupt,
        "invalid index payload",
    ))
}

pub fn btree_insert_row(btree: &Btree, rowid: i64, values: &[Value]) -> Result<()> {
    btree.insert(rowid, &encode_record(values))
}

pub fn btree_insert_index(btree: &Btree, key_values: &[Value], rowid: i64) -> Result<()> {
    btree.insert_index(&encode_record(key_values), rowid)
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
        let pager = Arc::new(Pager::open(&vfs, Some(tmp.path()), PagerFlags::default()).unwrap());
        let (btree, _) = Btree::create_table(pager.clone()).unwrap();
        btree_insert_row(
            &btree,
            1,
            &[Value::Integer(1), Value::Text("hello".into())],
        )
        .unwrap();
        btree_insert_row(
            &btree,
            2,
            &[Value::Integer(2), Value::Text("world".into())],
        )
        .unwrap();
        pager.commit().unwrap();
        let mut cursor = btree.cursor();
        assert!(cursor.first().unwrap());
        assert_eq!(cursor.key(), Some(1));
        assert!(cursor.next().unwrap());
        assert_eq!(cursor.key(), Some(2));
        assert!(!cursor.next().unwrap());
    }

    #[test]
    fn btree_large_insert_with_splits() {
        let tmp = NamedTempFile::new().unwrap();
        let vfs = UnixVfs;
        let pager = Arc::new(Pager::open(&vfs, Some(tmp.path()), PagerFlags::default()).unwrap());
        let (btree, _) = Btree::create_table(pager.clone()).unwrap();
        for i in 1..=500i64 {
            btree_insert_row(&btree, i, &[Value::Integer(i)]).unwrap();
        }
        pager.commit().unwrap();
        let mut cursor = btree.cursor();
        assert!(cursor.first().unwrap());
        let mut count = 0i64;
        loop {
            assert_eq!(cursor.key(), Some(count + 1));
            count += 1;
            if !cursor.next().unwrap() {
                break;
            }
        }
        assert_eq!(count, 500);
    }

    #[test]
    fn btree_persist_and_reload() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path();
        let vfs = UnixVfs;
        let saved_root;
        {
            let pager = Arc::new(Pager::open(&vfs, Some(path), PagerFlags::default()).unwrap());
            let (btree, root) = Btree::create_table(pager.clone()).unwrap();
            assert_eq!(root, 2);
            pager.begin().unwrap();
            for i in 1..=200i64 {
                btree_insert_row(&btree, i, &[Value::Text(format!("row{i}"))]).unwrap();
            }
            saved_root = btree.root_page();
            pager.commit().unwrap();
        }
        {
            let pager = Arc::new(Pager::open(&vfs, Some(path), PagerFlags::default()).unwrap());
            let btree = Btree::new(
                pager.clone(),
                saved_root,
                BtreeFlags {
                    intkey: true,
                    blobkey: false,
                },
            );
            let mut cursor = btree.cursor();
            assert!(cursor.first().unwrap());
            let mut count = 0;
            while cursor.next().unwrap() {
                count += 1;
            }
            assert_eq!(count + 1, 200);
        }
    }

    #[test]
    fn index_insert_and_scan() {
        let tmp = NamedTempFile::new().unwrap();
        let vfs = UnixVfs;
        let pager = Arc::new(Pager::open(&vfs, Some(tmp.path()), PagerFlags::default()).unwrap());
        let (btree, _) = Btree::create_index(pager.clone()).unwrap();
        btree_insert_index(&btree, &[Value::Integer(10)], 1).unwrap();
        btree_insert_index(&btree, &[Value::Integer(20)], 2).unwrap();
        pager.commit().unwrap();
        let mut cursor = btree.cursor();
        assert!(cursor.first().unwrap());
        assert_eq!(cursor.key(), Some(1));
        assert_eq!(cursor.values().unwrap(), vec![Value::Integer(10)]);
        assert!(cursor.next().unwrap());
        assert_eq!(cursor.key(), Some(2));
        assert!(!cursor.next().unwrap());
    }
}
