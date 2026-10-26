//! `Message` on the wire, and the trap that comes with it.
//!
//! # Why this exists
//!
//! A real application does not send mail inside the request that asked for it.
//! It writes the message to a queue and answers; `brocanth-jobs` runs it, with
//! retries, backoff and a dead letter. A job payload has to serialise, so a
//! [`Message`] that cannot serialise is a mailer that cannot be queued -- and
//! this crate must not make that impossible even though it must not depend on
//! the queue.
//!
//! So: a feature, off by default, that puts `Serialize` and `Deserialize` on
//! the three validated types. That is all the coupling a queue needs. Nothing
//! here names `brocanth-jobs`.
//!
//! # The trap
//!
//! `#[derive(Deserialize)]` on a validated newtype hands the attacker back the
//! constructor we took away. `Address` refuses a CR because `Address::parse`
//! refuses one; a derived `Deserialize` writes the field directly and refuses
//! nothing. A queue row is then a header-injection vector, and it is a worse
//! one than the form field was, because by then everybody believes the value
//! is already checked.
//!
//! Every type here therefore deserialises **through a wire struct and back
//! through the same validator the first construction used**. A row that was
//! edited in the database is refused on the way out of it, by the same code
//! that refused it on the way in.

use serde::{Deserialize, Serialize};

use crate::address::Address;
use crate::error::MailError;
use crate::message::{assemble, Message, Subject};

#[derive(Serialize, Deserialize)]
struct AddressWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display: Option<String>,
    address: String,
}

impl From<Address> for AddressWire {
    fn from(address: Address) -> Self {
        Self {
            display: address.display_name().map(str::to_string),
            address: address.address().to_string(),
        }
    }
}

impl TryFrom<AddressWire> for Address {
    type Error = MailError;

    fn try_from(wire: AddressWire) -> Result<Self, Self::Error> {
        match wire.display {
            Some(display) => Address::named(&display, &wire.address),
            None => Address::parse(&wire.address),
        }
    }
}

impl Serialize for Address {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        AddressWire::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AddressWire::deserialize(deserializer)?;
        Address::try_from(wire).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Subject {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Subject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Subject::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
struct MessageWire {
    from: Address,
    #[serde(default)]
    to: Vec<Address>,
    #[serde(default)]
    cc: Vec<Address>,
    #[serde(default)]
    reply_to: Vec<Address>,
    subject: Subject,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    html: Option<String>,
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        MessageWire {
            from: self.from().clone(),
            to: self.to().to_vec(),
            cc: self.cc().to_vec(),
            reply_to: self.reply_to().to_vec(),
            subject: self.subject().clone(),
            text: self.text().to_string(),
            html: self.html().map(str::to_string),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = MessageWire::deserialize(deserializer)?;
        // Through `assemble`, so the recipient count is checked here exactly
        // as it is at `MessageBuilder::build`. A row with no `to` and no `cc`
        // is refused rather than handed to a relay that answers 554.
        assemble(
            wire.from,
            wire.to,
            wire.cc,
            wire.reply_to,
            wire.subject,
            wire.text,
            wire.html,
        )
        .map_err(serde::de::Error::custom)
    }
}
