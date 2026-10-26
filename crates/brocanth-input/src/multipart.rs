//! `multipart/form-data`, read into the text fields a schema checks and the
//! files a store takes.
//!
//! # Why this is a third carrier and not a second form
//!
//! A urlencoded body is text and nothing else, so one function turns it into
//! a [`zod::Input`] and the handler is done. A multipart body carries two
//! kinds of thing at once: fields, which are text and belong to the schema,
//! and files, which are bytes and belong to a store. One return value cannot
//! be both, so [`Multipart`] holds them apart: [`Multipart::input`] gives the
//! fields under the same rules a urlencoded form gets, and
//! [`Multipart::files`] gives the rest.
//!
//! ```
//! use brocanth_input::Entrance;
//!
//! let body = b"--x\r\n\
//!     Content-Disposition: form-data; name=\"title\"\r\n\r\n\
//!     A cat\r\n\
//!     --x\r\n\
//!     Content-Disposition: form-data; name=\"photo\"; filename=\"cat.png\"\r\n\
//!     Content-Type: image/png\r\n\r\n\
//!     \x89PNG\r\n\
//!     --x--\r\n";
//!
//! let at = Entrance::new(Some("multipart/form-data; boundary=x"), "", body);
//! let sent = at.multipart().unwrap();
//!
//! assert_eq!(sent.field("title"), Some("A cat"));
//! let photo = sent.file("photo").unwrap();
//! assert_eq!(photo.filename(), Some("cat.png"));
//! assert_eq!(photo.bytes(), b"\x89PNG");
//! ```
//!
//! A file goes on to `brocanth-storage` as
//! `ObjectInfo::new().with_filename(..).with_declared_type(..)`, which is why
//! [`FilePart`] keeps the name and the type the client claimed and changes
//! neither. The store sniffs the bytes and ignores the claim; this crate does
//! not sniff, because then two crates would decide the same thing and one of
//! them would be wrong.
//!
//! # Limits
//!
//! A multipart parser with no caps is a denial-of-service hole: the body is
//! held in memory, each part is an allocation, and a header block with no end
//! is a scan to the last byte. Four caps answer that, all in [`Limits`], all
//! checked before the memory they guard is taken. A body over one of them is
//! a [`Fault`] whose [`status`](Fault::status) is 413.
//!
//! # Not done
//!
//! * `Content-Transfer-Encoding` is read and ignored. RFC 7578 §4.7 tells a
//!   client not to send one, and no browser does.
//! * `filename*` (RFC 5987) is not read. No browser sends it for a form part;
//!   they percent-encode the few bytes that would need it into `filename`.

use zod::{Input, Value};

/// How much of a multipart body this crate will read.
///
/// Every field is a ceiling, not a target. See [`Limits::DEFAULT`] for the
/// numbers and the reason behind each one.
///
/// A text field has no cap of its own: `total_bytes` already bounds it, and
/// a second number for the same thing is a second number to get wrong.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// The whole body, before a byte of it is copied.
    pub total_bytes: usize,
    /// One file part's content.
    pub file_bytes: usize,
    /// How many parts the body may hold.
    pub parts: usize,
    /// One part's header block, including the blank line that ends it.
    pub header_bytes: usize,
}

impl Limits {
    /// What a caller gets when it does not say.
    ///
    /// * `total_bytes`, 16 MiB. The body is in memory in one piece, so this
    ///   is what one request may cost the process. The server buffers 2 MiB
    ///   today, which makes this the second line and not the first.
    /// * `file_bytes`, 8 MiB. A photo from a phone or a scanned page fits. A
    ///   video does not, and a video wants a streaming reader this crate does
    ///   not have, so the refusal is honest.
    /// * `parts`, 64. A long form with a group of checkboxes and a few files
    ///   fits. Each part costs an allocation, so this is the cap that stops a
    ///   million empty parts from costing a million allocations.
    /// * `header_bytes`, 8 KiB. A part's headers are a disposition and a
    ///   type, under 300 bytes in practice. 8 KiB is what the usual front
    ///   proxies allow a whole request header block, so nothing a proxy
    ///   passes is refused here.
    pub const DEFAULT: Limits = Limits {
        total_bytes: 16 * 1024 * 1024,
        file_bytes: 8 * 1024 * 1024,
        parts: 64,
        header_bytes: 8 * 1024,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Why a multipart body could not be read.
///
/// Two groups, and the group decides the status. A body this parser could not
/// follow is 400: it arrived broken and no header will fix it. A body over a
/// cap is 413: it arrived whole and it is too big.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fault {
    /// The content type says `multipart/form-data` and names no boundary.
    NoBoundary,
    /// The boundary is empty, over 70 characters, or holds a byte RFC 2046
    /// does not allow in one.
    BadBoundary,
    /// The body does not open with the boundary.
    NoFirstBoundary,
    /// The body ended before the closing boundary.
    Truncated,
    /// A part's headers never reach the blank line that ends them.
    NoHeaderEnd,
    /// A header line with no colon in it.
    BadHeader(String),
    /// A part with no `Content-Disposition: form-data; name=...`.
    Unnamed,
    /// A text field whose bytes are not UTF-8.
    NotText(String),
    /// The body is over [`Limits::total_bytes`].
    BodyTooLarge {
        /// What arrived.
        bytes: usize,
        /// What was allowed.
        limit: usize,
    },
    /// A file is over [`Limits::file_bytes`].
    FileTooLarge {
        /// The field that carried it.
        name: String,
        /// What was allowed.
        limit: usize,
    },
    /// The body holds more parts than [`Limits::parts`].
    TooManyParts {
        /// What was allowed.
        limit: usize,
    },
    /// A part's header block is over [`Limits::header_bytes`].
    HeadersTooLarge {
        /// What was allowed.
        limit: usize,
    },
}

impl Fault {
    /// The status this deserves.
    ///
    /// 413 when the body is too big, because the client can send less. 400
    /// otherwise, because the bytes are wrong and sending fewer of them will
    /// not help.
    pub const fn status(&self) -> u16 {
        match self {
            Fault::BodyTooLarge { .. }
            | Fault::FileTooLarge { .. }
            | Fault::TooManyParts { .. }
            | Fault::HeadersTooLarge { .. } => 413,
            _ => 400,
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::NoBoundary => f.write_str("the content type names no boundary"),
            Fault::BadBoundary => f.write_str("the boundary is not a boundary"),
            Fault::NoFirstBoundary => f.write_str("the body does not open with the boundary"),
            Fault::Truncated => f.write_str("the body ended before the closing boundary"),
            Fault::NoHeaderEnd => f.write_str("a part's headers have no end"),
            Fault::BadHeader(line) => write!(f, "a header line with no colon: `{line}`"),
            Fault::Unnamed => f.write_str("a part with no name"),
            Fault::NotText(name) => write!(f, "the field `{name}` is not UTF-8"),
            Fault::BodyTooLarge { bytes, limit } => {
                write!(f, "the body is {bytes} bytes and the limit is {limit}")
            }
            Fault::FileTooLarge { name, limit } => {
                write!(f, "the file `{name}` is over the limit of {limit} bytes")
            }
            Fault::TooManyParts { limit } => write!(f, "more than {limit} parts"),
            Fault::HeadersTooLarge { limit } => {
                write!(f, "a part's headers are over the limit of {limit} bytes")
            }
        }
    }
}

impl std::error::Error for Fault {}

/// One part that carried a filename.
///
/// The filename and the media type are what the client said, kept and not
/// believed. `brocanth-storage` takes both as a claim and reads the real type
/// off the bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePart {
    name: String,
    filename: Option<String>,
    declared_type: Option<String>,
    bytes: Vec<u8>,
}

impl FilePart {
    /// The form field this arrived under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The filename the client claimed, if it claimed one that is not empty.
    ///
    /// A file input a person left alone still sends a part, with an empty
    /// filename and no bytes. That part is still a file here, because the
    /// handler is the one that knows whether an empty upload is a mistake.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// The media type the client claimed, if it claimed one.
    ///
    /// Hand this to `ObjectInfo::with_declared_type` and let the store decide
    /// what the bytes really are.
    pub fn declared_type(&self) -> Option<&str> {
        self.declared_type.as_deref()
    }

    /// The content, as it arrived.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many bytes arrived.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether no bytes arrived, which is what an untouched file input sends.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The content, moved out, for a store that takes ownership.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A `multipart/form-data` body, read.
///
/// The text fields and the files are apart, because a schema wants the first
/// and a store wants the second.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Multipart {
    fields: Vec<(String, String)>,
    files: Vec<FilePart>,
}

impl Multipart {
    /// Read a body under [`Limits::DEFAULT`].
    ///
    /// # Errors
    ///
    /// Any [`Fault`]: a body this parser cannot follow, or one over a cap.
    pub fn parse(body: &[u8], boundary: &str) -> Result<Multipart, Fault> {
        Multipart::read(body, boundary, Limits::DEFAULT, true)
    }

    /// Read a body under caps the caller chose.
    ///
    /// # Errors
    ///
    /// Any [`Fault`]: a body this parser cannot follow, or one over a cap.
    pub fn parse_with(body: &[u8], boundary: &str, limits: Limits) -> Result<Multipart, Fault> {
        Multipart::read(body, boundary, limits, true)
    }

    /// Read the text fields and drop the files.
    ///
    /// For a handler that only wants its struct. A file is still walked and
    /// still counted against [`Limits::file_bytes`], so a body that would be
    /// refused whole is refused here too; its bytes are never copied.
    ///
    /// # Errors
    ///
    /// Any [`Fault`]: a body this parser cannot follow, or one over a cap.
    pub fn fields_only(body: &[u8], boundary: &str, limits: Limits) -> Result<Multipart, Fault> {
        Multipart::read(body, boundary, limits, false)
    }

    /// The text fields, in arrival order, with repeats kept.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    /// The files, in arrival order, with repeats kept.
    pub fn files(&self) -> &[FilePart] {
        &self.files
    }

    /// The first text field under this name.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The first file under this name.
    pub fn file(&self, name: &str) -> Option<&FilePart> {
        self.files.iter().find(|f| f.name == name)
    }

    /// Every file under this name, for a `multiple` input.
    pub fn files_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a FilePart> {
        self.files.iter().filter(move |f| f.name == name)
    }

    /// The text fields, as input a schema reads.
    ///
    /// The same rules a urlencoded form gets, so one struct checks both: the
    /// shape is flat, so `age=36` may become a number; an empty field is
    /// [`Value::Null`], so a required field says it is missing; a name that
    /// repeats is a [`Value::List`] in arrival order, which is what a group
    /// of checkboxes sends.
    ///
    /// Files are not in here. A handler asks [`Multipart::file`] for those,
    /// because a schema has no rule that reads bytes.
    pub fn input(&self) -> Input {
        let mut map: Vec<(String, Value)> = Vec::new();
        for (name, text) in &self.fields {
            let value = if text.is_empty() {
                Value::Null
            } else {
                Value::Text(text.clone())
            };
            match map.iter_mut().find(|(k, _)| k == name) {
                Some((_, held)) => match held {
                    Value::List(items) => items.push(value),
                    other => {
                        let first = std::mem::replace(other, Value::Null);
                        *other = Value::List(vec![first, value]);
                    }
                },
                None => map.push((name.clone(), value)),
            }
        }
        Input::flat(Value::Map(map))
    }

    /// The one parser. `keep` says whether a file's bytes are copied.
    fn read(body: &[u8], boundary: &str, limits: Limits, keep: bool) -> Result<Multipart, Fault> {
        if !is_boundary(boundary) {
            return Err(Fault::BadBoundary);
        }
        // First, and before anything is allocated. A body over the cap costs
        // a length comparison, which is the whole point of having the cap.
        if body.len() > limits.total_bytes {
            return Err(Fault::BodyTooLarge {
                bytes: body.len(),
                limit: limits.total_bytes,
            });
        }

        let mut dash = Vec::with_capacity(boundary.len() + 4);
        dash.extend_from_slice(b"--");
        dash.extend_from_slice(boundary.as_bytes());
        let mut crlf_dash = Vec::with_capacity(dash.len() + 2);
        crlf_dash.extend_from_slice(b"\r\n");
        crlf_dash.extend_from_slice(&dash);

        // RFC 2046 allows a preamble before the first boundary and no client
        // writes one. A body that does not open with the boundary is a body
        // we are not sure of, so it is refused rather than searched.
        if !body.starts_with(&dash) {
            return Err(Fault::NoFirstBoundary);
        }

        let mut out = Multipart::default();
        let mut pos = dash.len();
        // Counted rather than read off the two vectors, because a dropped
        // file still took a part.
        let mut seen = 0usize;
        loop {
            // We stand just past a boundary. Two dashes close the body, and
            // whatever follows them is the epilogue, which is not ours.
            if body[pos..].starts_with(b"--") {
                return Ok(out);
            }
            // Transport padding: spaces and tabs a gateway may add.
            while matches!(body.get(pos), Some(b' ' | b'\t')) {
                pos += 1;
            }
            if !body[pos..].starts_with(b"\r\n") {
                return Err(Fault::Truncated);
            }
            pos += 2;

            if seen >= limits.parts {
                return Err(Fault::TooManyParts {
                    limit: limits.parts,
                });
            }
            seen += 1;

            let (headers, after) = read_headers(body, pos, limits.header_bytes)?;
            pos = after;

            // A part ends at the CRLF that belongs to the next boundary, not
            // at the next copy of the boundary text. That CRLF is the
            // delimiter's and not the content's, which is why a file holding
            // `--boundary` in the middle of a line survives.
            let end = find(body, &crlf_dash, pos).ok_or(Fault::Truncated)?;
            let content = &body[pos..end];
            pos = end + crlf_dash.len();

            let disposition = header(&headers, "content-disposition").ok_or(Fault::Unnamed)?;
            let name = parameter(disposition, "name").ok_or(Fault::Unnamed)?;

            match parameter(disposition, "filename") {
                Some(filename) => {
                    if content.len() > limits.file_bytes {
                        return Err(Fault::FileTooLarge {
                            name: name.to_string(),
                            limit: limits.file_bytes,
                        });
                    }
                    // When `keep` is false the file is counted against its
                    // cap above and then dropped, so its bytes are never
                    // copied.
                    if keep {
                        out.files.push(FilePart {
                            name: name.to_string(),
                            filename: if filename.is_empty() {
                                None
                            } else {
                                Some(filename.to_string())
                            },
                            declared_type: header(&headers, "content-type").map(str::to_string),
                            bytes: content.to_vec(),
                        });
                    }
                }
                None => {
                    let text = core::str::from_utf8(content)
                        .map_err(|_| Fault::NotText(name.to_string()))?;
                    out.fields.push((name.to_string(), text.to_string()));
                }
            }
        }
    }
}

/// The boundary a `multipart/form-data` content type names.
///
/// ```
/// let said = "multipart/form-data; boundary=\"a b\"";
/// assert_eq!(brocanth_input::multipart::boundary(said), Some("a b"));
/// ```
pub fn boundary(content_type: &str) -> Option<&str> {
    parameter(content_type, "boundary")
}

/// RFC 2046: 1 to 70 characters, from a short set, and never a trailing
/// space. A boundary that is not one is refused before it is searched for,
/// because an empty needle matches everywhere.
fn is_boundary(boundary: &str) -> bool {
    if boundary.is_empty() || boundary.len() > 70 || boundary.ends_with(' ') {
        return false;
    }
    boundary
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b" '()+_,-./:=? ".contains(&b))
}

/// The first header under this name. Names are already lowercase here.
fn header<'h>(headers: &'h [(String, String)], want: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(k, _)| k == want)
        .map(|(_, v)| v.as_str())
}

/// One part's headers, and where the content starts.
fn read_headers(
    body: &[u8],
    mut pos: usize,
    cap: usize,
) -> Result<(Vec<(String, String)>, usize), Fault> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut used = 0usize;
    loop {
        let end = find(body, b"\r\n", pos).ok_or(Fault::NoHeaderEnd)?;
        let line = &body[pos..end];
        // The cap is checked before the line is turned into a String, so a
        // header block that never ends costs a scan and no memory.
        used += line.len() + 2;
        if used > cap {
            return Err(Fault::HeadersTooLarge { limit: cap });
        }
        pos = end + 2;
        if line.is_empty() {
            return Ok((headers, pos));
        }
        // Lossy on purpose: a filename is the client's and a panic is not a
        // validation answer.
        let text = String::from_utf8_lossy(line);
        let Some((name, value)) = text.split_once(':') else {
            let mut shown: String = text.trim().chars().take(60).collect();
            if shown.len() < text.trim().len() {
                shown.push('…');
            }
            return Err(Fault::BadHeader(shown));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
}

/// One parameter out of a header value: `name="file"`, `name=file`.
///
/// A backslash is a plain character and not an escape. WHATWG HTML tells a
/// client to percent-encode the quote, the CR and the LF in a filename and
/// to leave everything else alone, so `"C:\dir\a.txt"` from an older client
/// keeps its slashes instead of losing them.
fn parameter<'v>(value: &'v str, want: &str) -> Option<&'v str> {
    let bytes = value.as_bytes();
    let mut i = value.find(';')? + 1;
    while i < bytes.len() {
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b';' {
            i += 1;
        }
        let name = value[start..i].trim();
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b';' {
            // A parameter with no value. Not one we are looking for.
            i += 1;
            continue;
        }
        i += 1;
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        let found = if bytes.get(i) == Some(&b'"') {
            i += 1;
            let from = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let text = &value[from..i];
            // Past the closing quote, then past anything up to the next `;`,
            // so a quoted value holding a `;` does not start a parameter.
            i += 1;
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            text
        } else {
            let from = i;
            while i < bytes.len() && bytes[i] != b';' {
                i += 1;
            }
            value[from..i].trim()
        };
        i += 1;
        if name.eq_ignore_ascii_case(want) {
            return Some(found);
        }
    }
    None
}

/// Where the needle next sits in the haystack, at or after `from`.
///
/// The first byte is scanned for before the rest is compared. A boundary is
/// up to 70 bytes and a body is megabytes, and comparing all 70 at every
/// offset is work an attacker chooses the size of.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let first = *needle.first()?;
    let mut i = from;
    while i + needle.len() <= hay.len() {
        let off = hay[i..].iter().position(|&b| b == first)?;
        let at = i + off;
        if at + needle.len() > hay.len() {
            return None;
        }
        if &hay[at..at + needle.len()] == needle {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a browser sends: long, and with characters that are legal in a
    /// boundary and awkward everywhere else.
    const B: &str = "----BrocanthFormBoundary7MA4YWxkTrZu0gW";

    /// A body out of `(headers, content)` pairs, with every CRLF where the
    /// grammar wants it and a closing boundary at the end.
    fn body(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (headers, content) in parts {
            out.extend_from_slice(format!("--{B}\r\n{headers}\r\n\r\n").as_bytes());
            out.extend_from_slice(content);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{B}--\r\n").as_bytes());
        out
    }

    fn field(name: &str) -> String {
        format!("Content-Disposition: form-data; name=\"{name}\"")
    }

    fn upload(name: &str, filename: &str, kind: &str) -> String {
        format!(
            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
             Content-Type: {kind}"
        )
    }

    fn read(body: &[u8]) -> Result<Multipart, Fault> {
        Multipart::parse(body, B)
    }

    // -- what arrives ------------------------------------------------------

    #[test]
    fn several_fields_and_a_file_come_back_apart() {
        let body = body(&[
            (&field("title"), b"A cat"),
            (&field("tags"), b"pets"),
            (
                &upload("photo", "cat.png", "image/png"),
                b"\x89PNG\r\n\x1a\n",
            ),
        ]);
        let sent = read(&body).unwrap();

        assert_eq!(sent.fields().len(), 2);
        assert_eq!(sent.field("title"), Some("A cat"));
        assert_eq!(sent.field("tags"), Some("pets"));

        assert_eq!(sent.files().len(), 1);
        let photo = sent.file("photo").unwrap();
        assert_eq!(photo.name(), "photo");
        assert_eq!(photo.filename(), Some("cat.png"));
        assert_eq!(photo.declared_type(), Some("image/png"));
        assert_eq!(photo.bytes(), b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn a_part_with_no_filename_is_a_value_and_one_with_a_filename_is_a_file() {
        let body = body(&[
            (&field("about"), b"plain text"),
            (&upload("doc", "notes.txt", "text/plain"), b"plain text"),
        ]);
        let sent = read(&body).unwrap();

        assert_eq!(
            sent.fields(),
            [("about".to_string(), "plain text".to_string())]
        );
        assert_eq!(sent.files().len(), 1);
    }

    #[test]
    fn a_name_that_repeats_keeps_every_value_and_becomes_a_list() {
        let body = body(&[
            (&field("tag"), b"red"),
            (&field("tag"), b"green"),
            (&field("tag"), b"blue"),
        ]);
        let sent = read(&body).unwrap();

        assert_eq!(sent.fields().len(), 3);
        assert_eq!(sent.field("tag"), Some("red"));
        assert_eq!(
            sent.input(),
            Input::flat(Value::Map(vec![(
                "tag".to_string(),
                Value::List(vec![
                    Value::Text("red".into()),
                    Value::Text("green".into()),
                    Value::Text("blue".into()),
                ])
            )]))
        );
    }

    #[test]
    fn a_filename_that_repeats_keeps_every_file_in_arrival_order() {
        let body = body(&[
            (&upload("shot", "one.png", "image/png"), b"first"),
            (&upload("shot", "two.png", "image/png"), b"second"),
        ]);
        let sent = read(&body).unwrap();

        let names: Vec<_> = sent.files_named("shot").map(|f| f.filename()).collect();
        assert_eq!(names, [Some("one.png"), Some("two.png")]);
        assert_eq!(sent.file("shot").unwrap().bytes(), b"first");
    }

    #[test]
    fn an_empty_field_is_null_so_a_required_rule_still_says_it_is_missing() {
        let body = body(&[(&field("title"), b"")]);
        let sent = read(&body).unwrap();

        assert_eq!(sent.field("title"), Some(""));
        assert_eq!(
            sent.input(),
            Input::flat(Value::Map(vec![("title".to_string(), Value::Null)]))
        );
    }

    #[test]
    fn a_part_with_no_content_type_keeps_no_claim_rather_than_inventing_one() {
        let headers = "Content-Disposition: form-data; name=\"photo\"; filename=\"cat.png\"";
        let sent = read(&body(&[(headers, b"bytes")])).unwrap();
        assert_eq!(sent.file("photo").unwrap().declared_type(), None);
    }

    #[test]
    fn an_untouched_file_input_arrives_as_a_file_with_nothing_in_it() {
        // What a browser sends for a file input nobody chose a file with.
        // It is still a part, so the handler is the one that decides whether
        // an empty upload is a mistake.
        let body = body(&[(&upload("avatar", "", "application/octet-stream"), b"")]);
        let sent = read(&body).unwrap();

        let avatar = sent.file("avatar").unwrap();
        assert_eq!(avatar.filename(), None);
        assert!(avatar.is_empty());
        assert_eq!(avatar.len(), 0);
    }

    #[test]
    fn zero_parts_is_an_empty_body_and_not_an_error() {
        let sent = read(&body(&[])).unwrap();

        assert!(sent.fields().is_empty());
        assert!(sent.files().is_empty());
        assert_eq!(sent.input(), Input::flat(Value::Map(Vec::new())));
    }

    #[test]
    fn an_epilogue_after_the_closing_boundary_is_not_read() {
        let mut body = body(&[(&field("a"), b"1")]);
        body.extend_from_slice(b"anything at all\r\n");
        assert_eq!(read(&body).unwrap().field("a"), Some("1"));
    }

    #[test]
    fn a_closing_boundary_with_no_crlf_after_it_still_closes() {
        let body = format!("--{B}\r\n{}\r\n\r\n1\r\n--{B}--", field("a")).into_bytes();
        assert_eq!(read(&body).unwrap().field("a"), Some("1"));
    }

    #[test]
    fn transport_padding_after_a_boundary_is_skipped() {
        let body = format!("--{B}  \t\r\n{}\r\n\r\n1\r\n--{B}--\r\n", field("a")).into_bytes();
        assert_eq!(read(&body).unwrap().field("a"), Some("1"));
    }

    // -- the content is bytes and stays bytes ------------------------------

    #[test]
    fn a_boundary_inside_a_file_is_content_and_not_a_delimiter() {
        // The delimiter is CRLF plus the boundary. The same text in the
        // middle of a line, or at the very start of the content, is content.
        let mut content = format!("--{B} is not a delimiter here").into_bytes();
        content.extend_from_slice(b"\r\nnor --");
        content.extend_from_slice(B.as_bytes());
        content.extend_from_slice(b"x\r\nand this is the end");

        let body = body(&[(&upload("f", "a.bin", "application/octet-stream"), &content)]);
        let sent = read(&body).unwrap();

        assert_eq!(sent.file("f").unwrap().bytes(), content.as_slice());
        assert_eq!(sent.files().len(), 1);
    }

    #[test]
    fn a_file_keeps_every_byte_including_the_ones_that_are_not_utf8() {
        let content: &[u8] = &[0x00, 0xff, 0xfe, 0x0d, 0x0a, 0x42];
        let body = body(&[(&upload("f", "a.bin", "application/octet-stream"), content)]);
        assert_eq!(read(&body).unwrap().file("f").unwrap().bytes(), content);
    }

    #[test]
    fn a_declared_type_that_disagrees_with_the_bytes_is_kept_as_a_claim() {
        // This crate does not sniff. The claim and the bytes both travel, and
        // `brocanth-storage` is the one place that decides which to believe.
        let body = body(&[(
            &upload("photo", "cat.png", "image/png"),
            b"%PDF-1.7\r\nnot a png at all",
        )]);
        let photo = read(&body).unwrap();
        let photo = photo.file("photo").unwrap();

        assert_eq!(photo.declared_type(), Some("image/png"));
        assert_eq!(photo.filename(), Some("cat.png"));
        assert!(photo.bytes().starts_with(b"%PDF"));
    }

    // -- the parameters on a header ----------------------------------------

    #[test]
    fn a_parameter_reads_quoted_and_unquoted_alike() {
        let cases = [
            ("form-data; name=plain", "name", Some("plain")),
            ("form-data; name=\"plain\"", "name", Some("plain")),
            ("form-data;name=\"no space\"", "name", Some("no space")),
            ("form-data; NAME=\"upper\"", "name", Some("upper")),
            ("form-data; name = \"spaced\"", "name", Some("spaced")),
            (
                "form-data; name=\"a\"; filename=\"b;c.txt\"",
                "filename",
                Some("b;c.txt"),
            ),
            (
                "form-data; name=\"a\"; filename=\"c:\\dir\\b.txt\"",
                "filename",
                Some("c:\\dir\\b.txt"),
            ),
            ("form-data; name=\"a\"", "filename", None),
            ("form-data", "name", None),
            ("multipart/form-data; boundary=x", "boundary", Some("x")),
            (
                "multipart/form-data; charset=utf-8; boundary=x",
                "boundary",
                Some("x"),
            ),
            (
                "multipart/form-data; boundary=\"a b\"",
                "boundary",
                Some("a b"),
            ),
        ];
        for (value, want, expected) in cases {
            assert_eq!(parameter(value, want), expected, "{value} / {want}");
        }
    }

    #[test]
    fn a_filename_with_a_semicolon_in_it_does_not_start_a_parameter() {
        let headers = "Content-Disposition: form-data; name=\"f\"; filename=\"a;b.png\"";
        let sent = read(&body(&[(headers, b"x")])).unwrap();
        assert_eq!(sent.file("f").unwrap().filename(), Some("a;b.png"));
    }

    #[test]
    fn a_filename_that_is_a_path_keeps_its_bytes_for_the_store_to_defang() {
        // Path traversal is the store's problem, and `Filename` there makes
        // it impossible rather than filtered. Mangling the name here would
        // mean two crates deciding the same thing.
        let headers = "Content-Disposition: form-data; name=\"f\"; filename=\"../../etc/passwd\"";
        let sent = read(&body(&[(headers, b"x")])).unwrap();
        assert_eq!(sent.file("f").unwrap().filename(), Some("../../etc/passwd"));
    }

    // -- the adversarial half ----------------------------------------------

    #[test]
    fn a_body_this_parser_cannot_follow_says_which_way_it_broke() {
        let whole = body(&[(&field("a"), b"1"), (&field("b"), b"2")]);

        let cases: Vec<(&str, Vec<u8>, Fault)> = vec![
            (
                "a truncated body: it stops in the middle of a part",
                whole[..whole.len() - 20].to_vec(),
                Fault::Truncated,
            ),
            (
                "a truncated body: it stops in the middle of the headers",
                format!("--{B}\r\nContent-Dispo").into_bytes(),
                Fault::NoHeaderEnd,
            ),
            (
                "a truncated body: nothing after the first boundary",
                format!("--{B}").into_bytes(),
                Fault::Truncated,
            ),
            (
                "no final boundary: the last part never ends",
                format!("--{B}\r\n{}\r\n\r\n1\r\n", field("a")).into_bytes(),
                Fault::Truncated,
            ),
            (
                "no final boundary: the closing dashes are missing",
                format!("--{B}\r\n{}\r\n\r\n1\r\n--{B}", field("a")).into_bytes(),
                Fault::Truncated,
            ),
            (
                "a part with no name",
                body(&[("Content-Disposition: form-data", b"1")]),
                Fault::Unnamed,
            ),
            (
                "a part with no disposition at all",
                body(&[("Content-Type: text/plain", b"1")]),
                Fault::Unnamed,
            ),
            (
                "a part with no headers at all",
                format!("--{B}\r\n\r\n1\r\n--{B}--\r\n").into_bytes(),
                Fault::Unnamed,
            ),
            (
                "a header line with no colon",
                body(&[(&format!("{}\r\nnonsense", field("a")), b"1")]),
                Fault::BadHeader("nonsense".to_string()),
            ),
            (
                "a body that does not open with the boundary",
                format!("preamble\r\n--{B}--\r\n").into_bytes(),
                Fault::NoFirstBoundary,
            ),
            ("an empty body", Vec::new(), Fault::NoFirstBoundary),
            (
                "bare LF where the grammar wants CRLF",
                format!("--{B}\n{}\n\n1\n--{B}--\n", field("a")).into_bytes(),
                Fault::Truncated,
            ),
            (
                "a text field whose bytes are not UTF-8",
                body(&[(&field("a"), &[0xff, 0xfe])]),
                Fault::NotText("a".to_string()),
            ),
        ];

        for (what, body, expected) in cases {
            assert_eq!(read(&body), Err(expected.clone()), "{what}");
            assert_eq!(expected.status(), 400, "{what}");
        }
    }

    #[test]
    fn a_boundary_that_is_not_one_is_refused_before_anything_is_searched_for() {
        let cases = [
            "",
            " ",
            "ends with a space ",
            "a\r\nb",
            "a\"b",
            &"x".repeat(71),
        ];
        for boundary in cases {
            assert_eq!(
                Multipart::parse(b"--x--\r\n", boundary),
                Err(Fault::BadBoundary),
                "{boundary:?}"
            );
        }
        assert!(is_boundary(&"x".repeat(70)));
        assert!(is_boundary("a b"));
        assert!(is_boundary("----WebKitFormBoundary7MA4YWxkTrZu0gW"));
    }

    #[test]
    fn a_content_type_with_no_boundary_has_none_to_give() {
        assert_eq!(boundary("multipart/form-data"), None);
        assert_eq!(boundary("multipart/form-data; charset=utf-8"), None);
        assert_eq!(boundary("multipart/form-data; boundary=x"), Some("x"));
    }

    // -- every cap ---------------------------------------------------------

    /// Small enough that a test body reaches them.
    const TIGHT: Limits = Limits {
        total_bytes: 4096,
        file_bytes: 64,
        parts: 3,
        header_bytes: 200,
    };

    #[test]
    fn a_body_over_the_total_is_refused_by_its_length_alone() {
        let body = body(&[(&field("a"), &vec![b'x'; 5000])]);
        let fault = Multipart::parse_with(&body, B, TIGHT).unwrap_err();

        assert_eq!(
            fault,
            Fault::BodyTooLarge {
                bytes: body.len(),
                limit: 4096
            }
        );
        assert_eq!(fault.status(), 413);
    }

    #[test]
    fn a_file_over_its_cap_is_refused_and_never_copied() {
        let body = body(&[(
            &upload("f", "big.bin", "application/octet-stream"),
            &[b'x'; 65],
        )]);
        let fault = Multipart::parse_with(&body, B, TIGHT).unwrap_err();

        assert_eq!(
            fault,
            Fault::FileTooLarge {
                name: "f".to_string(),
                limit: 64
            }
        );
        assert_eq!(fault.status(), 413);

        // One byte under, and the same body reads.
        let fits = body2(
            &upload("f", "big.bin", "application/octet-stream"),
            &[b'x'; 64],
        );
        assert_eq!(
            Multipart::parse_with(&fits, B, TIGHT)
                .unwrap()
                .file("f")
                .unwrap()
                .len(),
            64
        );
    }

    /// One part, for the tests that compare a cap against its neighbour.
    fn body2(headers: &str, content: &[u8]) -> Vec<u8> {
        body(&[(headers, content)])
    }

    #[test]
    fn more_parts_than_the_cap_are_refused_at_the_part_that_passes_it() {
        let under = body(&[
            (&field("a"), b"1"),
            (&field("b"), b"2"),
            (&field("c"), b"3"),
        ]);
        assert_eq!(
            Multipart::parse_with(&under, B, TIGHT)
                .unwrap()
                .fields()
                .len(),
            3
        );

        let over = body(&[
            (&field("a"), b"1"),
            (&field("b"), b"2"),
            (&field("c"), b"3"),
            (&field("d"), b"4"),
        ]);
        let fault = Multipart::parse_with(&over, B, TIGHT).unwrap_err();
        assert_eq!(fault, Fault::TooManyParts { limit: 3 });
        assert_eq!(fault.status(), 413);
    }

    #[test]
    fn no_prefix_of_a_good_body_panics_and_only_the_whole_one_reads() {
        // A truncated body is the common case, not the rare one: a client
        // that hangs up mid-upload sends one. Every cut of a known-good body
        // has to answer rather than index past its end.
        let whole = body(&[
            (&field("title"), b"A cat"),
            (
                &upload("photo", "cat.png", "image/png"),
                b"\x89PNG\r\n\x1a\n",
            ),
            (&field("tag"), b""),
        ]);
        // Every cut but the last two, which only drop the CRLF after the
        // closing boundary. That CRLF is the epilogue's and the body is
        // already closed without it.
        for cut in 0..whole.len() - 2 {
            assert!(read(&whole[..cut]).is_err(), "cut at {cut}");
        }
        assert!(read(&whole[..whole.len() - 2]).is_ok());
        assert!(read(&whole).is_ok());
    }

    #[test]
    fn a_part_header_block_over_its_cap_is_refused() {
        let padded = format!("{}\r\nX-Pad: {}", field("a"), "x".repeat(300));
        let fault = Multipart::parse_with(&body2(&padded, b"1"), B, TIGHT).unwrap_err();

        assert_eq!(fault, Fault::HeadersTooLarge { limit: 200 });
        assert_eq!(fault.status(), 413);
    }

    #[test]
    fn a_header_block_with_no_end_costs_a_scan_and_not_the_memory() {
        // The cap is counted as the lines are walked, so a part that opens a
        // header block and never closes it stops at the cap rather than at
        // the end of the body.
        let mut body = format!("--{B}\r\n").into_bytes();
        for _ in 0..100 {
            body.extend_from_slice(b"X-Pad: xxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\n");
        }
        assert_eq!(
            Multipart::parse_with(&body, B, TIGHT),
            Err(Fault::HeadersTooLarge { limit: 200 })
        );
    }

    #[test]
    fn the_defaults_are_the_ones_the_documentation_names() {
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Limits::DEFAULT.total_bytes, 16 * 1024 * 1024);
        assert_eq!(Limits::DEFAULT.file_bytes, 8 * 1024 * 1024);
        assert_eq!(Limits::DEFAULT.parts, 64);
        assert_eq!(Limits::DEFAULT.header_bytes, 8 * 1024);
        // A file cannot be capped above the body that carries it.
        const { assert!(Limits::DEFAULT.file_bytes <= Limits::DEFAULT.total_bytes) };
    }

    #[test]
    fn reading_only_the_fields_still_refuses_a_file_over_its_cap() {
        let body = body(&[(
            &upload("f", "big.bin", "application/octet-stream"),
            &[b'x'; 65],
        )]);
        assert!(Multipart::fields_only(&body, B, TIGHT).is_err());
    }

    #[test]
    fn reading_only_the_fields_keeps_the_fields_and_drops_the_bytes() {
        let body = body(&[
            (&field("title"), b"A cat"),
            (&upload("photo", "cat.png", "image/png"), b"\x89PNG"),
        ]);
        let sent = Multipart::fields_only(&body, B, Limits::DEFAULT).unwrap();

        assert_eq!(sent.field("title"), Some("A cat"));
        assert!(sent.files().is_empty());
    }

    #[test]
    fn a_dropped_file_still_took_a_part() {
        let body = body(&[
            (&upload("a", "a.bin", "application/octet-stream"), b"1"),
            (&upload("b", "b.bin", "application/octet-stream"), b"2"),
            (&upload("c", "c.bin", "application/octet-stream"), b"3"),
            (&upload("d", "d.bin", "application/octet-stream"), b"4"),
        ]);
        assert_eq!(
            Multipart::fields_only(&body, B, TIGHT),
            Err(Fault::TooManyParts { limit: 3 })
        );
    }
}
