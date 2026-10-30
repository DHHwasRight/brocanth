//! An address that cannot spell a header.
//!
//! [`brocanth-storage`](https://docs.rs/brocanth-storage) makes path traversal
//! impossible rather than filtered: a path is built from an `ObjectKey`, and
//! `../../etc/passwd` is not a value of that type. This is the same move for
//! the same reason. An envelope and a `To:` line are built from an
//! [`Address`], and `victim@example.test\r\nBcc: harvest@attacker.test` is not
//! a value of that type. There is no sanitising step to forget at a call site,
//! because there is no call site that takes a `&str`.
//!
//! # What the grammar is, and why it is this narrow
//!
//! `local@domain`, where every octet is ASCII graphic (0x21..=0x7e), the local
//! part carries none of the RFC 5322 specials that could close the address
//! (`<>,;:"\\()[]`), and the domain is dot-separated labels of letters, digits
//! and hyphens.
//!
//! That refuses things RFC 5322 allows: quoted local parts (`"a b"@x.test`),
//! address literals (`user@[192.0.2.1]`), comments, and non-ASCII under
//! SMTPUTF8. Each of those is rare, each of them is a second grammar to get
//! right in the place where getting it wrong is a blind carbon copy, and
//! between them they are a rounding error of real deliverable mail. A narrow
//! grammar that refuses a few real addresses is the cheaper mistake here; the
//! other one ships the Bcc.
//!
//! It also deliberately agrees with `brocanth_auth::EmailAddress`, which
//! lowercases and checks one `@`, a non-empty local part and a dotted domain.
//! An address that crate accepted parses here. The reverse is not promised:
//! this one is stricter, because this one goes on a wire.
//!
//! # Case
//!
//! The domain is lowercased, because DNS is case-insensitive and two spellings
//! of one host are one host. The local part is left alone, because RFC 5321
//! §2.4 says it belongs to the receiving server and no sender gets to
//! normalise it. `brocanth-auth` lowercases the whole address before it
//! stores one, so in practice both halves arrive lowercase anyway.

use crate::error::MailError;
use crate::header::{check_header_text, phrase};

/// The longest display name this crate carries.
///
/// A name longer than this is not a name. The cap is on characters rather
/// than octets so that it does not depend on the alphabet somebody writes in.
pub const MAX_DISPLAY_NAME: usize = 100;

/// The longest address, in octets. RFC 5321 §4.5.3.1.3 puts the forward path
/// at 256 octets including the angle brackets.
pub const MAX_ADDRESS: usize = 254;

/// The longest local part, in octets. RFC 5321 §4.5.3.1.1.
pub const MAX_LOCAL_PART: usize = 64;

/// The longest domain, in octets. RFC 5321 §4.5.3.1.2.
pub const MAX_DOMAIN: usize = 255;

/// A mailbox: an address, and optionally the name shown beside it.
///
/// Built only by [`Address::parse`] or [`Address::named`], both of which
/// refuse everything described in this module. There is no public field and no
/// `From<String>`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Address {
    display: Option<String>,
    addr: String,
}

impl Address {
    /// Checks an address with no display name.
    ///
    /// # Errors
    /// [`MailError::InvalidAddress`] naming the rule it broke. The refused
    /// text is not part of the error: see [`MailError`].
    pub fn parse(raw: &str) -> Result<Self, MailError> {
        Ok(Self {
            display: None,
            addr: parse_addr_spec(raw)?,
        })
    }

    /// Checks an address and the name shown beside it.
    ///
    /// # Errors
    /// [`MailError::InvalidAddress`] when the address is not usable, or
    /// [`MailError::InvalidHeader`] when the display name carries a line
    /// break, a control character or a bidirectional override.
    pub fn named(display: &str, raw: &str) -> Result<Self, MailError> {
        let addr = parse_addr_spec(raw)?;
        let display = display.trim();
        check_header_text(display).map_err(|reason| MailError::header("display name", reason))?;
        if display.chars().count() > MAX_DISPLAY_NAME {
            return Err(MailError::header("display name", "it is too long"));
        }
        Ok(Self {
            // An empty name is no name. A `"" <a@b.test>` renders as an empty
            // pair of quotes in every client that shows one.
            display: if display.is_empty() {
                None
            } else {
                Some(display.to_string())
            },
            addr,
        })
    }

    /// The address itself, with no name and no angle brackets.
    ///
    /// This is what goes on the SMTP envelope and what a test asserts on.
    pub fn address(&self) -> &str {
        &self.addr
    }

    /// The display name, when there is one.
    pub fn display_name(&self) -> Option<&str> {
        self.display.as_deref()
    }

    /// The part before the `@`.
    pub fn local_part(&self) -> &str {
        self.addr.split_once('@').map_or(&self.addr, |(l, _)| l)
    }

    /// The part after the `@`, lowercased.
    pub fn domain(&self) -> &str {
        self.addr.split_once('@').map_or("", |(_, d)| d)
    }

    /// What this mailbox looks like inside a `From`, `To`, `Cc` or `Reply-To`.
    ///
    /// Every byte of it is ASCII graphic or a space: a non-ASCII name is an
    /// RFC 2047 encoded-word, and an ASCII one is a quoted-string.
    pub fn header_value(&self) -> String {
        match &self.display {
            None => self.addr.clone(),
            Some(display) => format!("{} <{}>", phrase(display), self.addr),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.header_value())
    }
}

impl std::fmt::Debug for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Address({})", self.header_value())
    }
}

/// Everything RFC 5322 calls a special. None of them may appear unquoted, and
/// this crate has no quoted local part, so none of them may appear at all.
const SPECIALS: &[char] = &[
    '<', '>', '(', ')', '[', ']', ':', ';', '@', '\\', ',', '.', '"',
];

fn parse_addr_spec(raw: &str) -> Result<String, MailError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(MailError::address("it is empty"));
    }
    if trimmed.len() > MAX_ADDRESS {
        return Err(MailError::address("it is too long"));
    }
    // Checked before the split, so a CR, an LF, a NUL or a space is reported
    // as itself rather than as "no domain".
    for byte in trimmed.bytes() {
        if !(0x21..=0x7e).contains(&byte) {
            return Err(MailError::address(
                "it contains a byte that is not an ASCII graphic character",
            ));
        }
    }
    let mut parts = trimmed.split('@');
    let (local, domain) = match (parts.next(), parts.next(), parts.next()) {
        (Some(local), Some(domain), None) => (local, domain),
        _ => return Err(MailError::address("it does not have exactly one `@`")),
    };

    if local.is_empty() {
        return Err(MailError::address("it has no local part"));
    }
    if local.len() > MAX_LOCAL_PART {
        return Err(MailError::address("its local part is too long"));
    }
    // A dot is legal inside a local part and illegal at either end or doubled.
    // It is handled apart from the other specials for exactly that reason.
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return Err(MailError::address("its local part misplaces a dot"));
    }
    if local.chars().any(|c| SPECIALS.contains(&c) && c != '.') {
        return Err(MailError::address(
            "its local part contains a character that would close the address",
        ));
    }

    if domain.is_empty() {
        return Err(MailError::address("it has no domain"));
    }
    if domain.len() > MAX_DOMAIN {
        return Err(MailError::address("its domain is too long"));
    }
    if !domain.contains('.') {
        return Err(MailError::address("its domain has no dot"));
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return Err(MailError::address("its domain has an empty label"));
        }
        if label.len() > 63 {
            return Err(MailError::address(
                "its domain has a label that is too long",
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(MailError::address(
                "its domain has a label that starts or ends with a hyphen",
            ));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(MailError::address(
                "its domain has a label with a character that is not a letter, a digit or a hyphen",
            ));
        }
    }

    Ok(format!("{}@{}", local, domain.to_ascii_lowercase()))
}
