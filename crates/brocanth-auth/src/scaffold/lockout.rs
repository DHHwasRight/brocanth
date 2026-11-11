//! `auth-lockout` -- an account refuses password sign-in for minutes after
//! too many wrong passwords.
//!
//! The runtime is [`crate::lockout`]. This writes one function that builds
//! a [`Lockout`](crate::lockout::Lockout) with the policy in
//! `src/modules/auth_lockout.rs`, and a test that the policy is valid.
//!
//! # Decisions
//!
//! * **The policy is three options, not settings.** It is the same in every
//!   environment of one application, and a project reads it at compile
//!   time. A setting is for a value that two deployments of one image want
//!   different.
//! * **A test checks the policy, not the boot.** `LockoutPolicy::new`
//!   refuses a policy that locks out typos or lasts for days. The generated
//!   test calls it with the options, so `cargo test` fails before a deploy
//!   does.
//! * **The sign-in page is `auth-screens`' job.** With both, that module
//!   writes its sign-in through this function and its reset lifts the lock.
//!   Without `auth-screens`, a project calls `lockout(&auth)` from its own
//!   sign-in handler.

use brocanth_scaffold::snippet::module_ref;
use brocanth_scaffold::{Category, Manifest, Module, Opt, Project, Scaffold, Slot};

/// Account lockout, as a choice inside `auth`.
pub struct AuthLockout;

impl Module for AuthLockout {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth-lockout",
            "locks an account for minutes after too many wrong passwords",
        )
        .category(Category::Enhancer)
        .part_of_but_off("auth")
        .because("it counts wrong passwords in the store the `auth` plugin opens")
        .asked_in("doors", "Lock an account for a while after wrong passwords")
        .after("auth")
        .option(Opt::unsigned(
            "max_failures",
            10,
            "Wrong passwords that lock an account, 3 or more.",
        ))
        .option(Opt::unsigned(
            "window_secs",
            900,
            "The seconds in which those wrong passwords count, 60 to 86400.",
        ))
        .option(Opt::unsigned(
            "lock_for_secs",
            900,
            "How long a lock lasts, in seconds, 60 to 86400.",
        ))
        .absent(
            "auth-lockout",
            "no lockout; only a rate limit slows a guessing run",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/auth_lockout.rs",
            LOCKOUT_RS
                .replace("@MAX@", &module_ref("auth-lockout", "max_failures"))
                .replace("@WINDOW@", &module_ref("auth-lockout", "window_secs"))
                .replace("@LOCK_FOR@", &module_ref("auth-lockout", "lock_for_secs")),
        );
        s.at(Slot::PluginsMod).line("pub mod auth_lockout;");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

const LOCKOUT_RS: &str = r#"//! Account lockout: after too many wrong passwords, an account refuses
//! password sign-in for a while. The policy is in
//! `src/modules/auth_lockout.rs`.
//!
//! Sign in through `lockout(&auth)?.login(auth.accounts(), email, password)`
//! and render `Attempt::Refused` and `Attempt::JustLocked` alike: a page
//! that says "locked" tells a stranger the address has an account. On
//! `JustLocked`, mail the owner. After a password reset, call `clear`.

use brocanth_auth::{AuthError, Lockout, LockoutPolicy};

use crate::plugins::auth::{Clock, Services, Store};

/// The policy in `src/modules/auth_lockout.rs`.
///
/// # Errors
/// [`AuthError::Misconfigured`] when an option is out of its range.
pub fn policy() -> Result<LockoutPolicy, AuthError> {
    let max = @MAX@;
    let window = @WINDOW@;
    let lock_for = @LOCK_FOR@;
    LockoutPolicy::new(
        u32::try_from(max).unwrap_or(u32::MAX),
        i64::try_from(window).unwrap_or(i64::MAX),
        i64::try_from(lock_for).unwrap_or(i64::MAX),
    )
}

/// Password sign-in that counts failures, over the auth store.
///
/// # Errors
/// [`AuthError::Misconfigured`] when an option is out of its range.
pub fn lockout(auth: &Services) -> Result<Lockout<Store, Clock>, AuthError> {
    Ok(auth.lockout(policy()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_policy_in_the_module_file_is_valid() {
        if let Err(e) = super::policy() {
            panic!("{e}");
        }
    }
}
"#;

/// What the generated `AGENTS.md` says about the lockout.
const AGENTS: &str = "\n\
    ## Account lockout\n\
    \n\
    `src/plugins/auth_lockout.rs` builds the lockout with the policy in\n\
    `src/modules/auth_lockout.rs`. Sign in through `lockout(&auth)?.login(..)`,\n\
    not `auth.accounts().login(..)`, or wrong passwords are not counted.\n\
    Render `Attempt::Refused` and `Attempt::JustLocked` with the same words\n\
    and status; mail the owner on `JustLocked`. Call `clear` after a password\n\
    reset.\n";
