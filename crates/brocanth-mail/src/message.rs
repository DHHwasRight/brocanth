//! The message, and the bytes it becomes.
//!
//! # Every body is base64, and that is a security decision
//!
//! A body is the part of a mail an application fills from a template, and a
//! template is where a user's name, a referrer's note or a ticket title ends
//! up. So a body is attacker-adjacent text, and a body that goes on the wire
//! as itself can do three things it must not: spell the multipart boundary and
//! end the part early, carry a line longer than 998 octets and make a relay
//! rewrite the message, and carry a bare CR or LF that a relay and a client
//! disagree about.
//!
//! Base64 removes all three at once, because `A-Za-z0-9+/=` cannot spell any
//! of them, and it costs a third more bytes. No conditional, no "only when the
//! body looks dangerous", no second code path that is wrong: the message is
//! base64 or it is not sent.
//!
//! # Date and Message-ID
//!
//! `Date` is written here, from the system clock. `Message-ID` is not: the
//! submission server writes a correct one from its own domain, and a
//! Message-ID invented by a library is a value that looks authoritative and
//! names a host that does not exist.
//!
//! # What is not here
//!
//! No `Bcc`. A Bcc is not a header, it is an envelope recipient with no header
//! at all, and a `Bcc:` that reaches the wire is the bug every mail library
//! has shipped once. Adding one means adding a recipient list that is
//! deliberately absent from [`Message::render`], which is a seam worth
//! designing on purpose rather than slipping into the same struct as `Cc`.
//!
//! No attachments. An attachment is a second content type, a filename in a
//! header and a size limit, and this crate exists today so that a password
//! reset can be sent.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::address::Address;
use crate::error::MailError;
use crate::header::{address_list, check_header_text, unstructured, FOLD_AT};

/// The longest subject this crate carries, in characters.
///
/// A subject is one line in a list of mail. Past this it is a body somebody
/// put in the wrong field, and it is also enough encoded-words to make the
/// header block larger than the message.
pub const MAX_SUBJECT: usize = 255;

/// A subject line that cannot end its own header.
///
/// Validated for the same reason and with the same rule as a display name --
/// see [`crate::header`]. Public so that an application can refuse a subject
/// where it is composed, rather than at `build`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Subject(String);

impl Subject {
    /// Checks a subject.
    ///
    /// # Errors
    /// [`MailError::InvalidHeader`] when it carries a line break, a control
    /// character, a bidirectional override, or more than [`MAX_SUBJECT`]
    /// characters.
    pub fn parse(raw: &str) -> Result<Self, MailError> {
        let text = raw.trim();
        check_header_text(text).map_err(|reason| MailError::header("subject", reason))?;
        if text.chars().count() > MAX_SUBJECT {
            return Err(MailError::header("subject", "it is too long"));
        }
        Ok(Self(text.to_string()))
    }

    /// The subject, as the person wrote it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Subject({:?})", self.0)
    }
}

/// One message, ready to send.
///
/// Every field is already checked: the addresses are [`Address`] values, the
/// subject is a [`Subject`], and there is at least one recipient. A `Message`
/// that exists is a message that can be rendered, which is what lets
/// [`Message::render`] be infallible and lets a backend have nothing to
/// validate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    reply_to: Vec<Address>,
    subject: Subject,
    text: String,
    html: Option<String>,
}

impl Message {
    /// Starts a message from its sender.
    pub fn builder(from: Address) -> MessageBuilder {
        MessageBuilder {
            from,
            to: Vec::new(),
            cc: Vec::new(),
            reply_to: Vec::new(),
            subject: None,
            text: String::new(),
            html: None,
            error: None,
        }
    }

    /// Who it is from.
    pub fn from(&self) -> &Address {
        &self.from
    }

    /// The `To` recipients.
    pub fn to(&self) -> &[Address] {
        &self.to
    }

    /// The `Cc` recipients.
    pub fn cc(&self) -> &[Address] {
        &self.cc
    }

    /// The `Reply-To` addresses.
    pub fn reply_to(&self) -> &[Address] {
        &self.reply_to
    }

    /// The subject.
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    /// The plain-text body, which every message has.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The HTML body, when there is one.
    pub fn html(&self) -> Option<&str> {
        self.html.as_deref()
    }

    /// Everyone the envelope has to name: `To` then `Cc`, in order.
    ///
    /// A backend builds its envelope from this and never from the headers, so
    /// the two cannot drift.
    pub fn recipients(&self) -> Vec<&Address> {
        self.to.iter().chain(self.cc.iter()).collect()
    }

    /// The RFC 5322 bytes, with CR LF line endings.
    ///
    /// Infallible: everything that could fail already did, at `build`.
    pub fn render(&self) -> Vec<u8> {
        self.render_at(now_unix(), &boundary())
    }

    /// The same bytes, with the clock and the boundary supplied.
    ///
    /// Crate-private, and it exists so the tests can assert on a whole message
    /// rather than on the parts of one that happen to be deterministic.
    pub(crate) fn render_at(&self, at: i64, boundary: &str) -> Vec<u8> {
        let mut out = String::with_capacity(512 + self.text.len() + self.html_len());

        push_line(&mut out, &address_list("From", &[self.from.header_value()]));
        if !self.to.is_empty() {
            push_line(&mut out, &address_list("To", &rendered(&self.to)));
        }
        if !self.cc.is_empty() {
            push_line(&mut out, &address_list("Cc", &rendered(&self.cc)));
        }
        if !self.reply_to.is_empty() {
            push_line(
                &mut out,
                &address_list("Reply-To", &rendered(&self.reply_to)),
            );
        }
        push_line(
            &mut out,
            &format!(
                "Subject: {}",
                unstructured("Subject", self.subject.as_str())
            ),
        );
        push_line(&mut out, &format!("Date: {}", rfc5322_date(at)));
        push_line(&mut out, "MIME-Version: 1.0");

        match &self.html {
            None => {
                push_line(&mut out, "Content-Type: text/plain; charset=utf-8");
                push_line(&mut out, "Content-Transfer-Encoding: base64");
                out.push_str("\r\n");
                out.push_str(&base64_body(&self.text));
            }
            Some(html) => {
                // `multipart/alternative` and not `mixed`: the two parts are
                // the same message, and the client shows the last one it can
                // render. So the plain text goes first and the HTML second,
                // which is the order RFC 2046 §5.1.4 gives -- least faithful
                // first.
                push_line(
                    &mut out,
                    &format!("Content-Type: multipart/alternative; boundary=\"{boundary}\""),
                );
                out.push_str("\r\n");
                push_line(&mut out, "This is a message in MIME format.");
                out.push_str("\r\n");

                push_line(&mut out, &format!("--{boundary}"));
                push_line(&mut out, "Content-Type: text/plain; charset=utf-8");
                push_line(&mut out, "Content-Transfer-Encoding: base64");
                out.push_str("\r\n");
                out.push_str(&base64_body(&self.text));

                push_line(&mut out, &format!("--{boundary}"));
                push_line(&mut out, "Content-Type: text/html; charset=utf-8");
                push_line(&mut out, "Content-Transfer-Encoding: base64");
                out.push_str("\r\n");
                out.push_str(&base64_body(html));

                push_line(&mut out, &format!("--{boundary}--"));
            }
        }
        out.into_bytes()
    }

    fn html_len(&self) -> usize {
        self.html.as_ref().map_or(0, String::len)
    }
}

fn rendered(addresses: &[Address]) -> Vec<String> {
    addresses.iter().map(Address::header_value).collect()
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push_str("\r\n");
}

/// Base64, wrapped so no line reaches the 998-octet limit, CR LF terminated.
fn base64_body(text: &str) -> String {
    let encoded = STANDARD.encode(text.as_bytes());
    let mut out = String::with_capacity(encoded.len() + encoded.len() / FOLD_AT * 2 + 2);
    for chunk in encoded.as_bytes().chunks(76) {
        // `chunks` of base64 output, so every chunk is ASCII by construction.
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\r\n");
    }
    if encoded.is_empty() {
        out.push_str("\r\n");
    }
    out
}

/// A multipart boundary nobody can guess.
///
/// It does not strictly have to be unguessable, because every body is base64
/// and no body can spell a boundary. It is anyway: unguessable is one line
/// here, and "it cannot matter" is the sentence in front of most MIME
/// smuggling bugs.
fn boundary() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // The OS has no randomness. Uniqueness is the part that must hold, and
        // a counter with the process id gives that. Secrecy is the part that
        // was belt and braces.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        bytes[..8].copy_from_slice(&n.to_le_bytes());
        bytes[8..].copy_from_slice(&u64::from(std::process::id()).to_le_bytes());
    }
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("=_brocanth_{hex}")
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// `Tue, 27 Oct 2026 09:41:03 +0000`.
///
/// UTC, always, with a numeric offset and no name. A local time zone in a mail
/// header tells a recipient where the server is, and a named zone is the one
/// field of RFC 5322 that nothing agrees on.
fn rfc5322_date(unix: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday, which is why the table starts there.
    let weekday = DAYS[days.rem_euclid(7) as usize];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        weekday,
        day,
        MONTHS[(month - 1) as usize],
        year,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
    )
}

/// Howard Hinnant's `civil_from_days`, which is exact for every day this
/// program will ever see and needs no table.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Collects a message and checks it once, at [`MessageBuilder::build`].
///
/// The first refusal is kept and the rest of the chain is still accepted, so
/// `build` reports the first thing that was wrong rather than the last. A
/// builder that returned `Result` from every method would make the common call
/// -- five lines of `?` in a controller -- unreadable, and the check would
/// still have to happen at `build` for the recipient count.
#[derive(Clone, Debug)]
pub struct MessageBuilder {
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    reply_to: Vec<Address>,
    subject: Option<Subject>,
    text: String,
    html: Option<String>,
    error: Option<MailError>,
}

impl MessageBuilder {
    /// Adds a `To` recipient.
    #[must_use]
    pub fn to(mut self, address: Address) -> Self {
        self.to.push(address);
        self
    }

    /// Adds several `To` recipients.
    #[must_use]
    pub fn to_all(mut self, addresses: impl IntoIterator<Item = Address>) -> Self {
        self.to.extend(addresses);
        self
    }

    /// Adds a `Cc` recipient.
    #[must_use]
    pub fn cc(mut self, address: Address) -> Self {
        self.cc.push(address);
        self
    }

    /// Adds several `Cc` recipients.
    #[must_use]
    pub fn cc_all(mut self, addresses: impl IntoIterator<Item = Address>) -> Self {
        self.cc.extend(addresses);
        self
    }

    /// Adds a `Reply-To` address.
    #[must_use]
    pub fn reply_to(mut self, address: Address) -> Self {
        self.reply_to.push(address);
        self
    }

    /// Sets the subject, checking it.
    ///
    /// A refusal is held until [`MessageBuilder::build`].
    #[must_use]
    pub fn subject(mut self, subject: impl AsRef<str>) -> Self {
        match Subject::parse(subject.as_ref()) {
            Ok(parsed) => self.subject = Some(parsed),
            Err(e) => self.keep(e),
        }
        self
    }

    /// Sets the subject from an already-checked value.
    #[must_use]
    pub fn subject_value(mut self, subject: Subject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Sets the plain-text body.
    #[must_use]
    pub fn text(mut self, body: impl Into<String>) -> Self {
        self.text = body.into();
        self
    }

    /// Sets the HTML body.
    ///
    /// The plain-text body stays required. A mail with only an HTML part is
    /// unreadable in a terminal client, scores badly with every spam filter,
    /// and is the reason a password-reset link lands in a junk folder.
    #[must_use]
    pub fn html(mut self, body: impl Into<String>) -> Self {
        self.html = Some(body.into());
        self
    }

    /// Checks what is left to check and hands back the message.
    ///
    /// # Errors
    /// The first refusal any step made, or [`MailError::NoRecipient`] when
    /// there is no `To` and no `Cc`.
    pub fn build(self) -> Result<Message, MailError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if self.to.is_empty() && self.cc.is_empty() {
            return Err(MailError::NoRecipient);
        }
        Ok(Message {
            from: self.from,
            to: self.to,
            cc: self.cc,
            reply_to: self.reply_to,
            subject: self.subject.unwrap_or(Subject(String::new())),
            text: self.text,
            html: self.html,
        })
    }

    fn keep(&mut self, error: MailError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

/// Rebuilds a [`Message`] from parts that have already been checked.
///
/// Used by the `serde` feature, which has to run every validator again on the
/// way in. Not public: a caller builds with [`Message::builder`].
#[cfg(feature = "serde")]
pub(crate) fn assemble(
    from: Address,
    to: Vec<Address>,
    cc: Vec<Address>,
    reply_to: Vec<Address>,
    subject: Subject,
    text: String,
    html: Option<String>,
) -> Result<Message, MailError> {
    let mut builder = Message::builder(from)
        .to_all(to)
        .cc_all(cc)
        .subject_value(subject)
        .text(text);
    for address in reply_to {
        builder = builder.reply_to(address);
    }
    if let Some(html) = html {
        builder = builder.html(html);
    }
    builder.build()
}

#[cfg(test)]
mod date_tests {
    use super::*;

    #[test]
    fn the_date_header_is_rfc_5322() {
        // 2026-10-27T09:41:03Z, a Tuesday.
        assert_eq!(
            rfc5322_date(1_793_094_063),
            "Tue, 27 Oct 2026 09:41:03 +0000"
        );
        assert_eq!(rfc5322_date(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        // A leap day, to prove the civil calendar is not an approximation.
        assert_eq!(
            rfc5322_date(1_709_164_800),
            "Thu, 29 Feb 2024 00:00:00 +0000"
        );
    }
}
