//! SQLite value types and column affinity.

use std::cmp::Ordering;

/// Column affinity types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Affinity {
    #[default]
    Text,
    Numeric,
    Integer,
    Real,
    Blob,
}

impl Affinity {
    pub fn from_char(c: char) -> Self {
        match c {
            't' | 'T' => Affinity::Text,
            'i' | 'I' => Affinity::Integer,
            'r' | 'R' => Affinity::Real,
            'b' | 'B' => Affinity::Blob,
            _ => Affinity::Numeric,
        }
    }

    pub fn as_char(self) -> char {
        match self {
            Affinity::Text => 't',
            Affinity::Numeric => 'n',
            Affinity::Integer => 'i',
            Affinity::Real => 'r',
            Affinity::Blob => 'b',
        }
    }
}

/// A SQL value stored in a register or column.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Default for Value {
    fn default() -> Self {
        Value::Null
    }
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            Value::Real(r) => Some(*r as i64),
            Value::Text(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn as_real(&self) -> Option<f64> {
        match self {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            Value::Text(s) => s.parse().ok(),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            Value::Integer(i) => None,
            Value::Real(r) => None,
            Value::Null => None,
            Value::Blob(_) => None,
        }
    }

    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Integer(i) => i.to_string(),
            Value::Real(r) => {
                if r.fract() == 0.0 && r.abs() < 9_007_199_254_740_992.0 {
                    format!("{:.0}", r)
                } else {
                    r.to_string()
                }
            }
            Value::Text(s) => s.clone(),
            Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }

    pub fn affinity(&self) -> Affinity {
        match self {
            Value::Null => Affinity::Text,
            Value::Integer(_) => Affinity::Integer,
            Value::Real(_) => Affinity::Real,
            Value::Text(_) => Affinity::Text,
            Value::Blob(_) => Affinity::Blob,
        }
    }

    pub fn compare(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Integer(a), Value::Real(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (Value::Real(a), Value::Integer(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::Real(a), Value::Real(b)) => {
                a.partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            (a, b) => a.to_text().cmp(&b.to_text()),
        }
    }

    pub fn apply_affinity(&self, affinity: Affinity) -> Value {
        if matches!(self, Value::Null) {
            return Value::Null;
        }
        match affinity {
            Affinity::Text => Value::Text(self.to_text()),
            Affinity::Integer => {
                if let Some(i) = self.as_integer() {
                    Value::Integer(i)
                } else {
                    self.clone()
                }
            }
            Affinity::Real => {
                if let Some(r) = self.as_real() {
                    Value::Real(r)
                } else {
                    self.clone()
                }
            }
            Affinity::Numeric => {
                if let Some(i) = self.as_integer() {
                    Value::Integer(i)
                } else if let Some(r) = self.as_real() {
                    Value::Real(r)
                } else {
                    Value::Text(self.to_text())
                }
            }
            Affinity::Blob => match self {
                Value::Blob(b) => Value::Blob(b.clone()),
                _ => Value::Blob(self.to_text().into_bytes()),
            },
        }
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Integer(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Integer(v as i64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Real(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Blob(v)
    }
}

/// Page number type.
pub type PageNo = u32;

/// Invalid page number.
pub const INVALID_PAGE: PageNo = 0;
