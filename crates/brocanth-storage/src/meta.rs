//! Enough about an object to serve it, and no more.
//!
//! Three types: what the client claims ([`ObjectInfo`]), what the store knows
//! ([`ObjectMeta`]), and the two scalars that reach an HTTP header
//! ([`ContentType`], [`Filename`]). Both of those are validated at
//! construction, because both end up in a response header and neither is ours.

use crate::key::ObjectKey;

/// A media type that is safe to put in a header.
///
/// A newtype rather than a `String`, and a parser rather than a trim, because
/// this value arrives in a multipart part header, is chosen entirely by the
/// client, and leaves again in `Content-Type`. A CR or an LF in it is response
/// splitting; a byte over 0x7e is a header a proxy and an origin disagree
/// about. So the grammar is RFC 9110's `token "/" token` plus optional
/// `; parameter=value` built from the same token characters, and anything else
/// is not a media type.
///
/// There is no `mime` dependency. The whole of what this crate does with a
/// media type is store it, hand it back, and tell the serving half whether it
/// is safe to render inline — none of which needs a registry of two thousand
/// extensions.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentType(String);

impl ContentType {
    /// `application/octet-stream`: what an unknown object is served as.
    pub fn octet_stream() -> Self {
        Self("application/octet-stream".to_string())
    }

    /// Parses a media type, lowercasing the type and subtype.
    ///
    /// Answers `None` for anything that is not `token/token` with optional
    /// parameters, which includes every byte a header must not carry.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.len() > 180 {
            return None;
        }
        let (essence, params) = match text.split_once(';') {
            Some((e, p)) => (e.trim(), Some(p)),
            None => (text, None),
        };
        let (ty, subtype) = essence.split_once('/')?;
        if !is_token(ty) || !is_token(subtype) {
            return None;
        }
        let mut out = format!(
            "{}/{}",
            ty.to_ascii_lowercase(),
            subtype.to_ascii_lowercase()
        );
        if let Some(params) = params {
            for param in params.split(';') {
                let param = param.trim();
                if param.is_empty() {
                    continue;
                }
                let (name, value) = param.split_once('=')?;
                let value = value.trim().trim_matches('"');
                if !is_token(name.trim()) || !is_token(value) {
                    return None;
                }
                out.push_str("; ");
                out.push_str(&name.trim().to_ascii_lowercase());
                out.push('=');
                out.push_str(value);
            }
        }
        Some(Self(out))
    }

    /// Recognises a media type from the first bytes of the content.
    ///
    /// Only the four raster formats this crate can derive a variant from, and
    /// that is the point: the sniffed type is trusted over the client's
    /// declared one in [`Storage::put`], so the set of things it can claim has
    /// to be the set of things that are inert in a browser. Adding
    /// `text/html` here would be adding stored XSS.
    ///
    /// [`Storage::put`]: crate::storage::Storage::put
    pub fn sniff(content: &[u8]) -> Option<Self> {
        let starts =
            |magic: &[u8]| content.len() >= magic.len() && &content[..magic.len()] == magic;
        if starts(b"\x89PNG\r\n\x1a\n") {
            return Some(Self("image/png".into()));
        }
        if starts(b"\xff\xd8\xff") {
            return Some(Self("image/jpeg".into()));
        }
        if starts(b"GIF87a") || starts(b"GIF89a") {
            return Some(Self("image/gif".into()));
        }
        if content.len() >= 12 && &content[0..4] == b"RIFF" && &content[8..12] == b"WEBP" {
            return Some(Self("image/webp".into()));
        }
        None
    }

    /// The media type as it goes in the header.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `type/subtype`, parameters dropped.
    pub fn essence(&self) -> &str {
        self.0.split(';').next().unwrap_or(&self.0).trim()
    }

    /// Whether a browser may be allowed to render this inline.
    ///
    /// True only for the raster images [`ContentType::sniff`] recognises.
    /// Everything else — including a type the client declared and this crate
    /// believed for want of a better answer — must be served as an attachment,
    /// with `X-Content-Type-Options: nosniff`, and ideally from an origin that
    /// holds no session cookie. That decision belongs to the serving half;
    /// this is the bit it needs to make it.
    pub fn is_safe_to_inline(&self) -> bool {
        matches!(
            self.essence(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        )
    }

    /// Whether this crate would try to derive an image variant from it.
    pub fn is_image(&self) -> bool {
        self.is_safe_to_inline()
    }
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'..=b'`' | b'|' | b'~')
        })
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ContentType({})", self.0)
    }
}

/// The name the upload arrived with, kept for the download and for nothing
/// else.
///
/// **This never becomes a path component.** The backend builds paths from an
/// [`ObjectKey`] only, so what is left for this type is the other half of the
/// problem: the name goes back out in a `Content-Disposition` header and in an
/// HTML page. So the constructor drops every path separator, every control
/// character, every quote and every leading dot, caps the length, and answers
/// `None` when nothing survives. A name that was `../../etc/passwd` becomes
/// `etcpasswd`, which is harmless because it was never going to be opened.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Filename(String);

/// The longest filename this crate keeps. Shorter than every filesystem's
/// limit, because the value is metadata and a kilobyte of it is an attack on
/// the metadata file rather than a filename.
pub const MAX_FILENAME_LEN: usize = 120;

impl Filename {
    /// Sanitises a client-supplied name, or answers `None` if nothing usable
    /// is left.
    pub fn sanitise(raw: &str) -> Option<Self> {
        let mut out = String::with_capacity(raw.len().min(MAX_FILENAME_LEN));
        for c in raw.chars() {
            let keep = !c.is_control()
                && !matches!(c, '/' | '\\' | ':' | '"' | '\'' | '<' | '>' | '|' | '?' | '*')
                // A right-to-left override turns `evil.exe` into `evil.txt` in
                // every file listing that renders it. The trick is old and
                // still works.
                && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}');
            if !keep {
                continue;
            }
            // Measured before the push, so the cap is a byte cap that never
            // cuts a character in half.
            if out.len() + c.len_utf8() > MAX_FILENAME_LEN {
                break;
            }
            out.push(c);
        }
        // Leading dots make a hidden file and `..` makes a parent. Neither can
        // reach the filesystem from here, but a name that renders as `..` in a
        // download dialogue is still a name worth refusing.
        let out = out.trim().trim_start_matches('.').trim().to_string();
        if out.is_empty() {
            None
        } else {
            Some(Self(out))
        }
    }

    /// The sanitised name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The extension, lowercased, without the dot.
    pub fn extension(&self) -> Option<String> {
        let (_, ext) = self.0.rsplit_once('.')?;
        if ext.is_empty() || ext.len() > 16 {
            None
        } else {
            Some(ext.to_ascii_lowercase())
        }
    }
}

impl std::fmt::Display for Filename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for Filename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Filename({})", self.0)
    }
}

/// What the caller claims about the bytes it is storing.
///
/// Both fields are claims, not facts, and the backend treats them as such: see
/// [`Storage::put`](crate::storage::Storage::put) for what it believes.
#[derive(Clone, Debug, Default)]
pub struct ObjectInfo {
    /// The name the upload arrived with, already sanitised.
    pub filename: Option<Filename>,
    /// The media type the client declared.
    pub declared_type: Option<ContentType>,
}

impl ObjectInfo {
    /// Nothing claimed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the filename, sanitising it.
    #[must_use]
    pub fn with_filename(mut self, raw: &str) -> Self {
        self.filename = Filename::sanitise(raw);
        self
    }

    /// Sets the declared media type, dropping it if it is not one.
    #[must_use]
    pub fn with_declared_type(mut self, raw: &str) -> Self {
        self.declared_type = ContentType::parse(raw);
        self
    }
}

/// What the store knows about an object.
///
/// No timestamp, and that is a decision rather than an omission: the address
/// is a strong ETag ([`ObjectKey::etag`]), so a serving half has everything a
/// conditional request needs without a `Last-Modified` that a backend
/// migration would silently change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    /// Where it lives.
    pub key: ObjectKey,
    /// How many bytes.
    pub size: u64,
    /// What to serve it as. Never absent: an object whose type could not be
    /// established is `application/octet-stream`.
    pub content_type: ContentType,
    /// The name of the upload that created the object, when there was one.
    ///
    /// Under content addressing this is the name of *an* upload, not of this
    /// reference: the second person to upload the same bytes does not get
    /// their filename recorded, deliberately — see
    /// [`Storage::put`](crate::storage::Storage::put). Keep the per-record
    /// name on the record.
    pub filename: Option<Filename>,
}
