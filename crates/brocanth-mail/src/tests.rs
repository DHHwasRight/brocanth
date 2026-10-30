//! The tests, and the first module is the one that matters: a header that
//! cannot be ended from inside a value.

use std::sync::Arc;

use crate::address::Address;
use crate::error::MailError;
use crate::mailer::Mailer;
use crate::memory::Memory;
use crate::message::{Message, Subject};
use crate::plugin::Mail;
use crate::rt::block_on;

/// Every way anybody has tried to end a header from inside a value.
///
/// `%0a` and `&#10;` are in the list because they are what reaches a form
/// field; they are not line breaks to this crate, they are five harmless
/// characters, and the test below asserts exactly that distinction rather
/// than pretending they are refused.
const BREAKS: &[&str] = &[
    "\r\n", "\n", "\r", "\r\n\r\n", "\n\n", "\u{0}", "\u{b}", "\u{c}", "\u{85}", "\u{2028}",
    "\u{2029}", "\t",
];

fn from() -> Address {
    Address::parse("no-reply@example.test").expect("a sender")
}

fn to() -> Address {
    Address::parse("person@example.test").expect("a recipient")
}

mod header_injection {
    use super::*;

    #[test]
    fn no_break_survives_in_a_subject() {
        for attempt in BREAKS {
            let injected = format!("Reset your password{attempt}Bcc: harvest@attacker.test");
            assert!(
                matches!(
                    Subject::parse(&injected),
                    Err(MailError::InvalidHeader {
                        field: "subject",
                        ..
                    })
                ),
                "{attempt:?} was accepted in a subject"
            );
            // And the same refusal through the builder, because that is the
            // path a controller takes.
            assert!(
                Message::builder(from())
                    .to(to())
                    .subject(&injected)
                    .text("body")
                    .build()
                    .is_err(),
                "{attempt:?} was accepted by the builder"
            );
        }
    }

    #[test]
    fn no_break_survives_in_a_display_name() {
        for attempt in BREAKS {
            let injected = format!("Example{attempt}Bcc: harvest@attacker.test");
            assert!(
                matches!(
                    Address::named(&injected, "no-reply@example.test"),
                    Err(MailError::InvalidHeader {
                        field: "display name",
                        ..
                    })
                ),
                "{attempt:?} was accepted in a display name"
            );
        }
    }

    #[test]
    fn no_break_survives_in_an_address() {
        for attempt in BREAKS {
            let injected = format!("person@example.test{attempt}Bcc: harvest@attacker.test");
            assert!(
                matches!(
                    Address::parse(&injected),
                    Err(MailError::InvalidAddress { .. })
                ),
                "{attempt:?} was accepted in an address"
            );
            // The break in front of the address too, which is how a value
            // appended to a header rather than prepended to one arrives.
            let injected = format!("{attempt}person@example.test");
            assert!(
                matches!(
                    Address::parse(&injected),
                    Err(MailError::InvalidAddress { .. })
                ) || Address::parse(&injected).is_ok_and(|a| a.address() == "person@example.test"),
                "{attempt:?} changed the address it was in front of"
            );
        }
    }

    #[test]
    fn percent_encoded_breaks_are_text_and_stay_text() {
        // A controller that URL-decodes its own form field before building a
        // subject is the bug. This crate sees five characters, keeps them, and
        // the rendered header still has exactly one `Subject:` line.
        let message = Message::builder(from())
            .to(to())
            .subject("Reset%0d%0aBcc: harvest@attacker.test")
            .text("body")
            .build()
            .expect("literal percent signs are text");
        let wire = rendered(&message);
        assert_eq!(wire.matches("\r\nBcc:").count(), 0);
        assert_eq!(count_headers(&wire, "Subject:"), 1);
    }

    #[test]
    fn a_quoted_display_name_cannot_close_its_own_quotes() {
        let address = Address::named("Ex\"ample\\, Bcc: harvest@attacker.test", "a@example.test")
            .expect("quotes and backslashes are text");
        assert_eq!(
            address.header_value(),
            "\"Ex\\\"ample\\\\, Bcc: harvest@attacker.test\" <a@example.test>"
        );
    }

    #[test]
    fn an_over_long_subject_is_refused_rather_than_truncated() {
        let long = "a".repeat(crate::MAX_SUBJECT + 1);
        assert!(matches!(
            Subject::parse(&long),
            Err(MailError::InvalidHeader {
                field: "subject",
                ..
            })
        ));
    }

    #[test]
    fn an_over_long_display_name_is_refused() {
        let long = "a".repeat(crate::MAX_DISPLAY_NAME + 1);
        assert!(matches!(
            Address::named(&long, "a@example.test"),
            Err(MailError::InvalidHeader {
                field: "display name",
                ..
            })
        ));
    }
}

mod addresses {
    use super::*;

    /// Everything that is not a mailbox address.
    const NOT_ADDRESSES: &[&str] = &[
        "",
        "   ",
        "person",
        "@example.test",
        "person@",
        "person@localhost",
        "person@@example.test",
        "person@example@test",
        "a b@example.test",
        "person @example.test",
        "person@ example.test",
        ".person@example.test",
        "person.@example.test",
        "per..son@example.test",
        "<person@example.test>",
        "Example <person@example.test>",
        "person@example.test, other@example.test",
        "person@example.test;other@example.test",
        "person(comment)@example.test",
        "\"person\"@example.test",
        "person@[192.0.2.1]",
        "person@-example.test",
        "person@example-.test",
        "person@example..test",
        "pérson@example.test",
        "person@exämple.test",
        "person@example.test\u{0}",
        "person@example.test\u{feff}",
    ];

    #[test]
    fn nothing_in_the_list_is_an_address() {
        for attempt in NOT_ADDRESSES {
            assert!(
                matches!(
                    Address::parse(attempt),
                    Err(MailError::InvalidAddress { .. })
                ),
                "{attempt:?} parsed as an address"
            );
        }
    }

    #[test]
    fn a_real_address_parses_and_normalises_its_domain() {
        let address = Address::parse("  Person.Name+tag@Example.TEST  ").expect("an address");
        assert_eq!(address.address(), "Person.Name+tag@example.test");
        assert_eq!(address.local_part(), "Person.Name+tag");
        assert_eq!(address.domain(), "example.test");
        assert_eq!(address.display_name(), None);
        assert_eq!(address.header_value(), "Person.Name+tag@example.test");
    }

    #[test]
    fn what_brocanth_auth_accepts_this_accepts() {
        // `brocanth_auth::EmailAddress::parse` lowercases and asks for one
        // `@`, a non-empty local part and a dotted domain. Its output is the
        // input this crate gets in the only journey that matters today.
        for accepted in [
            "person@example.test",
            "person+tag@example.test",
            "p@a.b",
            "first.last@mail.example.co.uk",
        ] {
            assert!(Address::parse(accepted).is_ok(), "{accepted:?} was refused");
        }
    }

    #[test]
    fn an_empty_display_name_is_no_display_name() {
        let address = Address::named("   ", "person@example.test").expect("an address");
        assert_eq!(address.display_name(), None);
        assert_eq!(address.header_value(), "person@example.test");
    }

    #[test]
    fn a_non_ascii_display_name_is_encoded_not_passed_through() {
        let address = Address::named("Café Münster", "person@example.test").expect("an address");
        let value = address.header_value();
        assert!(value.starts_with("=?utf-8?B?"), "{value}");
        assert!(value.ends_with("<person@example.test>"), "{value}");
        assert!(
            value.is_ascii(),
            "a header carried a non-ASCII byte: {value}"
        );
    }
}

mod rendering {
    use super::*;

    #[test]
    fn a_text_only_message_renders_as_one_part() {
        let message = Message::builder(from())
            .to(to())
            .subject("Reset your password")
            .text("Open https://example.test/reset?t=abc")
            .build()
            .expect("a message");
        let wire = rendered(&message);

        assert!(wire.starts_with("From: no-reply@example.test\r\n"));
        assert!(wire.contains("To: person@example.test\r\n"));
        assert!(wire.contains("Subject: Reset your password\r\n"));
        assert!(wire.contains("Date: "));
        assert!(wire.contains("MIME-Version: 1.0\r\n"));
        assert!(wire.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(wire.contains("Content-Transfer-Encoding: base64\r\n"));
        assert!(!wire.contains("multipart"));
        assert_eq!(
            bodies(&wire),
            vec!["Open https://example.test/reset?t=abc".to_string()]
        );
    }

    #[test]
    fn a_text_and_html_message_renders_as_multipart_alternative() {
        let message = Message::builder(from())
            .to(to())
            .subject("Reset your password")
            .text("Open https://example.test/reset?t=abc")
            .html("<a href=\"https://example.test/reset?t=abc\">Reset</a>")
            .build()
            .expect("a message");
        let wire = rendered(&message);

        assert!(wire.contains(
            "Content-Type: multipart/alternative; boundary=\"=_brocanth_test_boundary\"\r\n"
        ));
        assert_eq!(wire.matches("--=_brocanth_test_boundary\r\n").count(), 2);
        assert!(wire.ends_with("--=_brocanth_test_boundary--\r\n"));
        // Plain text first, HTML second: RFC 2046 §5.1.4 puts the least
        // faithful alternative first, and the client shows the last one it can
        // render.
        let text_at = wire.find("text/plain").expect("a plain part");
        let html_at = wire.find("text/html").expect("an HTML part");
        assert!(text_at < html_at);
        assert_eq!(
            bodies(&wire),
            vec![
                "Open https://example.test/reset?t=abc".to_string(),
                "<a href=\"https://example.test/reset?t=abc\">Reset</a>".to_string(),
            ]
        );
    }

    #[test]
    fn a_non_ascii_body_survives_the_round_trip() {
        let body = "Bonjour Café — 日本語 🫖\r\nhttps://example.test/reset";
        let message = Message::builder(from())
            .to(to())
            .subject("Réinitialisation")
            .text(body)
            .build()
            .expect("a message");
        let wire = rendered(&message);
        assert!(
            wire.is_ascii(),
            "a non-ASCII byte reached the wire, which a relay may rewrite"
        );
        assert_eq!(bodies(&wire), vec![body.to_string()]);
    }

    #[test]
    fn no_rendered_line_is_longer_than_the_limit() {
        // 998 octets is RFC 5322's hard limit. Past it, a relay rewraps the
        // message for us, and a rewrapped base64 body is a corrupt body.
        let message = Message::builder(from())
            .to_all((0..40).map(|n| {
                Address::named(
                    &format!("Recipient Number {n}"),
                    &format!("person-{n}@example.test"),
                )
                .expect("an address")
            }))
            .subject("Une très longue ligne d'objet ".repeat(8))
            .text("x".repeat(5000))
            .build()
            .expect("a message");
        for line in rendered(&message).split("\r\n") {
            assert!(line.len() <= 998, "a line of {} octets", line.len());
        }
    }

    #[test]
    fn every_recipient_field_reaches_the_header_block() {
        let message = every_field();
        let wire = rendered(&message);
        assert_eq!(count_headers(&wire, "From:"), 1);
        assert_eq!(count_headers(&wire, "To:"), 1);
        assert_eq!(count_headers(&wire, "Cc:"), 1);
        assert_eq!(count_headers(&wire, "Reply-To:"), 1);
        assert!(wire.contains("second@example.test"));
        assert!(wire.contains("copied@example.test"));
        assert!(wire.contains("support@example.test"));
    }

    #[test]
    fn the_envelope_is_to_then_cc_and_never_the_header_block() {
        let message = every_field();
        let envelope: Vec<&str> = message.recipients().iter().map(|a| a.address()).collect();
        assert_eq!(
            envelope,
            vec![
                "person@example.test",
                "second@example.test",
                "copied@example.test"
            ]
        );
    }
}

mod memory_backend {
    use super::*;

    #[test]
    fn a_message_with_every_field_round_trips() {
        let message = every_field();
        let mailer = Memory::new();
        assert!(mailer.is_empty());

        block_on(mailer.send(&message)).expect("the memory mailer accepts everything");

        assert_eq!(mailer.len(), 1);
        let sent = mailer.last().expect("one message");
        assert_eq!(sent.message, message);
        assert_eq!(sent.message.from().address(), "no-reply@example.test");
        assert_eq!(sent.message.from().display_name(), Some("Example"));
        assert_eq!(sent.message.to().len(), 2);
        assert_eq!(sent.message.cc().len(), 1);
        assert_eq!(sent.message.reply_to().len(), 1);
        assert_eq!(sent.message.subject().as_str(), "Reset your password");
        assert_eq!(sent.message.text(), "Open https://example.test/reset?t=abc");
        assert_eq!(
            sent.message.html(),
            Some("<a href=\"https://example.test/reset?t=abc\">Reset</a>")
        );
        assert!(sent.headers().contains("Subject: Reset your password"));
    }

    #[test]
    fn it_answers_what_went_to_one_address() {
        let mailer = Memory::new();
        block_on(mailer.send(&every_field())).expect("sent");
        block_on(
            mailer.send(
                &Message::builder(from())
                    .to(Address::parse("other@example.test").expect("an address"))
                    .subject("Something else")
                    .text("body")
                    .build()
                    .expect("a message"),
            ),
        )
        .expect("sent");

        assert_eq!(mailer.to("person@example.test").len(), 1);
        // Cc counts: a test asking "did this reach them" means the envelope.
        assert_eq!(mailer.to("copied@example.test").len(), 1);
        assert_eq!(mailer.to("nobody@example.test").len(), 0);
        assert_eq!(mailer.sent().len(), 2);

        mailer.clear();
        assert!(mailer.is_empty());
    }

    #[test]
    fn it_is_a_mailer_behind_an_arc() {
        // The shape every application holds, and the one `MailConfig` takes.
        let mailer: Arc<dyn Mailer> = Arc::new(Memory::new());
        block_on(mailer.send(&every_field())).expect("sent");
    }
}

mod the_plugin {
    use super::*;

    #[test]
    fn the_service_fills_in_the_sender() {
        let memory = Arc::new(Memory::new());
        let mail = Mail::new(
            memory.clone(),
            Address::named("Example", "no-reply@example.test").expect("a sender"),
        );

        let message = mail
            .message()
            .to(to())
            .subject("Reset your password")
            .text("Open https://example.test/reset?t=abc")
            .build()
            .expect("a message");
        block_on(mail.send(&message)).expect("sent");

        let sent = memory.last().expect("one message");
        assert_eq!(sent.message.from().address(), "no-reply@example.test");
        assert_eq!(sent.message.from().display_name(), Some("Example"));
    }

    #[test]
    fn a_clone_shares_one_backend() {
        let memory = Arc::new(Memory::new());
        let mail = Mail::new(memory.clone(), from());
        let second = mail.clone();

        block_on(mail.send(&every_field())).expect("sent");
        block_on(second.send(&every_field())).expect("sent");
        assert_eq!(memory.len(), 2);
    }
}

mod building {
    use super::*;

    #[test]
    fn a_message_with_no_recipient_is_refused() {
        let built = Message::builder(from())
            .subject("Hello")
            .text("body")
            .build();
        assert!(matches!(built, Err(MailError::NoRecipient)));
    }

    #[test]
    fn a_cc_alone_is_a_recipient() {
        assert!(Message::builder(from())
            .cc(to())
            .subject("Hello")
            .text("body")
            .build()
            .is_ok());
    }

    #[test]
    fn the_first_refusal_is_the_one_reported() {
        let built = Message::builder(from())
            .to(to())
            .subject("first\r\nBcc: a@example.test")
            .subject("second\r\nBcc: b@example.test")
            .build();
        assert!(matches!(
            built,
            Err(MailError::InvalidHeader {
                field: "subject",
                ..
            })
        ));
    }

    #[test]
    fn an_error_names_no_text_the_caller_supplied() {
        // An error that quotes the refused value writes attacker-chosen bytes
        // into a log line, which is the same injection one layer down.
        let secret = "harvest@attacker.test";
        let refused =
            Address::parse(&format!("person@example.test\r\nBcc: {secret}")).expect_err("refused");
        assert!(!refused.to_string().contains(secret));
        let refused = Subject::parse(&format!("Hello\r\nBcc: {secret}")).expect_err("refused");
        assert!(!refused.to_string().contains(secret));
    }
}

#[cfg(feature = "serde")]
mod on_the_wire {
    use super::*;

    #[test]
    fn a_message_round_trips_through_json() {
        let message = every_field();
        let json = serde_json::to_string(&message).expect("serialised");
        let back: Message = serde_json::from_str(&json).expect("deserialised");
        assert_eq!(back, message);
    }

    #[test]
    fn a_row_edited_in_the_queue_is_refused_on_the_way_out() {
        let json = r#"{
            "from": {"address": "no-reply@example.test"},
            "to": [{"display": "Example\r\nBcc: harvest@attacker.test",
                    "address": "person@example.test"}],
            "subject": "Hello",
            "text": "body"
        }"#;
        assert!(serde_json::from_str::<Message>(json).is_err());
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A message with every field filled in.
fn every_field() -> Message {
    Message::builder(Address::named("Example", "no-reply@example.test").expect("a sender"))
        .to(to())
        .to(Address::parse("second@example.test").expect("an address"))
        .cc(Address::named("Copied", "copied@example.test").expect("an address"))
        .reply_to(Address::named("Support", "support@example.test").expect("an address"))
        .subject("Reset your password")
        .text("Open https://example.test/reset?t=abc")
        .html("<a href=\"https://example.test/reset?t=abc\">Reset</a>")
        .build()
        .expect("a message")
}

/// Renders with a fixed clock and a fixed boundary, so a test can assert on
/// the whole message rather than on the parts of it that happen to be
/// deterministic.
fn rendered(message: &Message) -> String {
    String::from_utf8(message.render_at(1_793_094_063, "=_brocanth_test_boundary"))
        .expect("the renderer writes ASCII headers and base64 bodies")
}

/// Every header in the block whose line starts with `name`.
///
/// Counts starts of lines, so a `Bcc:` smuggled into a subject is counted
/// where it would do harm and a `Bcc:` inside a folded value is not.
fn count_headers(wire: &str, name: &str) -> usize {
    let block = wire.split("\r\n\r\n").next().unwrap_or(wire);
    block
        .split("\r\n")
        .filter(|line| line.starts_with(name))
        .count()
}

/// Every body in the message, base64-decoded back to text.
fn bodies(wire: &str) -> Vec<String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let mut out = Vec::new();
    for part in wire
        .split("Content-Transfer-Encoding: base64\r\n\r\n")
        .skip(1)
    {
        let encoded: String = part
            .split("\r\n")
            .take_while(|line| !line.starts_with("--=_brocanth"))
            .collect();
        let decoded = STANDARD.decode(encoded.trim_end()).expect("base64");
        out.push(String::from_utf8(decoded).expect("utf-8"));
    }
    out
}
