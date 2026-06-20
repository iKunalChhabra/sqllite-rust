//! SQLite record format encoding and decoding.

use crate::error::{Result, ResultCode, SqlliteError};
use crate::types::Value;
use crate::varint::{read_varint, write_varint, varint_len};

/// Serial type constants from SQLite record format.
pub const SERIAL_NULL: u64 = 0;
pub const SERIAL_INT8: u64 = 1;
pub const SERIAL_INT16: u64 = 2;
pub const SERIAL_INT24: u64 = 3;
pub const SERIAL_INT32: u64 = 4;
pub const SERIAL_INT48: u64 = 5;
pub const SERIAL_INT64: u64 = 6;
pub const SERIAL_FLOAT64: u64 = 7;
pub const SERIAL_TEXT0: u64 = 8; // reserved
pub const SERIAL_TEXT_MIN: u64 = 12;
pub const SERIAL_BLOB_MIN: u64 = 13;

/// Encode a list of values into a SQLite record blob.
pub fn encode_record(values: &[Value]) -> Vec<u8> {
    let mut header_buf = Vec::new();
    let mut body = Vec::new();

    let mut buf = [0u8; 9];
    let n = write_varint(values.len() as u64, &mut buf);
    header_buf.extend_from_slice(&buf[..n]);

    for v in values {
        let st = serial_type(v);
        let n = write_varint(st, &mut buf);
        header_buf.extend_from_slice(&buf[..n]);
        encode_value(v, st, &mut body);
    }

    let header_len = header_buf.len();
    let mut record = Vec::with_capacity(header_len + body.len() + 9);
    let n = write_varint(header_len as u64, &mut buf);
    record.extend_from_slice(&buf[..n]);
    record.extend_from_slice(&header_buf);
    record.extend_from_slice(&body);
    record
}

fn serial_type(value: &Value) -> u64 {
    match value {
        Value::Null => SERIAL_NULL,
        Value::Integer(i) => {
            if *i >= i8::MIN as i64 && *i <= i8::MAX as i64 {
                SERIAL_INT8
            } else if *i >= i16::MIN as i64 && *i <= i16::MAX as i64 {
                SERIAL_INT16
            } else if *i >= -(1i64 << 23) && *i < (1i64 << 23) {
                SERIAL_INT24
            } else if *i >= i32::MIN as i64 && *i <= i32::MAX as i64 {
                SERIAL_INT32
            } else if *i >= -(1i64 << 47) && *i < (1i64 << 47) {
                SERIAL_INT48
            } else {
                SERIAL_INT64
            }
        }
        Value::Real(_) => SERIAL_FLOAT64,
        Value::Text(s) => ((s.len() * 2) as u64) + SERIAL_TEXT_MIN,
        Value::Blob(b) => ((b.len() * 2) as u64) + SERIAL_BLOB_MIN,
    }
}

fn encode_value(value: &Value, serial: u64, buf: &mut Vec<u8>) {
    match value {
        Value::Null => {}
        Value::Integer(i) => {
            let bytes = match serial {
                SERIAL_INT8 => i.to_le_bytes()[..1].to_vec(),
                SERIAL_INT16 => i.to_le_bytes()[..2].to_vec(),
                SERIAL_INT24 => i.to_le_bytes()[..3].to_vec(),
                SERIAL_INT32 => i.to_le_bytes()[..4].to_vec(),
                SERIAL_INT48 => i.to_le_bytes()[..6].to_vec(),
                _ => i.to_le_bytes().to_vec(),
            };
            buf.extend_from_slice(&bytes);
        }
        Value::Real(r) => buf.extend_from_slice(&r.to_le_bytes()),
        Value::Text(s) => buf.extend_from_slice(s.as_bytes()),
        Value::Blob(b) => buf.extend_from_slice(b),
    }
}

/// Decode a SQLite record blob into values.
pub fn decode_record(data: &[u8]) -> Result<Vec<Value>> {
    let (header_size, n) = read_varint(data, 0)?;
    let header_end = n + header_size as usize;
    if header_end > data.len() {
        return Err(SqlliteError::sql(ResultCode::Corrupt, "truncated record header"));
    }

    let (col_count, m) = read_varint(data, n)?;
    let mut serial_types = Vec::with_capacity(col_count as usize);
    let mut offset = n + m;
    for _ in 0..col_count {
        let (st, k) = read_varint(data, offset)?;
        serial_types.push(st);
        offset += k;
    }

    let body = &data[header_end..];
    let mut values = Vec::with_capacity(col_count as usize);
    let mut body_offset = 0;
    for st in serial_types {
        let (value, consumed) = decode_serial(st, body, body_offset)?;
        values.push(value);
        body_offset += consumed;
    }
    Ok(values)
}

fn decode_serial(serial: u64, data: &[u8], offset: usize) -> Result<(Value, usize)> {
    if serial == SERIAL_NULL {
        return Ok((Value::Null, 0));
    }
    if serial == SERIAL_INT8 {
        return read_int(data, offset, 1).map(|v| (Value::Integer(v), 1));
    }
    if serial == SERIAL_INT16 {
        return read_int(data, offset, 2).map(|v| (Value::Integer(v), 2));
    }
    if serial == SERIAL_INT24 {
        return read_int(data, offset, 3).map(|v| (Value::Integer(v), 3));
    }
    if serial == SERIAL_INT32 {
        return read_int(data, offset, 4).map(|v| (Value::Integer(v), 4));
    }
    if serial == SERIAL_INT48 {
        return read_int(data, offset, 6).map(|v| (Value::Integer(v), 6));
    }
    if serial == SERIAL_INT64 {
        return read_int(data, offset, 8).map(|v| (Value::Integer(v), 8));
    }
    if serial == SERIAL_FLOAT64 {
        if offset + 8 > data.len() {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "truncated float"));
        }
        let bytes: [u8; 8] = data[offset..offset + 8].try_into().unwrap();
        return Ok((Value::Real(f64::from_le_bytes(bytes)), 8));
    }
    if serial >= SERIAL_TEXT_MIN && serial % 2 == 0 {
        let len = ((serial - SERIAL_TEXT_MIN) / 2) as usize;
        if offset + len > data.len() {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "truncated text"));
        }
        let text = String::from_utf8_lossy(&data[offset..offset + len]).into_owned();
        return Ok((Value::Text(text), len));
    }
    if serial >= SERIAL_BLOB_MIN && serial % 2 == 1 {
        let len = ((serial - SERIAL_BLOB_MIN) / 2) as usize;
        if offset + len > data.len() {
            return Err(SqlliteError::sql(ResultCode::Corrupt, "truncated blob"));
        }
        return Ok((Value::Blob(data[offset..offset + len].to_vec()), len));
    }
    Err(SqlliteError::sql(
        ResultCode::Corrupt,
        format!("invalid serial type {serial}"),
    ))
}

fn read_int(data: &[u8], offset: usize, nbytes: usize) -> Result<i64> {
    if offset + nbytes > data.len() {
        return Err(SqlliteError::sql(ResultCode::Corrupt, "truncated integer"));
    }
    let mut buf = [0u8; 8];
    buf[..nbytes].copy_from_slice(&data[offset..offset + nbytes]);
    // Sign-extend
    if nbytes < 8 && (data[offset + nbytes - 1] & 0x80) != 0 {
        for b in &mut buf[nbytes..] {
            *b = 0xff;
        }
    }
    Ok(i64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip() {
        let values = vec![
            Value::Null,
            Value::Integer(42),
            Value::Real(3.14),
            Value::Text("hello".into()),
            Value::Blob(vec![1, 2, 3]),
        ];
        let encoded = encode_record(&values);
        let decoded = decode_record(&encoded).unwrap();
        assert_eq!(values, decoded);
    }
}
