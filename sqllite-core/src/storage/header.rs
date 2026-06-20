//! Database file header (first 100 bytes of page 1).

use crate::constants::*;
use crate::error::{Result, ResultCode, SqlliteError};
use crate::io::{read_u32_be, write_u32_be};
use crate::types::PageNo;

/// Parsed database file header.
#[derive(Debug, Clone)]
pub struct DatabaseHeader {
    pub page_size: u32,
    pub write_version: u8,
    pub read_version: u8,
    pub reserved_bytes: u8,
    pub max_embedded_payload: u8,
    pub min_embedded_payload: u8,
    pub leaf_payload: u8,
    pub change_counter: u32,
    pub database_size: u32,
    pub freelist_trunk: PageNo,
    pub freelist_count: u32,
    pub schema_cookie: u32,
    pub schema_format: u32,
    pub default_cache_size: u32,
    pub largest_root: PageNo,
    pub text_encoding: u32,
    pub user_version: u32,
    pub incr_vacuum: u32,
    pub application_id: u32,
    pub version_valid_for: u32,
    pub sqlite_version: u32,
}

impl Default for DatabaseHeader {
    fn default() -> Self {
        Self::new(DEFAULT_PAGE_SIZE)
    }
}

impl DatabaseHeader {
    pub fn new(page_size: u32) -> Self {
        Self {
            page_size,
            write_version: 1,
            read_version: 1,
            reserved_bytes: 0,
            max_embedded_payload: MAX_EMBEDDED_PAYLOAD,
            min_embedded_payload: MIN_EMBEDDED_PAYLOAD,
            leaf_payload: LEAF_PAYLOAD,
            change_counter: 1,
            database_size: 0,
            freelist_trunk: 0,
            freelist_count: 0,
            schema_cookie: 0,
            schema_format: SCHEMA_FORMAT,
            default_cache_size: 0,
            largest_root: 0,
            text_encoding: TEXT_ENCODING_UTF8,
            user_version: 0,
            incr_vacuum: 0,
            application_id: 0,
            version_valid_for: 1,
            sqlite_version: 3044001, // 3.44.1
        }
    }

    /// Parse header from the first 100 bytes of page 1.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < DATABASE_HEADER_SIZE {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "database header too short"));
        }
        if &data[..16] != DATABASE_HEADER_MAGIC {
            return Err(SqlliteError::sql(ResultCode::NotADb, "file is not a database"));
        }

        let raw_page_size = read_u16_be(data, HEADER_OFFSET_PAGE_SIZE) as u32;
        let page_size = if raw_page_size == 1 {
            65536
        } else {
            raw_page_size
        };

        if page_size < MIN_PAGE_SIZE || page_size > MAX_PAGE_SIZE || !page_size.is_power_of_two() {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "invalid page size"));
        }

        Ok(Self {
            page_size,
            write_version: data[HEADER_OFFSET_WRITE_VERSION],
            read_version: data[HEADER_OFFSET_READ_VERSION],
            reserved_bytes: data[HEADER_OFFSET_RESERVED],
            max_embedded_payload: data[HEADER_OFFSET_MAX_EMBED_PAYLOAD],
            min_embedded_payload: data[HEADER_OFFSET_MIN_EMBED_PAYLOAD],
            leaf_payload: data[HEADER_OFFSET_LEAF_PAYLOAD],
            change_counter: read_u32_be(data, HEADER_OFFSET_CHANGE_COUNTER),
            database_size: read_u32_be(data, HEADER_OFFSET_DATABASE_SIZE),
            freelist_trunk: read_u32_be(data, HEADER_OFFSET_FREELIST_TRUNK),
            freelist_count: read_u32_be(data, HEADER_OFFSET_FREELIST_COUNT),
            schema_cookie: read_u32_be(data, HEADER_OFFSET_SCHEMA_COOKIE),
            schema_format: read_u32_be(data, HEADER_OFFSET_SCHEMA_FORMAT),
            default_cache_size: read_u32_be(data, HEADER_OFFSET_DEFAULT_CACHE),
            largest_root: read_u32_be(data, HEADER_OFFSET_LARGEST_ROOT),
            text_encoding: read_u32_be(data, HEADER_OFFSET_TEXT_ENCODING),
            user_version: read_u32_be(data, HEADER_OFFSET_USER_VERSION),
            incr_vacuum: read_u32_be(data, HEADER_OFFSET_INCR_VACUUM),
            application_id: read_u32_be(data, HEADER_OFFSET_APPLICATION_ID),
            version_valid_for: read_u32_be(data, HEADER_OFFSET_VERSION_VALID),
            sqlite_version: read_u32_be(data, HEADER_OFFSET_SQLITE_VERSION),
        })
    }

    /// Serialize header into the first 100 bytes of a page buffer.
    pub fn write(&self, page: &mut [u8]) {
        page[..16].copy_from_slice(DATABASE_HEADER_MAGIC);
        let raw_page_size = if self.page_size == 65536 {
            1u16
        } else {
            self.page_size as u16
        };
        page[HEADER_OFFSET_PAGE_SIZE] = (raw_page_size >> 8) as u8;
        page[HEADER_OFFSET_PAGE_SIZE + 1] = raw_page_size as u8;
        page[HEADER_OFFSET_WRITE_VERSION] = self.write_version;
        page[HEADER_OFFSET_READ_VERSION] = self.read_version;
        page[HEADER_OFFSET_RESERVED] = self.reserved_bytes;
        page[HEADER_OFFSET_MAX_EMBED_PAYLOAD] = self.max_embedded_payload;
        page[HEADER_OFFSET_MIN_EMBED_PAYLOAD] = self.min_embedded_payload;
        page[HEADER_OFFSET_LEAF_PAYLOAD] = self.leaf_payload;
        write_u32_be(page, HEADER_OFFSET_CHANGE_COUNTER, self.change_counter);
        write_u32_be(page, HEADER_OFFSET_DATABASE_SIZE, self.database_size);
        write_u32_be(page, HEADER_OFFSET_FREELIST_TRUNK, self.freelist_trunk);
        write_u32_be(page, HEADER_OFFSET_FREELIST_COUNT, self.freelist_count);
        write_u32_be(page, HEADER_OFFSET_SCHEMA_COOKIE, self.schema_cookie);
        write_u32_be(page, HEADER_OFFSET_SCHEMA_FORMAT, self.schema_format);
        write_u32_be(page, HEADER_OFFSET_DEFAULT_CACHE, self.default_cache_size);
        write_u32_be(page, HEADER_OFFSET_LARGEST_ROOT, self.largest_root);
        write_u32_be(page, HEADER_OFFSET_TEXT_ENCODING, self.text_encoding);
        write_u32_be(page, HEADER_OFFSET_USER_VERSION, self.user_version);
        write_u32_be(page, HEADER_OFFSET_INCR_VACUUM, self.incr_vacuum);
        write_u32_be(page, HEADER_OFFSET_APPLICATION_ID, self.application_id);
        write_u32_be(page, HEADER_OFFSET_VERSION_VALID, self.version_valid_for);
        write_u32_be(page, HEADER_OFFSET_SQLITE_VERSION, self.sqlite_version);
    }

    pub fn usable_page_size(&self) -> u32 {
        self.page_size - self.reserved_bytes as u32
    }
}

fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}
