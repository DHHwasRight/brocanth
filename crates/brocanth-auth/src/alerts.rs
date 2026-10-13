//! Login alerts: "your account was signed into from somewhere new".
//!
//! The crate sends no mail. This returns a [`LoginAlert`] the way
//! [`RegistrationNotice`] and [`ResetOutcome`] return what to send, and the
//! application's mailer decides the rest.
//!
//! # "Somewhere new" has to mean something, so here is what it means
//!
//! **A network the account has not signed in from in the last ninety days.**
//! The network is the /24 of an IPv4 address or the /48 of an IPv6 one — the
//! coarsest unit that still moves when a person does.
//!
//! That is the whole definition. In particular it is **not** the device: the
//! user agent is deliberately not part of it. A browser's user agent changes
//! every few weeks on its own, and a signal that fires on a Chrome update
//! teaches people to delete the mail unread, which is worse than sending
//! nothing.
//!
//! # Its false positives, stated rather than discovered
//!
//! * **A phone on mobile data alerts constantly.** Carrier networks hand out
//!   addresses from pools far wider than a /24, so a commute can cross several.
//!   This is the big one.
//! * **A VPN alerts on every exit node**, and alerts again when the provider
//!   rotates them.
//! * **A dynamic residential address alerts after a router reboot**, when the
//!   ISP's lease lands in a different /24.
//! * And the false *negative* that matters: **an attacker inside the same
//!   office, campus or carrier NAT never alerts**, because they are in the same
//!   /24. A network is a weak proxy for a person. This control catches the
//!   credential-stuffing run from another continent, which is the common case,
//!   and it does not catch your colleague.
//!
//! # What is stored, which is as little as the definition needs
//!
//! One row per account per network, holding **HMAC-SHA256(key, network)** —
//! never the address, never the user agent, never a city. The hash is keyed,
//! with the same [`SigningKey`] the session cookie uses, because an unkeyed
//! digest of a /24 is 2^24 guesses and therefore not a hash at all: anybody who
//! reads the table recovers every network every user signs in from. With the
//! key held outside the database, the table is a set of opaque 32-byte values.
//!
//! Rows are evicted by [`LoginAlerts::purge`]. Nothing here grows without a
//! bound you set.
//!
//! # It is not an enumeration surface
//!
//! [`LoginAlerts::note`] is reachable only after a sign-in has already
//! succeeded, so there is no address to hide and nothing a caller can learn
//! that they did not already prove.
//!
//! [`RegistrationNotice`]: crate::accounts::RegistrationNotice
//! [`ResetOutcome`]: crate::accounts::ResetOutcome

use std::net::IpAddr;
use std::sync::Arc;

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::identity::AccountId;
use crate::session::SigningKey;
use crate::storage::SignInLocationStore;

/// How long a network stays familiar without being used again, in seconds.
pub const DEFAULT_FAMILIAR_FOR: i64 = 90 * 24 * 60 * 60;

/// What to send, if anything.
///
/// Unlike [`RegistrationNotice`], this one carries no secret and the response
/// may say whatever it likes: the person reading it has already authenticated
/// as the account.
///
/// [`RegistrationNotice`]: crate::accounts::RegistrationNotice
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginAlert {
    /// Seen before, recently enough. Send nothing.
    Familiar {
        /// When this network was last used.
        last_seen_at: UnixTime,
    },
    /// Send the notice.
    Unfamiliar {
        /// Whose account.
        account: AccountId,
        /// When the sign-in happened.
        at: UnixTime,
        /// Whether this is the account's first recorded sign-in anywhere.
        ///
        /// It is, by the definition, "somewhere new" — but the message should
        /// almost certainly be a welcome rather than a warning, and a user
        /// whose very first act produces a security alert is a user who learns
        /// the alerts are noise.
        first_ever: bool,
    },
    /// No usable client address, so no judgement was made and nothing is sent.
    ///
    /// This is what you get when the request arrived through a proxy you have
    /// not taught to pass the real client address. Silence is the right answer:
    /// treating "unknown" as "new" alerts on every single sign-in.
    NotJudged,
}

/// Decides whether a sign-in came from somewhere new.
pub struct LoginAlerts<S: SignInLocationStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    key: SigningKey,
    familiar_for: i64,
}

impl<S: SignInLocationStore, C: Clock> LoginAlerts<S, C> {
    /// Builds the service with a ninety-day memory.
    ///
    /// The key is the session signing key. One key, one place to rotate — and
    /// rotating it makes every network unfamiliar once, which produces one
    /// round of alerts and then settles.
    pub fn new(store: Arc<S>, clock: Arc<C>, key: SigningKey) -> Self {
        Self {
            store,
            clock,
            key,
            familiar_for: DEFAULT_FAMILIAR_FOR,
        }
    }

    /// Changes how long a network stays familiar, in seconds.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] outside one day to ten years.
    pub fn with_familiar_for(mut self, seconds: i64) -> Result<Self, AuthError> {
        if !(86_400..=10 * 365 * 86_400).contains(&seconds) {
            return Err(AuthError::misconfigured(format!(
                "a network stays familiar between 86400 seconds and ten years, not {seconds}"
            )));
        }
        self.familiar_for = seconds;
        Ok(self)
    }

    /// How long a network stays familiar, in seconds.
    pub fn familiar_for(&self) -> i64 {
        self.familiar_for
    }

    /// Records a sign-in and says whether to warn about it.
    ///
    /// `client_ip` is the address the request really came from. Behind a proxy
    /// that is the one your proxy told you about, not the socket's peer — and
    /// a value a client can set is a value an attacker sets to the victim's own
    /// network, which turns the alert off. Pass `None` rather than a guess.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn note(
        &self,
        account: &AccountId,
        client_ip: Option<&str>,
    ) -> Result<LoginAlert, AuthError> {
        let Some(fingerprint) = client_ip.and_then(|raw| self.fingerprint(raw)) else {
            return Ok(LoginAlert::NotJudged);
        };
        let now = self.clock.now();
        let previous = self
            .store
            .record_sign_in_location(account, &fingerprint, now)
            .await?;
        Ok(match previous {
            Some(last_seen_at) if now - last_seen_at < self.familiar_for => {
                LoginAlert::Familiar { last_seen_at }
            }
            other => LoginAlert::Unfamiliar {
                account: account.clone(),
                at: now,
                // Nothing at all for this network *and* nothing for the
                // account: the store answers per network, so "first ever" is
                // the honest reading of "no row and no other row", which only
                // the store can see. A row that has merely gone stale is not a
                // first sign-in.
                first_ever: other.is_none()
                    && self.store.count_sign_in_locations(account).await? <= 1,
            },
        })
    }

    /// Forgets networks nobody has used since `unseen_since`, and answers how
    /// many rows went.
    ///
    /// Run it from a sweeper. A sensible argument is `now - familiar_for`:
    /// anything older can never make a sign-in familiar again, so keeping it is
    /// keeping a record of where somebody was for no purpose.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn purge(&self, unseen_since: UnixTime) -> Result<u64, AuthError> {
        Ok(self.store.purge_sign_in_locations(unseen_since).await?)
    }

    /// The keyed digest of the network an address belongs to, or `None` when
    /// the address does not parse.
    ///
    /// Public so a test, or an application migrating an existing table, can
    /// compute the same value this module stores.
    pub fn fingerprint(&self, client_ip: &str) -> Option<[u8; 32]> {
        let parsed: IpAddr = client_ip.trim().parse().ok()?;
        let network: Vec<u8> = match parsed {
            // /24 and /48: the coarsest prefix that still moves when the
            // person does. Finer and a load balancer rotating its egress is a
            // new location; coarser and a whole hosting provider is one place.
            IpAddr::V4(v4) => v4.octets()[..3].to_vec(),
            IpAddr::V6(v6) => v6.octets()[..6].to_vec(),
        };
        let tag = self.key.sign_message(&network);
        let mut out = [0u8; 32];
        out.copy_from_slice(&tag);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alerts() -> LoginAlerts<crate::memory::MemoryStore, crate::clock::TestClock> {
        LoginAlerts::new(
            Arc::new(crate::memory::MemoryStore::new()),
            Arc::new(crate::clock::TestClock::default()),
            SigningKey::from_bytes([3u8; 32]),
        )
    }

    #[test]
    fn the_fingerprint_is_the_network_and_not_the_address() {
        let a = alerts();
        assert_eq!(
            a.fingerprint("203.0.113.7"),
            a.fingerprint("203.0.113.209"),
            "two addresses in one /24 are one place"
        );
        assert_ne!(a.fingerprint("203.0.113.7"), a.fingerprint("203.0.114.7"));
        assert_eq!(
            a.fingerprint("2001:db8:1234:5678::1"),
            a.fingerprint("2001:db8:1234:ffff::9"),
            "two addresses in one /48 are one place"
        );
        assert_ne!(
            a.fingerprint("2001:db8:1234::1"),
            a.fingerprint("2001:db8:9999::1")
        );
    }

    #[test]
    fn a_different_key_gives_a_different_digest() {
        let one = alerts();
        let two = LoginAlerts::new(
            Arc::new(crate::memory::MemoryStore::new()),
            Arc::new(crate::clock::TestClock::default()),
            SigningKey::from_bytes([4u8; 32]),
        );
        assert_ne!(
            one.fingerprint("203.0.113.7"),
            two.fingerprint("203.0.113.7")
        );
    }

    #[test]
    fn nonsense_is_not_a_location() {
        let a = alerts();
        for bad in ["", "localhost", "203.0.113", "not an address", "1.2.3.4.5"] {
            assert!(a.fingerprint(bad).is_none(), "accepted `{bad}`");
        }
    }
}
