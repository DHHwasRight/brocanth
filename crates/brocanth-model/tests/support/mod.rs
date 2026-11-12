//! What every database test file here shares.

/// Unwraps an optional harness, or announces the skip and returns.
///
/// `$made` is a future of `Result<Option<Harness>, DbErr>`, as
/// `Harness::isolated()` returns. `None` means that no variable names a
/// server: the test says so on stderr and returns, so `cargo test` passes on
/// a laptop with no Postgres.
macro_rules! or_skip {
    ($made:expr) => {
        match $made.await.expect("connecting to Postgres") {
            Some(harness) => harness,
            None => {
                brocanth_model::testing::skipped(
                    "no Postgres server; set BROCANTH_MODEL_TEST_POSTGRES or \
                     BROCANTH_TEST_DATABASE_URL to run this",
                );
                return;
            }
        }
    };
}
