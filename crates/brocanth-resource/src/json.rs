//! Just enough JSON to write an envelope, and no dependency.
//!
//! This crate writes the page envelope and the error body. The rows inside
//! them belong to the application, which encodes them in
//! [`Resource::encode_row`](crate::Resource::encode_row). That split is why
//! there is no `serde` here: the one shape this crate knows is four numbers
//! and a list, and linking a serialiser to write it would put `serde` in the
//! tree of every application that mounts a resource.
//!
//! An application that does want `serde` uses it in its own `encode_row`,
//! where it is already using its own row type. If enough of them do, a
//! `serde` feature on this crate is the right place for it — and it would be a
//! feature, so PLAN.md §8.2 holds: with it off, the code is absent.

/// Append a JSON string literal, with the escapes the format requires.
///
/// Control characters go out as `\u00XX`. `<`, `>` and `&` are left alone:
/// this is a JSON body with a JSON content type, not a fragment of HTML.
pub fn push_string(out: &mut Vec<u8>, text: &str) {
    out.push(b'"');
    for ch in text.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Append `"name":`.
pub fn push_key(out: &mut Vec<u8>, name: &str) {
    push_string(out, name);
    out.push(b':');
}

/// Append a number.
pub fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(value.to_string().as_bytes());
}

/// The body of an error reply: `{"status":404,"message":"not found"}`.
pub fn error_body(status: u16, message: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + message.len());
    out.extend_from_slice(b"{\"status\":");
    push_u64(&mut out, u64::from(status));
    out.extend_from_slice(b",\"message\":");
    push_string(&mut out, message);
    out.push(b'}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_is_escaped_the_way_json_asks() {
        let mut out = Vec::new();
        push_string(&mut out, "a\"b\\c\nd\u{1}e");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"a\\\"b\\\\c\\nd\\u0001e\""
        );
    }

    #[test]
    fn an_error_body_carries_its_own_status() {
        assert_eq!(
            String::from_utf8(error_body(404, "not found")).unwrap(),
            "{\"status\":404,\"message\":\"not found\"}"
        );
    }
}
