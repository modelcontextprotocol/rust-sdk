use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use super::{EventsError, JsonObject, Value, invalid};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key(Vec<u8>);

impl Key {
    pub(super) fn new(
        principal: &str,
        url: &str,
        name: &str,
        args: &JsonObject,
    ) -> Result<Self, EventsError> {
        let mut encoded = b"io.modelcontextprotocol/events webhook key v1\0".to_vec();
        for part in [principal, url, name] {
            string(&mut encoded, part);
        }
        object(&mut encoded, args)?;
        Ok(Self(encoded))
    }

    pub(super) fn id(&self) -> String {
        let hash = Sha256::digest(&self.0);
        format!("sub_{}", URL_SAFE_NO_PAD.encode(&hash[..16]))
    }
}

fn length(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u64).to_be_bytes());
}

fn string(out: &mut Vec<u8>, s: &str) {
    length(out, s.len());
    out.extend_from_slice(s.as_bytes());
}

fn object(out: &mut Vec<u8>, obj: &JsonObject) -> Result<(), EventsError> {
    let mut sorted: Vec<_> = obj.iter().collect();
    sorted.sort_unstable_by_key(|(key, _)| *key);
    length(out, sorted.len());
    for (key, value) in sorted {
        string(out, key);
        encode_value(out, value)?;
    }
    Ok(())
}

fn encode_value(out: &mut Vec<u8>, value: &Value) -> Result<(), EventsError> {
    match value {
        Value::Null => out.push(b'n'),
        Value::Bool(v) => out.push(if *v { b't' } else { b'f' }),
        Value::String(s) => {
            out.push(b's');
            string(out, s);
        }
        Value::Array(items) => {
            out.push(b'a');
            length(out, items.len());
            for item in items {
                encode_value(out, item)?;
            }
        }
        Value::Object(obj) => {
            out.push(b'o');
            object(out, obj)?;
        }
        Value::Number(n) => {
            if let Some(n) = n.as_u64() {
                out.push(b'u');
                out.extend_from_slice(&n.to_be_bytes());
            } else if let Some(n) = n.as_i64() {
                if n == 0 {
                    out.push(b'u');
                } else {
                    out.push(b'i');
                }
                out.extend_from_slice(&n.to_be_bytes());
            } else {
                let n = n
                    .as_f64()
                    .ok_or_else(|| invalid("unsupported webhook argument number"))?;
                // Outside binary64's safe integer range, never merge a
                // rounded floating-point token with an exact JSON integer.
                if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 {
                    if n < 0.0 {
                        out.push(b'i');
                        out.extend_from_slice(&(n as i64).to_be_bytes());
                    } else {
                        out.push(b'u');
                        out.extend_from_slice(&(n as u64).to_be_bytes());
                    }
                } else {
                    out.push(b'd');
                    out.extend_from_slice(&n.to_bits().to_be_bytes());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_arguments_and_stable_ids() {
        let key = |principal, text| {
            let arguments = serde_json::from_str(text).unwrap();
            Key::new(principal, "https://example.net/", "demo", &arguments).unwrap()
        };
        let expected = key(
            "alice",
            r#"{"a":[18446744073709551615, -9223372036854775808, 0], "b":0.0000001}"#,
        );
        assert_eq!(
            expected,
            key(
                "alice",
                r#"{"b":1e-7,"a":[18446744073709551615,-9223372036854775808,-0]}"#
            )
        );
        assert_eq!(
            expected,
            key(
                "alice",
                r#"{"b":1E-7,"a":[18446744073709551615,-9223372036854775808,-0.0]}"#
            )
        );
        assert_ne!(
            expected,
            key(
                "alice",
                r#"{"b":1e-7,"a":[18446744073709551614,-9223372036854775808,-0]}"#
            )
        );
        assert_ne!(
            expected,
            key(
                "alice",
                r#"{"b":1e-7,"a":[18446744073709551615,-9223372036854775807,-0]}"#
            )
        );
        assert_ne!(key("alice", "{}"), key("bob", "{}"));
        assert_eq!(key("alice", r#"{"a":2}"#), key("alice", r#"{"a":2.0}"#));
        assert_ne!(
            key("alice", r#"{"a":9007199254740992}"#),
            key("alice", r#"{"a":9007199254740992.0}"#)
        );
        assert_ne!(
            key("alice", r#"{"a":[1,2]}"#),
            key("alice", r#"{"a":[2,1]}"#)
        );
        assert_ne!(key("alice", r#"{"a":2}"#), key("alice", r#"{"a":"2"}"#));
        assert!(expected.id().starts_with("sub_"));
    }
}
