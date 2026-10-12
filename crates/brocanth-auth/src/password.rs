//! Local passwords: argon2id, and no choice is offered.
//!
//! # Why there is no algorithm setting
//!
//! Every library that made the hash function configurable has shipped a
//! deployment running the weakest option in its list, because the list was
//! read once, at the start of a project, by somebody choosing a default. The
//! only thing a configurable hash buys is the ability to configure it wrong.
//! So [`Passwords`] hashes with argon2id and the type system offers no second
//! choice — there is no `Algorithm` parameter anywhere in this crate's public
//! API.
//!
//! argon2id rather than argon2i or argon2d: argon2id is the hybrid the RFC
//! recommends for password hashing, because argon2i alone is weaker against
//! time-memory trade-offs and argon2d alone is vulnerable to side channels on
//! a machine running other people's code.
//!
//! # What *is* configurable, and why that is different
//!
//! Cost. [`PasswordCost`] has two constructors and they are named, not
//! numeric: [`PasswordCost::recommended`] is OWASP's
//! 19 MiB / 2 iterations / 1 lane, and [`PasswordCost::insecure_for_tests`]
//! says in its own name what it is for. Cost has to be tunable because it is
//! the one parameter that depends on the machine, and a test suite that waited
//! 100 ms per hash would simply stop hashing in its tests.
//!
//! # Why verification takes an `Option`
//!
//! [`Passwords::verify_or_dummy`] is the whole of this crate's timing defence
//! for login. An address nobody has registered must cost the same as an
//! address that exists with the wrong password, and the only way to achieve
//! that is to run argon2 anyway — against a hash of a value nobody knows. See
//! [`crate::accounts`].

use argon2::password_hash::phc::PasswordHash;
use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};

use crate::error::AuthError;

/// How expensive a hash is.
///
/// Not *which* hash. There is only one of those.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswordCost {
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}

impl PasswordCost {
    /// OWASP's current argon2id recommendation: 19 MiB, 2 iterations, 1 lane.
    pub const fn recommended() -> Self {
        Self {
            memory_kib: 19 * 1024,
            iterations: 2,
            lanes: 1,
        }
    }

    /// The cheapest parameters argon2 accepts.
    ///
    /// Named so that it cannot appear in a production configuration without
    /// the reviewer reading the word `insecure`. A hash produced with these is
    /// still a valid PHC string and still verifies, which is the point: the
    /// test suite exercises the real code path at a cost a test can afford.
    pub const fn insecure_for_tests() -> Self {
        Self {
            memory_kib: 8,
            iterations: 1,
            lanes: 1,
        }
    }

    /// Custom cost, for a deployment that has measured its own machine.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] when argon2 rejects the
    /// combination.
    pub fn custom(memory_kib: u32, iterations: u32, lanes: u32) -> Result<Self, AuthError> {
        let cost = Self {
            memory_kib,
            iterations,
            lanes,
        };
        cost.params()?;
        Ok(cost)
    }

    fn params(self) -> Result<Params, AuthError> {
        Params::new(self.memory_kib, self.iterations, self.lanes, None)
            .map_err(|e| AuthError::misconfigured(format!("argon2 rejected these costs: {e}")))
    }
}

impl Default for PasswordCost {
    fn default() -> Self {
        Self::recommended()
    }
}

/// Hashes and verifies passwords.
///
/// Holds a precomputed hash of an unguessable value, used by
/// [`Passwords::verify_or_dummy`] when there is no account to verify against.
pub struct Passwords {
    argon2: Argon2<'static>,
    dummy: String,
}

impl Passwords {
    /// Builds a hasher at the given cost.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] for impossible costs, and
    /// [`AuthError::Crypto`] if the dummy hash cannot be computed.
    pub fn new(cost: PasswordCost) -> Result<Self, AuthError> {
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, cost.params()?);
        // The dummy is hashed from fresh randomness at startup rather than
        // from a constant: a constant would make the dummy hash identical in
        // every deployment, and an attacker who recognises it in a timing side
        // channel learns that the account does not exist — which is exactly
        // what the dummy was there to hide.
        let filler = crate::random::random_secret()?;
        let dummy = hash_with(&argon2, &filler)?;
        Ok(Self { argon2, dummy })
    }

    /// A hasher at [`PasswordCost::recommended`].
    ///
    /// # Errors
    /// See [`Passwords::new`].
    pub fn recommended() -> Result<Self, AuthError> {
        Self::new(PasswordCost::recommended())
    }

    /// Hashes a password into a PHC string.
    ///
    /// # Errors
    /// Returns [`AuthError::Crypto`] when argon2 fails.
    pub fn hash(&self, password: &str) -> Result<String, AuthError> {
        hash_with(&self.argon2, password)
    }

    /// Checks a password against a PHC string.
    ///
    /// The comparison inside is constant time: `password-hash` compares the
    /// two digests through `subtle::ConstantTimeEq`, not `==`.
    pub fn verify(&self, phc: &str, password: &str) -> bool {
        match PasswordHash::new(phc) {
            Ok(parsed) => self
                .argon2
                .verify_password(password.as_bytes(), &parsed)
                .is_ok(),
            // A stored hash we cannot parse is a corrupted row, not a correct
            // password.
            Err(_) => false,
        }
    }

    /// Checks a password against a hash that may not exist.
    ///
    /// When `phc` is `None` — no such account, or an account that only signs
    /// in through a provider — this still runs one full argon2 verification,
    /// against [`Passwords`]'s own dummy hash, and then answers `false`. The
    /// caller therefore spends the same time on a known address with a wrong
    /// password and on an address nobody has registered, which is the half of
    /// enumeration resistance that a careful response body cannot provide.
    pub fn verify_or_dummy(&self, phc: Option<&str>, password: &str) -> bool {
        match phc {
            Some(h) => self.verify(h, password),
            None => {
                let _ = self.verify(&self.dummy, password);
                false
            }
        }
    }
}

impl std::fmt::Debug for Passwords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passwords(argon2id)")
    }
}

fn hash_with(argon2: &Argon2<'static>, password: &str) -> Result<String, AuthError> {
    argon2
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| AuthError::crypto(format!("argon2 could not hash: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_is_a_phc_argon2id_string_and_round_trips() {
        let p = Passwords::new(PasswordCost::insecure_for_tests()).unwrap();
        let h = p.hash("correct horse battery staple").unwrap();
        assert!(h.starts_with("$argon2id$"), "{h}");
        assert!(p.verify(&h, "correct horse battery staple"));
        assert!(!p.verify(&h, "correct horse battery stapl"));
    }

    #[test]
    fn two_hashes_of_one_password_differ_because_the_salt_does() {
        let p = Passwords::new(PasswordCost::insecure_for_tests()).unwrap();
        assert_ne!(p.hash("hunter2").unwrap(), p.hash("hunter2").unwrap());
    }

    #[test]
    fn a_corrupt_stored_hash_is_not_a_correct_password() {
        let p = Passwords::new(PasswordCost::insecure_for_tests()).unwrap();
        assert!(!p.verify("not a phc string", "anything"));
    }
}
