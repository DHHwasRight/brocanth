//! Multi-factor authentication, as a set of satisfied factors and a rule that
//! says which sets are enough.
//!
//! # Why a set and not a flag
//!
//! `mfa_enabled: bool` answers one question and the wrong one. The questions an
//! application actually asks are "what did this person just prove?", "what
//! could they prove?" and "is that enough for *this*?" — and those are a set, a
//! set, and a predicate over the two. Once authentication is a set:
//!
//! * adding a factor kind is adding a variant, not rewriting the sign-in flow;
//! * "password **or** passkey, then TOTP **or** a recovery code" is a rule you
//!   can write down and test rather than a nest of `if`s;
//! * a session can record *which* factors proved it
//!   ([`SessionRecord::factors`]), so an audit log and a step-up check read the
//!   same field;
//! * and a factor nobody has implemented yet — [`FactorKind::Passkey`] — is
//!   already carried by the type, so the day WebAuthn lands it changes a
//!   policy and not a protocol.
//!
//! [`SessionRecord::factors`]: crate::storage::SessionRecord::factors
//!
//! # The three classes, and the rule shipped
//!
//! Every kind belongs to one [`FactorClass`]:
//!
//! * **[`FactorClass::Primary`]** — proves control of the account's main
//!   credential: [`FactorKind::Password`], [`FactorKind::Provider`].
//! * **[`FactorClass::Second`]** — proves possession of something the holder
//!   registered: [`FactorKind::Totp`], [`FactorKind::RecoveryCode`],
//!   [`FactorKind::EmailOtp`].
//! * **[`FactorClass::Standalone`]** — proves both at once and is bound to the
//!   origin, so phishing does not transport it: [`FactorKind::Passkey`].
//!
//! The sufficiency rule for a sign-in, in one sentence: **a standalone factor
//! alone, or a primary factor plus — when the account has a second factor to
//! give — one second factor.** [`FactorPolicy::for_sign_in`] is that sentence.
//!
//! # And its relationship to [`Privilege`]
//!
//! A sign-in grants [`Privilege::Authenticated`] and never
//! [`Privilege::Elevated`], however many factors were presented. Elevation is
//! not "how hard was the front door"; it is "was this identity proved
//! *recently*", which is what makes it the right gate for changing a password
//! or deleting an account. So it is granted by
//! [`FactorPolicy::for_elevation`] against a **fresh** proof, on a session that
//! already exists, by rotating it — and it decays, because
//! [`Sessions::rotate`] resets `created_at` and
//! [`SessionRecord::elevation_is_fresh`] measures from there.
//!
//! A recovery code does not elevate, by default. It is the credential a user
//! keeps on paper precisely because they have lost the other one, and somebody
//! who found that paper should not be two clicks from owning the account. The
//! user with no phone still has password reset by mail.
//! [`FactorPolicy::allowing_recovery_codes_to_elevate`] exists for an
//! application that has weighed that differently.
//!
//! [`Privilege`]: crate::session::Privilege
//! [`Privilege::Authenticated`]: crate::session::Privilege::Authenticated
//! [`Privilege::Elevated`]: crate::session::Privilege::Elevated
//! [`Sessions::rotate`]: crate::session::Sessions::rotate
//! [`SessionRecord::elevation_is_fresh`]: crate::storage::SessionRecord::elevation_is_fresh
//!
//! # The one thing this cannot hide
//!
//! An attacker who already has a correct password learns whether the account
//! has a second factor, because the next screen asks for one. There is no fix:
//! the user has to be told what to type. It is stated here rather than left for
//! somebody to discover, and the exposure needs a correct primary factor — an
//! unauthenticated caller learns nothing, because nothing on this type is
//! reachable before one.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::identity::AccountId;
use crate::impersonation::AccountHolder;
use crate::recovery::{self, RecoveryCodes};
use crate::session::Privilege;
use crate::storage::{MfaStore, TotpSecretRecord};
use crate::totp::{Totp, TotpSecret};

/// How long an elevation lasts before it has to be re-proved, in seconds.
pub const DEFAULT_ELEVATION_TTL: i64 = 15 * 60;

/// What a factor proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FactorClass {
    /// Control of the account's main credential.
    Primary,
    /// Possession of something the account holder registered.
    Second,
    /// Both at once, bound to the origin.
    Standalone,
}

/// One kind of proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum FactorKind {
    /// A password verified by [`crate::password::Passwords`].
    Password,
    /// A sign-in through a linked provider.
    Provider,
    /// A TOTP code. See [`crate::totp`].
    Totp,
    /// A recovery code. See [`crate::recovery`].
    RecoveryCode,
    /// A one-time code mailed to the account's address. See [`crate::otp`].
    EmailOtp,
    /// A WebAuthn credential.
    ///
    /// **Nothing in this crate produces one yet.** The kind exists so that a
    /// [`FactorSet`], a stored session and a [`FactorPolicy`] written today
    /// still mean what they say when WebAuthn lands, and so that an
    /// application's own passkey implementation has a name to put in the set.
    /// It is classed [`FactorClass::Standalone`], which is the whole reason
    /// passkeys are worth having: a user-verifying credential proves possession
    /// and the user in one gesture, and it cannot be replayed at a domain the
    /// browser did not bind it to.
    Passkey,
}

impl FactorKind {
    /// Every kind, in a stable order.
    pub const ALL: [FactorKind; 6] = [
        FactorKind::Password,
        FactorKind::Provider,
        FactorKind::Totp,
        FactorKind::RecoveryCode,
        FactorKind::EmailOtp,
        FactorKind::Passkey,
    ];

    /// What it proves.
    pub fn class(self) -> FactorClass {
        match self {
            FactorKind::Password | FactorKind::Provider => FactorClass::Primary,
            FactorKind::Totp | FactorKind::RecoveryCode | FactorKind::EmailOtp => {
                FactorClass::Second
            }
            FactorKind::Passkey => FactorClass::Standalone,
        }
    }

    /// A stable string, for a store writing a column and a log reading one.
    pub fn as_str(self) -> &'static str {
        match self {
            FactorKind::Password => "password",
            FactorKind::Provider => "provider",
            FactorKind::Totp => "totp",
            FactorKind::RecoveryCode => "recovery_code",
            FactorKind::EmailOtp => "email_otp",
            FactorKind::Passkey => "passkey",
        }
    }

    /// Reads back what [`FactorKind::as_str`] wrote.
    pub fn parse(text: &str) -> Option<Self> {
        FactorKind::ALL.into_iter().find(|k| k.as_str() == text)
    }

    const fn bit(self) -> u16 {
        1 << match self {
            FactorKind::Password => 0,
            FactorKind::Provider => 1,
            FactorKind::Totp => 2,
            FactorKind::RecoveryCode => 3,
            FactorKind::EmailOtp => 4,
            FactorKind::Passkey => 5,
        }
    }
}

impl std::fmt::Display for FactorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of factor kinds.
///
/// It serialises as a list of [`FactorKind::as_str`] names rather than as a
/// number, because a session row that reads `["password","totp"]` is one an
/// operator can audit and a `5` is not.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "Vec<String>", try_from = "Vec<String>")]
pub struct FactorSet(u16);

impl FactorSet {
    /// Nothing proved. What a pre-login session and an impersonated session
    /// both carry.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// A set from a list.
    pub fn of(kinds: impl IntoIterator<Item = FactorKind>) -> Self {
        kinds.into_iter().fold(Self::empty(), |set, k| set.with(k))
    }

    /// This set plus one kind.
    #[must_use]
    pub const fn with(self, kind: FactorKind) -> Self {
        Self(self.0 | kind.bit())
    }

    /// This set minus one kind.
    #[must_use]
    pub const fn without(self, kind: FactorKind) -> Self {
        Self(self.0 & !kind.bit())
    }

    /// Adds a kind in place.
    pub fn insert(&mut self, kind: FactorKind) {
        self.0 |= kind.bit();
    }

    /// Whether the kind is in the set.
    pub const fn contains(self, kind: FactorKind) -> bool {
        self.0 & kind.bit() != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many kinds.
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// Everything in both sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Everything in this set and in the other.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Everything in this set that is not in the other.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Whether every kind here is also there. This is the sufficiency test a
    /// hand-written rule would use.
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    /// Only the kinds of one class.
    #[must_use]
    pub fn of_class(self, class: FactorClass) -> Self {
        self.iter().filter(|k| k.class() == class).collect()
    }

    /// Whether any kind of this class is present.
    pub fn any_of_class(self, class: FactorClass) -> bool {
        !self.of_class(class).is_empty()
    }

    /// The kinds, in [`FactorKind::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = FactorKind> {
        FactorKind::ALL
            .into_iter()
            .filter(move |k| self.contains(*k))
    }
}

impl FromIterator<FactorKind> for FactorSet {
    fn from_iter<T: IntoIterator<Item = FactorKind>>(iter: T) -> Self {
        Self::of(iter)
    }
}

impl std::fmt::Debug for FactorSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl From<FactorSet> for Vec<String> {
    fn from(set: FactorSet) -> Self {
        set.iter().map(|k| k.as_str().to_string()).collect()
    }
}

impl TryFrom<Vec<String>> for FactorSet {
    type Error = String;

    fn try_from(names: Vec<String>) -> Result<Self, Self::Error> {
        let mut set = FactorSet::empty();
        for name in names {
            set.insert(
                FactorKind::parse(&name).ok_or_else(|| format!("`{name}` is not a factor kind"))?,
            );
        }
        Ok(set)
    }
}

/// Whether a set of satisfied factors is enough.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sufficiency {
    /// Enough. Open a session at this privilege.
    Sufficient(Privilege),
    /// A primary factor landed but the account owes a second one. Any single
    /// kind in this set completes the sign-in; ask for one of them.
    Incomplete(FactorSet),
    /// The policy demands a second factor and this account has none to give.
    /// Send the person to enrolment; do not open a session.
    EnrolmentRequired,
    /// Nothing presented counts. There is no primary factor in the set.
    Insufficient,
}

/// When a second factor is demanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SecondFactorRequirement {
    /// Only from accounts that enrolled one. The default, and the only one that
    /// lets a product add MFA without locking out everybody who has not set it
    /// up yet.
    #[default]
    WhenEnrolled,
    /// From every account, so an account with none cannot sign in at all until
    /// it enrols. Correct for an operator console; wrong for a consumer
    /// product, where it is a support queue rather than a security control.
    Always,
}

/// Which sets of factors are enough for what.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactorPolicy {
    second_factor: SecondFactorRequirement,
    recovery_code_elevates: bool,
    elevation_ttl: i64,
}

impl FactorPolicy {
    /// A second factor from the accounts that have one, recovery codes that do
    /// not elevate, and elevation that lasts fifteen minutes.
    pub const fn recommended() -> Self {
        Self {
            second_factor: SecondFactorRequirement::WhenEnrolled,
            recovery_code_elevates: false,
            elevation_ttl: DEFAULT_ELEVATION_TTL,
        }
    }

    /// Demands a second factor from every account.
    #[must_use]
    pub const fn requiring_a_second_factor_always(mut self) -> Self {
        self.second_factor = SecondFactorRequirement::Always;
        self
    }

    /// Lets a recovery code reach [`Privilege::Elevated`].
    ///
    /// Read the module documentation first. The default is off because a
    /// recovery code is the credential most likely to be on paper in a drawer,
    /// and elevation is what guards changing the password.
    #[must_use]
    pub const fn allowing_recovery_codes_to_elevate(mut self) -> Self {
        self.recovery_code_elevates = true;
        self
    }

    /// Sets how long an elevation stays fresh, in seconds.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] outside 60..=86400.
    pub fn with_elevation_ttl(mut self, seconds: i64) -> Result<Self, AuthError> {
        if !(60..=86_400).contains(&seconds) {
            return Err(AuthError::misconfigured(format!(
                "an elevation lasts between 60 and 86400 seconds, not {seconds}"
            )));
        }
        self.elevation_ttl = seconds;
        Ok(self)
    }

    /// How long an elevation stays fresh, in seconds.
    pub fn elevation_ttl(&self) -> i64 {
        self.elevation_ttl
    }

    /// When a second factor is demanded.
    pub fn second_factor(&self) -> SecondFactorRequirement {
        self.second_factor
    }

    /// Whether `satisfied` is enough to open a session.
    ///
    /// `enrolled` is what the account *could* prove —
    /// [`Mfa::second_factors`] computes it. Only its
    /// [`FactorClass::Second`] and [`FactorClass::Standalone`] members are
    /// read, so the two queries that method makes are all this needs.
    pub fn for_sign_in(&self, enrolled: &FactorSet, satisfied: &FactorSet) -> Sufficiency {
        if satisfied.any_of_class(FactorClass::Standalone) {
            return Sufficiency::Sufficient(Privilege::Authenticated);
        }
        if !satisfied.any_of_class(FactorClass::Primary) {
            return Sufficiency::Insufficient;
        }
        let owes_a_second = match self.second_factor {
            SecondFactorRequirement::Always => true,
            SecondFactorRequirement::WhenEnrolled => {
                enrolled.any_of_class(FactorClass::Second)
                    || enrolled.any_of_class(FactorClass::Standalone)
            }
        };
        if !owes_a_second || satisfied.any_of_class(FactorClass::Second) {
            return Sufficiency::Sufficient(Privilege::Authenticated);
        }
        let candidates = enrolled
            .of_class(FactorClass::Second)
            .union(enrolled.of_class(FactorClass::Standalone))
            .difference(*satisfied);
        if candidates.is_empty() {
            Sufficiency::EnrolmentRequired
        } else {
            Sufficiency::Incomplete(candidates)
        }
    }

    /// Whether a **fresh** proof is enough to reach [`Privilege::Elevated`].
    ///
    /// `satisfied` here is what was proved *just now*, in the step-up, not what
    /// opened the session. Rotate the session to
    /// [`Privilege::Elevated`] when this answers true; do not elevate on the
    /// strength of the original sign-in, which may be eight hours old.
    pub fn for_elevation(&self, enrolled: &FactorSet, satisfied: &FactorSet) -> bool {
        if satisfied.any_of_class(FactorClass::Standalone) {
            return true;
        }
        let mut second = satisfied.of_class(FactorClass::Second);
        if !self.recovery_code_elevates {
            second = second.without(FactorKind::RecoveryCode);
        }
        if !second.is_empty() {
            return true;
        }
        // An account with nothing but a password cannot produce a second
        // factor, so re-typing the password is the strongest proof available
        // and refusing it would mean nobody can ever change their password.
        if !(enrolled.any_of_class(FactorClass::Second)
            || enrolled.any_of_class(FactorClass::Standalone))
        {
            return satisfied.any_of_class(FactorClass::Primary);
        }
        false
    }
}

impl Default for FactorPolicy {
    fn default() -> Self {
        Self::recommended()
    }
}

/// A TOTP enrolment that has been started but not yet proved.
///
/// The secret is stored unconfirmed, so a user who scans the QR code and then
/// closes the tab has not locked themselves out of anything. Only
/// [`Mfa::confirm_totp_enrolment`] makes it a factor.
#[derive(Clone, Debug)]
pub struct TotpEnrolment {
    /// The secret, so the page can render the code by hand for somebody who
    /// cannot scan.
    pub secret: TotpSecret,
    /// The `otpauth:` URI to put in the QR code.
    pub uri: String,
}

/// Second factors: enrolment, verification and recovery codes.
pub struct Mfa<S: MfaStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    totp: Totp,
    policy: FactorPolicy,
    recovery_code_count: usize,
    /// A secret nobody holds, verified against when the account has no TOTP
    /// enrolled, so that "no second factor" costs what "wrong code" costs.
    /// [`crate::password::Passwords`] keeps a dummy hash for the same reason.
    dummy: TotpSecret,
}

impl<S: MfaStore, C: Clock> Mfa<S, C> {
    /// Builds the service: RFC 6238's interoperable parameters, the
    /// recommended policy, ten recovery codes.
    ///
    /// # Errors
    /// Fails when the OS has no randomness for the dummy secret.
    pub fn new(store: Arc<S>, clock: Arc<C>) -> Result<Self, AuthError> {
        Ok(Self {
            store,
            clock,
            totp: Totp::interoperable(),
            policy: FactorPolicy::recommended(),
            recovery_code_count: recovery::CODES_PER_SET,
            dummy: TotpSecret::generate()?,
        })
    }

    /// Replaces the TOTP parameters. Read [`Totp::custom`] first.
    #[must_use]
    pub fn with_totp(mut self, totp: Totp) -> Self {
        self.totp = totp;
        self
    }

    /// Replaces the policy.
    #[must_use]
    pub fn with_policy(mut self, policy: FactorPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Changes how many recovery codes a set holds.
    ///
    /// # Errors
    /// See [`RecoveryCodes::generate_count`].
    pub fn with_recovery_code_count(mut self, count: usize) -> Result<Self, AuthError> {
        RecoveryCodes::generate_count(count)?;
        self.recovery_code_count = count;
        Ok(self)
    }

    /// The policy in force.
    pub fn policy(&self) -> &FactorPolicy {
        &self.policy
    }

    /// The TOTP parameters in force.
    pub fn totp(&self) -> Totp {
        self.totp
    }

    /// The second factors this account could present: two queries, no secrets.
    ///
    /// This is the `enrolled` argument [`FactorPolicy::for_sign_in`] wants.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn second_factors(&self, account: &AccountId) -> Result<FactorSet, AuthError> {
        let mut set = FactorSet::empty();
        if self
            .store
            .totp_secret(account)
            .await?
            .is_some_and(|r| r.confirmed_at.is_some())
        {
            set.insert(FactorKind::Totp);
        }
        if self.store.count_recovery_codes(account).await? > 0 {
            set.insert(FactorKind::RecoveryCode);
        }
        Ok(set)
    }

    /// Everything the account has, including its primary credentials, for a
    /// settings page. Four queries.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn enrolled(&self, account: &AccountId) -> Result<FactorSet, AuthError> {
        let mut set = self.second_factors(account).await?;
        if self
            .store
            .account(account)
            .await?
            .is_some_and(|a| a.password_hash.is_some())
        {
            set.insert(FactorKind::Password);
        }
        if !self.store.links_for_account(account).await?.is_empty() {
            set.insert(FactorKind::Provider);
        }
        Ok(set)
    }

    /// How many recovery codes are left.
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn recovery_codes_remaining(&self, account: &AccountId) -> Result<u32, AuthError> {
        Ok(self.store.count_recovery_codes(account).await?)
    }

    /// Starts a TOTP enrolment: a fresh secret, stored unconfirmed.
    ///
    /// Calling it again before confirmation replaces the pending secret, which
    /// is what a user who abandoned the first QR code expects.
    ///
    /// # Errors
    /// Returns [`AuthError::Impersonated`] through [`AccountHolder`] — an
    /// operator cannot enrol a factor on somebody else's account — and fails
    /// when randomness or the store fails.
    pub async fn begin_totp_enrolment(
        &self,
        who: &AccountHolder,
        issuer: &str,
        label: &str,
    ) -> Result<TotpEnrolment, AuthError> {
        let secret = TotpSecret::generate()?;
        self.store
            .put_totp_secret(
                who.account(),
                TotpSecretRecord {
                    secret: secret.as_bytes().to_vec(),
                    created_at: self.clock.now(),
                    confirmed_at: None,
                    last_step: 0,
                },
            )
            .await?;
        let uri = secret.otpauth_uri(&self.totp, issuer, label);
        Ok(TotpEnrolment { secret, uri })
    }

    /// Proves the pending enrolment and returns the recovery codes.
    ///
    /// The codes are returned **once**, here, and the store keeps only their
    /// digests. There is no call that reads them back, because there is no
    /// copy to read.
    ///
    /// Confirming also spends the step the code came from, so the code the user
    /// just typed into the enrolment form cannot immediately be replayed into
    /// the sign-in form.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] for a wrong code,
    /// [`AuthError::Misconfigured`] when no enrolment was started, and
    /// [`AuthError::Impersonated`] through [`AccountHolder`].
    pub async fn confirm_totp_enrolment(
        &self,
        who: &AccountHolder,
        presented: &str,
    ) -> Result<RecoveryCodes, AuthError> {
        let account = who.account();
        let record = self.store.totp_secret(account).await?.ok_or_else(|| {
            AuthError::misconfigured("there is no TOTP enrolment to confirm on this account")
        })?;
        let secret = TotpSecret::from_bytes(record.secret);
        let step = self
            .totp
            .matching_step(&secret, presented, self.clock.now())
            .ok_or(AuthError::InvalidCredentials)?;
        if !self.store.claim_totp_step(account, step).await? {
            return Err(AuthError::InvalidCredentials);
        }
        self.store
            .confirm_totp_secret(account, self.clock.now())
            .await?;

        let codes = RecoveryCodes::generate_count(self.recovery_code_count)?;
        self.store
            .replace_recovery_codes(account, codes.digests())
            .await?;
        Ok(codes)
    }

    /// Removes TOTP, and the recovery codes with it.
    ///
    /// The codes go too. They exist to recover a second factor; left behind
    /// with no second factor to recover they are simply a second password, on
    /// paper, that the user believes they have deleted.
    ///
    /// # Errors
    /// Returns [`AuthError::Impersonated`] through [`AccountHolder`], and fails
    /// when the store fails.
    pub async fn disable_totp(&self, who: &AccountHolder) -> Result<(), AuthError> {
        let account = who.account();
        self.store.delete_totp_secret(account).await?;
        self.store
            .replace_recovery_codes(account, Vec::new())
            .await?;
        Ok(())
    }

    /// Replaces the recovery codes and returns the new set, once.
    ///
    /// # Errors
    /// Returns [`AuthError::Impersonated`] through [`AccountHolder`], and fails
    /// when randomness or the store fails.
    pub async fn regenerate_recovery_codes(
        &self,
        who: &AccountHolder,
    ) -> Result<RecoveryCodes, AuthError> {
        let codes = RecoveryCodes::generate_count(self.recovery_code_count)?;
        self.store
            .replace_recovery_codes(who.account(), codes.digests())
            .await?;
        Ok(codes)
    }

    /// Checks a TOTP code and spends its step.
    ///
    /// Two things happen and both are necessary. The code is matched across the
    /// skew window in constant time, and the step it matched is claimed from
    /// the store, which refuses any step it has already seen. Without the
    /// second half a correct code works for the rest of its thirty seconds, so
    /// anybody who read it over a shoulder, off a phishing page or out of a log
    /// can use it again.
    ///
    /// An account with no confirmed TOTP still pays a full window of HMAC
    /// evaluations, against a secret nobody holds, and then answers the same
    /// error.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] for a wrong code, a replayed
    /// code, a code outside the window, and an account with no TOTP. One
    /// variant, four causes.
    pub async fn verify_totp(&self, account: &AccountId, presented: &str) -> Result<(), AuthError> {
        let record = self.store.totp_secret(account).await?;
        let enrolled = record
            .as_ref()
            .filter(|r| r.confirmed_at.is_some())
            .map(|r| TotpSecret::from_bytes(r.secret.clone()));
        let secret = enrolled.as_ref().unwrap_or(&self.dummy);

        let step = self.totp.matching_step(secret, presented, self.clock.now());
        let (Some(step), true) = (step, enrolled.is_some()) else {
            return Err(AuthError::InvalidCredentials);
        };
        // The claim is the replay defence and it is atomic in the store: two
        // requests carrying the same code cannot both win it.
        if !self.store.claim_totp_step(account, step).await? {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(())
    }

    /// Spends a recovery code and answers how many are left.
    ///
    /// Deleting the row *is* the check, so a code cannot be spent twice and two
    /// concurrent uses of one code cannot both succeed.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] for a code that is wrong or
    /// already spent.
    pub async fn verify_recovery_code(
        &self,
        account: &AccountId,
        presented: &str,
    ) -> Result<u32, AuthError> {
        let digest = recovery::digest(presented);
        if !self.store.consume_recovery_code(account, &digest).await? {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(self.store.count_recovery_codes(account).await?)
    }

    /// Whether an elevation granted at `since` is still fresh.
    pub fn elevation_is_fresh(&self, since: UnixTime) -> bool {
        self.clock.now() - since < self.policy.elevation_ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_round_trips_through_its_names() {
        let set = FactorSet::of([FactorKind::Password, FactorKind::Totp]);
        let names: Vec<String> = set.into();
        assert_eq!(names, vec!["password", "totp"]);
        assert_eq!(FactorSet::try_from(names).unwrap(), set);
        assert!(FactorSet::try_from(vec!["sms".to_string()]).is_err());
    }

    #[test]
    fn the_set_algebra_is_the_sufficiency_test() {
        let satisfied = FactorSet::of([FactorKind::Password, FactorKind::Totp]);
        assert!(FactorSet::of([FactorKind::Password]).is_subset_of(satisfied));
        assert!(!FactorSet::of([FactorKind::Passkey]).is_subset_of(satisfied));
        assert_eq!(satisfied.len(), 2);
        assert_eq!(
            satisfied.of_class(FactorClass::Second),
            FactorSet::of([FactorKind::Totp])
        );
    }
}
