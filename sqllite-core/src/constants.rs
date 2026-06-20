//! Fundamental constants for SQLite file format compatibility.

/// Magic string at the start of every SQLite database file.
pub const DATABASE_HEADER_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Size of the database file header in bytes.
pub const DATABASE_HEADER_SIZE: usize = 100;

/// Default page size.
pub const DEFAULT_PAGE_SIZE: u32 = 4096;

/// Minimum page size.
pub const MIN_PAGE_SIZE: u32 = 512;

/// Maximum page size.
pub const MAX_PAGE_SIZE: u32 = 65536;

/// Byte offset of page size in database header.
pub const HEADER_OFFSET_PAGE_SIZE: usize = 16;

/// Byte offset of file format write version.
pub const HEADER_OFFSET_WRITE_VERSION: usize = 18;

/// Byte offset of file format read version.
pub const HEADER_OFFSET_READ_VERSION: usize = 19;

/// Byte offset of reserved bytes per page.
pub const HEADER_OFFSET_RESERVED: usize = 20;

/// Byte offset of maximum embedded payload fraction.
pub const HEADER_OFFSET_MAX_EMBED_PAYLOAD: usize = 21;

/// Byte offset of minimum embedded payload fraction.
pub const HEADER_OFFSET_MIN_EMBED_PAYLOAD: usize = 22;

/// Byte offset of leaf payload fraction.
pub const HEADER_OFFSET_LEAF_PAYLOAD: usize = 23;

/// Byte offset of file change counter.
pub const HEADER_OFFSET_CHANGE_COUNTER: usize = 24;

/// Byte offset of database size in pages.
pub const HEADER_OFFSET_DATABASE_SIZE: usize = 28;

/// Byte offset of first freelist trunk page.
pub const HEADER_OFFSET_FREELIST_TRUNK: usize = 32;

/// Byte offset of total freelist page count.
pub const HEADER_OFFSET_FREELIST_COUNT: usize = 36;

/// Byte offset of schema cookie.
pub const HEADER_OFFSET_SCHEMA_COOKIE: usize = 40;

/// Byte offset of schema format number.
pub const HEADER_OFFSET_SCHEMA_FORMAT: usize = 44;

/// Byte offset of default page cache size.
pub const HEADER_OFFSET_DEFAULT_CACHE: usize = 48;

/// Byte offset of largest root b-tree page.
pub const HEADER_OFFSET_LARGEST_ROOT: usize = 52;

/// Byte offset of text encoding.
pub const HEADER_OFFSET_TEXT_ENCODING: usize = 56;

/// Byte offset of user version.
pub const HEADER_OFFSET_USER_VERSION: usize = 60;

/// Byte offset of incremental vacuum mode.
pub const HEADER_OFFSET_INCR_VACUUM: usize = 64;

/// Byte offset of application ID.
pub const HEADER_OFFSET_APPLICATION_ID: usize = 68;

/// Byte offset of version-valid-for number.
pub const HEADER_OFFSET_VERSION_VALID: usize = 92;

/// Byte offset of SQLite version number.
pub const HEADER_OFFSET_SQLITE_VERSION: usize = 96;

/// B-tree page type: interior index.
pub const PAGE_TYPE_INTERIOR_INDEX: u8 = 0x02;

/// B-tree page type: interior table.
pub const PAGE_TYPE_INTERIOR_TABLE: u8 = 0x05;

/// B-tree page type: leaf index.
pub const PAGE_TYPE_LEAF_INDEX: u8 = 0x0a;

/// B-tree page type: leaf table.
pub const PAGE_TYPE_LEAF_TABLE: u8 = 0x0d;

/// Page 1 is always the database header page.
pub const ROOT_PAGE: u32 = 1;

/// Schema format number (latest).
pub const SCHEMA_FORMAT: u32 = 4;

/// UTF-8 text encoding.
pub const TEXT_ENCODING_UTF8: u32 = 1;

/// UTF-16le text encoding.
pub const TEXT_ENCODING_UTF16LE: u32 = 2;

/// UTF-16be text encoding.
pub const TEXT_ENCODING_UTF16BE: u32 = 3;

/// Maximum number of columns in a table.
pub const MAX_COLUMN: usize = 2000;

/// Maximum length of a SQL statement.
pub const MAX_SQL_LENGTH: usize = 1_000_000_000;

/// Maximum length of a LIKE pattern.
pub const MAX_LIKE_PATTERN_LENGTH: usize = 50_000;

/// Maximum number of attached databases.
pub const MAX_ATTACHED: usize = 125;

/// Name of the schema table.
pub const SCHEMA_TABLE_NAME: &str = "sqlite_schema";

/// Legacy name of the schema table.
pub const SCHEMA_TABLE_NAME_LEGACY: &str = "sqlite_master";

/// Maximum embedded payload fraction (64/255).
pub const MAX_EMBEDDED_PAYLOAD: u8 = 64;

/// Minimum embedded payload fraction (32/255).
pub const MIN_EMBEDDED_PAYLOAD: u8 = 32;

/// Leaf payload fraction (32/255).
pub const LEAF_PAYLOAD: u8 = 32;

/// Journal header magic.
pub const JOURNAL_HEADER_MAGIC: &[u8; 8] = b"\xD9\xD5\x05\xF9\x20\xA1\x63\xD7";

/// WAL magic number (big-endian checksum).
pub const WAL_MAGIC: u32 = 0x377f0682;

/// WAL magic number (little-endian checksum).
pub const WAL_MAGIC_LE: u32 = 0x377f0683;

/// WAL header size.
pub const WAL_HEADER_SIZE: usize = 32;

/// WAL frame header size.
pub const WAL_FRAME_HEADER_SIZE: usize = 24;
