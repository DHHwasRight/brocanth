//! The guarantee that is a claim about programs which do not exist.
//!
//! Every other test in this crate asserts that something *does* the right
//! thing. The chain in [`brocanth_auth::mfa`] claims something stronger — that
//! a sign-in which skipped the policy, a forged admission and a proof spent
//! twice are not programs anybody can write — and no ordinary test can show
//! that, because the code under test would have to compile in order to run.
//!
//! So these cases are compiled and their errors compared against the recorded
//! ones. A case that starts compiling is a hole that has opened.
//!
//! The `.stderr` files are rustc's own diagnostics and are therefore tied to
//! the toolchain `mise.toml` pins. After a toolchain bump, regenerate them with
//! `TRYBUILD=overwrite cargo test -p brocanth-auth --test compile_fail` and
//! read the diff: the error *codes* are the assertion, and the wording is not.

#[test]
fn the_bypasses_do_not_compile() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
