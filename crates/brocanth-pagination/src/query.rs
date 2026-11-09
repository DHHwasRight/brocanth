//! The query string: read one value, read every pair, write a value back.
//!
//! No dependency does this. A list's query string is a few known keys, and a
//! percent-decoder for them is forty lines, which is cheaper than a crate in
//! the tree of something every list links.

/// Every name/value pair of a query string, decoded, in order.
///
/// `+` is a space, as a form submitted with GET writes it. A leading `?` is
/// skipped. An empty pair is dropped.
pub fn pairs(query: &str) -> Vec<(String, String)> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(raw))
        })
        .collect()
}

/// The first value of `name`, decoded. `None` when the key is absent; an
/// empty string when it is present with no value.
pub fn param(query: &str, name: &str) -> Option<String> {
    query.trim_start_matches('?').split('&').find_map(|pair| {
        let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(key) == name).then(|| decode(raw))
    })
}

/// Decodes `%XX` escapes and `+`, and leaves anything malformed as written.
///
/// Byte-level throughout. Slicing the `&str` instead would panic on `%` in
/// front of a multi-byte character, because the two bytes after the `%` can
/// fall inside one.
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high * 16 + low);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encodes everything outside the unreserved set of RFC 3986.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Writes pairs back into a query string, each part encoded.
pub fn join<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    pairs
        .into_iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips() {
        for value in ["plain", "a b", "é/ü?&=", "100%", "%", "%4", "+"] {
            assert_eq!(decode(&encode(value)), value, "{value}");
        }
    }

    #[test]
    fn a_malformed_escape_stays_as_written() {
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%4"), "%4");
        assert_eq!(decode("%é"), "%é");
    }

    #[test]
    fn a_form_space_is_a_plus() {
        assert_eq!(
            param("?q=red+shoes&page=2", "q").as_deref(),
            Some("red shoes")
        );
        assert_eq!(param("flag", "flag").as_deref(), Some(""));
        assert_eq!(param("a=1", "b"), None);
    }
}
