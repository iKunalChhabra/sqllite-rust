//! SQLite variable-length integer encoding.

use crate::error::{Result, ResultCode, SqlliteError};

/// Maximum bytes in a varint.
pub const MAX_VARINT_LEN: usize = 9;

/// Read a varint from `data` starting at `offset`. Returns (value, bytes_consumed).
pub fn read_varint(data: &[u8], offset: usize) -> Result<(u64, usize)> {
    if offset >= data.len() {
        return Err(SqlliteError::sql(
            ResultCode::Corrupt,
            "truncated varint",
        ));
    }

    let mut result: u64 = 0;
    for i in 0..MAX_VARINT_LEN {
        if offset + i >= data.len() {
            return Err(SqlliteError::sql(
                ResultCode::Corrupt,
                "truncated varint",
            ));
        }
        let byte = data[offset + i];
        if i == MAX_VARINT_LEN - 1 {
            result = (result << 8) | byte as u64;
            return Ok((result, MAX_VARINT_LEN));
        }
        result = (result << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
    }
    unreachable!()
}

/// Write a varint to `buf`. Returns number of bytes written.
pub fn write_varint(mut value: u64, buf: &mut [u8]) -> usize {
    if value == 0 {
        buf[0] = 0;
        return 1;
    }
    let mut tmp = [0u8; MAX_VARINT_LEN];
    let mut len = 0usize;
    while value > 0 {
        tmp[len] = (value & 0x7f) as u8;
        value >>= 7;
        len += 1;
    }
    for i in 0..len {
        buf[i] = tmp[len - 1 - i];
        if i < len - 1 {
            buf[i] |= 0x80;
        }
    }
    len
}

/// Compute the number of bytes needed to encode a varint.
pub fn varint_len(mut value: u64) -> usize {
    if value == 0 {
        return 1;
    }
    let mut len = 0;
    while value > 0 {
        value >>= 7;
        len += 1;
    }
    len
}

/// Read a signed varint (zigzag encoded).
pub fn read_signed_varint(data: &[u8], offset: usize) -> Result<(i64, usize)> {
    let (uv, n) = read_varint(data, offset)?;
    let v = if uv & 1 == 1 {
        -((uv >> 1) as i64) - 1
    } else {
        (uv >> 1) as i64
    };
    Ok((v, n))
}

/// Write a signed varint (zigzag encoded).
pub fn write_signed_varint(value: i64, buf: &mut [u8]) -> usize {
    let uv = if value < 0 {
        ((-value - 1) as u64) << 1 | 1
    } else {
        (value as u64) << 1
    };
    write_varint(uv, buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_varint() {
        for v in [0u64, 1, 127, 128, 300, 16384] {
            let mut buf = [0u8; MAX_VARINT_LEN];
            let n = write_varint(v, &mut buf);
            let (decoded, m) = read_varint(&buf, 0).unwrap();
            assert_eq!(v, decoded, "failed for value {v}");
            assert_eq!(n, m);
        }
    }

    #[test]
    fn signed_varint() {
        for v in [-1i64, 0, 1, 100, -100] {
            let mut buf = [0u8; MAX_VARINT_LEN];
            let n = write_signed_varint(v, &mut buf);
            let (decoded, m) = read_signed_varint(&buf, 0).unwrap();
            assert_eq!(v, decoded, "failed for value {v}");
            assert_eq!(n, m);
        }
    }
}
