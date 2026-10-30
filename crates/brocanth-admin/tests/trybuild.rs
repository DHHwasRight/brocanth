//! PLAN.md §12's assertion, for the admin, run by the compiler.
//!
//! > **A build of `brocanth-resource` with no policy registered must not
//! > compile.** Authorization claimed in a README rots. Authorization
//! > asserted by the type checker does not.
//!
//! The admin is the other generated CRUD path, so it makes the same claim:
//! `AdminResource::Policy` has no default. These are snapshots of the
//! compiler's own output, because a failure is only worth having if it says
//! what is missing. The pass case is the same resource with its policy, so
//! the failures are about the policy and nothing else.

#[test]
fn an_admin_resource_without_a_policy_does_not_compile_and_the_error_says_so() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_resource.rs");
    t.compile_fail("tests/ui/fail_no_policy.rs");
    t.compile_fail("tests/ui/fail_policy_for_another_entity.rs");
}
