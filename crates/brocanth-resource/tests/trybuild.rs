//! PLAN.md §12's assertion, run by the compiler.
//!
//! > **And one more assertion in the same spirit as the dependency test: a
//! > build of `brocanth-resource` with no policy registered must not compile.**
//! > Authorization claimed in a README rots. Authorization asserted by the type
//! > checker does not.
//!
//! An assertion that something *fails* is only worth having if the failure
//! says what is wrong, so these are snapshots of the compiler's own output
//! rather than a bare "it did not build". `brocanth-command` set the precedent in
//! this workspace.
//!
//! The pass case matters as much as the three failures: it is the same
//! resource with a policy, so the failures are about the policy and not about
//! something else being wrong with the file.

#[test]
fn a_resource_without_a_policy_does_not_compile_and_the_error_says_so() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_resource.rs");
    t.compile_fail("tests/ui/fail_no_policy.rs");
    t.compile_fail("tests/ui/fail_not_a_policy.rs");
    t.compile_fail("tests/ui/fail_policy_for_another_entity.rs");
}
