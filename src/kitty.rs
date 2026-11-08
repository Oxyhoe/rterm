//! Kitty graphics protocol: parsing of `G <keys> ; <payload>` messages into
//! concrete operations. Execution (decoding, storing) lives in the app.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct KittyMsg {
    /// Control keys, e.g. `("a", "t")`, `("i?", "")` for queries.
    pub keys: Vec<(String, String)>,
    pub payload: Option<String>,
}

impl KittyMsg {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_query(&self, k: &str) -> bool {
        self.keys.iter().any(|(key, _)| key == &format!("{k}?"))
    }

    pub fn get_u64(&self, k: &str) -> Option<u64> {
        self.get(k)?.parse().ok()
    }

    pub fn get_u32(&self, k: &str) -> Option<u32> {
        self.get(k)?.parse().ok()
    }

    /// Quietness level: q=1 suppresses errors, q=2 suppresses everything.
    pub fn quiet(&self) -> u8 {
        self.get_u32("q").unwrap_or(0).min(2) as u8
    }
}

/// Parse the text that follows `ESC _ G`.
pub fn parse(payload: &str) -> KittyMsg {
    let (controls, data) = match payload.find(';') {
        Some(i) => (&payload[..i], Some(payload[i + 1..].to_string())),
        None => (payload, None),
    };
    let mut keys = Vec::new();
    for part in controls.split(',') {
        if part.is_empty() {
            continue;
        }
        if let Some(i) = part.find('=') {
            keys.push((part[..i].to_string(), part[i + 1..].to_string()));
        } else {
            keys.push((part.to_string(), String::new()));
        }
    }
    KittyMsg {
        keys,
        payload: data,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rgb,
    Rgba,
    Png,
}

impl Format {
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            24 => Some(Self::Rgb),
            32 => Some(Self::Rgba),
            100 => Some(Self::Png),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transmit {
    pub key: String,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// z=0 none, z=32/z=33 zlib.
    pub compression: u8,
    /// m=1: more chunks follow.
    pub more: bool,
    pub payload: Vec<u8>,
    pub quiet: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeleteScope {
    Image,
    CursorCell,
    All,
    // at/after not supported by rterm
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Transmit(Transmit),
    /// Print a previously transmitted image (a=p / a=T).
    Print {
        key: String,
        col: Option<u32>,
        row: Option<u32>,
        quiet: u8,
    },
    Delete {
        scope: DeleteScope,
        key: Option<String>,
        quiet: u8,
    },
    Query {
        quiet: u8,
    },
    /// Unimplemented op — reply with error unless quiet.
    Unsupported {
        action: String,
        quiet: u8,
    },
}

/// Decode a parsed message into an operation.
pub fn interpret(msg: &KittyMsg) -> Op {
    let quiet = msg.quiet();
    if msg.has_query("i") || msg.has_query("p") {
        return Op::Query { quiet };
    }
    let action = msg.get("a").unwrap_or("");
    match action {
        "t" | "T" => {
            let format = Format::from_code(msg.get_u32("f").unwrap_or(32)).unwrap_or(Format::Rgba);
            Op::Transmit(Transmit {
                key: msg.get("i").unwrap_or("0").to_string(),
                format,
                width: msg.get_u32("s").unwrap_or(0),
                height: msg.get_u32("v").unwrap_or(0),
                compression: msg.get_u32("z").unwrap_or(0) as u8,
                more: msg.get_u32("m").unwrap_or(0) == 1,
                payload: msg.payload.clone().unwrap_or_default().into_bytes(),
                quiet,
            })
        }
        "p" => Op::Print {
            key: msg.get("i").unwrap_or("0").to_string(),
            col: msg.get_u32("c").map(|c| c.saturating_sub(1)),
            row: msg.get_u32("r").map(|r| r.saturating_sub(1)),
            quiet,
        },
        "d" => {
            let scope = match msg.get("d").unwrap_or("a") {
                "i" => DeleteScope::Image,
                "c" => DeleteScope::CursorCell,
                _ => DeleteScope::All,
            };
            Op::Delete {
                scope,
                key: msg.get("i").map(str::to_string),
                quiet,
            }
        }
        "" | "q" => Op::Unsupported {
            action: "q".into(),
            quiet,
        },
        other => Op::Unsupported {
            action: other.into(),
            quiet,
        },
    }
}

/// Accumulates chunked transmits (m=1 ... m=0) keyed by image id.
///
/// base64 is decoded incrementally as chunks arrive; the resulting
/// [`Transmit::payload`] holds the **raw** image bytes once complete.
#[derive(Default)]
pub struct TransmitBuffer {
    pending: HashMap<String, Pending>,
    /// Encoded bytes seen (for the memory cap).
    bytes: usize,
}

struct Pending {
    head: Transmit,
    /// Partial base64 group carried between chunks.
    carry: String,
}

const MAX_PENDING: usize = 128 * 1024 * 1024;

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let st = base64::engine::general_purpose::STANDARD;
    let np = base64::engine::general_purpose::STANDARD_NO_PAD;
    st.decode(s)
        .or_else(|_| np.decode(s))
        .map_err(|e| e.to_string())
}

impl TransmitBuffer {
    pub fn push(&mut self, mut t: Transmit) -> Result<Option<Transmit>, String> {
        let key = t.key.clone();
        let text = String::from_utf8_lossy(&t.payload).into_owned();
        t.payload = Vec::new();
        let more = t.more;

        self.bytes += text.len();
        if self.bytes > MAX_PENDING {
            self.bytes = 0;
            self.pending.clear();
            return Err("transmit buffer overflow".into());
        }

        {
            let entry = self
                .pending
                .entry(key.clone())
                .and_modify(|e| e.carry.push_str(&text))
                .or_insert_with(|| Pending {
                    head: t,
                    carry: text.clone(),
                });

            // Decode complete 4-char groups; a group with '=' ends the stream.
            while entry.carry.len() >= 4 {
                let group: String = entry.carry.drain(..4).collect();
                let end_of_stream = group.contains('=');
                let bytes = b64_decode(&group)?;
                entry.head.payload.extend_from_slice(&bytes);
                if end_of_stream {
                    entry.carry.clear();
                    break;
                }
            }
        }

        if more {
            return Ok(None);
        }

        // Final chunk: flush the tail (1..3 unpadded chars, or padded remainder).
        let mut done = self.pending.remove(&key).ok_or("missing transmit")?;
        self.bytes = self.bytes.saturating_sub(done.carry.len());
        let tail = std::mem::take(&mut done.carry);
        if !tail.is_empty() {
            if tail.len() == 1 {
                return Err("invalid base64 tail".into());
            }
            let bytes = b64_decode(&tail)?;
            done.head.payload.extend_from_slice(&bytes);
        }
        done.head.more = false;
        Ok(Some(done.head))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_and_payload() {
        let msg = parse("i=1,f=100,a=t,q=1;AAAA");
        assert_eq!(msg.get("i"), Some("1"));
        assert_eq!(msg.get("f"), Some("100"));
        assert_eq!(msg.get("a"), Some("t"));
        assert_eq!(msg.payload.as_deref(), Some("AAAA"));
        assert_eq!(msg.quiet(), 1);
    }

    #[test]
    fn interprets_transmit() {
        let op = interpret(&parse("a=t,i=7,f=32,s=4,v=2,z=32;QUJD"));
        match op {
            Op::Transmit(t) => {
                assert_eq!(t.key, "7");
                assert_eq!(t.format, Format::Rgba);
                assert_eq!(t.width, 4);
                assert_eq!(t.height, 2);
                assert_eq!(t.compression, 32);
                assert!(!t.more);
                assert_eq!(t.payload, b"QUJD");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn interprets_print_and_delete() {
        assert_eq!(
            interpret(&parse("a=p,i=3,c=2,r=5;")),
            Op::Print {
                key: "3".into(),
                col: Some(1),
                row: Some(4),
                quiet: 0
            }
        );
        assert_eq!(
            interpret(&parse("a=d,d=i,i=3;")),
            Op::Delete {
                scope: DeleteScope::Image,
                key: Some("3".into()),
                quiet: 0
            }
        );
        assert_eq!(
            interpret(&parse("a=d;")),
            Op::Delete {
                scope: DeleteScope::All,
                key: None,
                quiet: 0
            }
        );
    }

    #[test]
    fn query_detection() {
        assert_eq!(interpret(&parse("i?")), Op::Query { quiet: 0 });
        assert!(matches!(interpret(&parse("a=z")), Op::Unsupported { .. }));
    }

    #[test]
    fn chunked_transmit_reassembles() {
        use base64::Engine;
        let e = base64::engine::general_purpose::STANDARD;
        let part1 = b"ABCDEF";
        let part2 = b"GHIJ";
        let mut buf = TransmitBuffer::default();

        let msg1 = parse(&format!("a=t,i=1,m=1,f=32,s=6,v=1;{}", e.encode(part1)));
        let Op::Transmit(t1) = interpret(&msg1) else {
            panic!()
        };
        assert!(buf.push(t1).unwrap().is_none());

        let msg2 = parse(&format!("a=t,i=1,f=32,s=6,v=1;{}", e.encode(part2)));
        let Op::Transmit(t2) = interpret(&msg2) else {
            panic!()
        };
        let done = buf.push(t2).unwrap().expect("final chunk completes");
        let mut expect = part1.to_vec();
        expect.extend_from_slice(part2);
        assert_eq!(done.payload, expect);
        assert!(!done.more);
    }

    #[test]
    fn unaligned_base64_chunks_stitch() {
        use base64::Engine;
        let np = base64::engine::general_purpose::STANDARD_NO_PAD;
        let raw = b"unaligned chunk bytes payload";
        let enc = np.encode(raw);
        let mut buf = TransmitBuffer::default();
        // Split the encoded stream at a non-multiple-of-4 boundary.
        let (a, b) = enc.split_at(7);
        let Op::Transmit(t1) = interpret(&parse(&format!("a=t,i=9,m=1,f=32;{a}"))) else {
            panic!()
        };
        assert!(buf.push(t1).unwrap().is_none());
        let Op::Transmit(t2) = interpret(&parse(&format!("a=t,i=9,f=32;{b}"))) else {
            panic!()
        };
        let done = buf.push(t2).unwrap().unwrap();
        assert_eq!(done.payload, raw);
    }
}
