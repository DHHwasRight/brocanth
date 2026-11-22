// The pinned Postgres release and the SHA-256 of its archive per target.
//
// Read twice: by `src/embedded.rs` (`include!`), which checks every archive
// before it extracts it, and by `build.rs`, which checks a bundled archive
// before it embeds it. One file, so the two cannot drift.
//
// BUMP BOTH TOGETHER. A new version needs the eight new hashes. Take each
// one from the release's `postgresql-<version>-<target>.tar.gz.sha256`
// file (github.com/theseus-rs/postgresql-binaries/releases), and confirm it
// against your own download of the archive (`shasum -a 256 -c`). Never
// compute a hash from a download alone.
//
// 18.7.0: taken from the release and confirmed against a download of each
// archive, 2026-11-22.

/// The Postgres release the server runs. Pinned, so a start never asks
/// which release is the latest, and every machine runs the same server.
/// Bump it together with [`ARCHIVE_SHA256`].
pub const POSTGRES_VERSION: &str = "18.7.0";

/// The SHA-256 of `postgresql-<POSTGRES_VERSION>-<target>.tar.gz`, per
/// target, in lower-case hex. Bump it together with [`POSTGRES_VERSION`].
pub const ARCHIVE_SHA256: &[(&str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "86456c0ed99d2466524eaa074db6580cf7d748b91042e252c83eaca9a9d216e7",
    ),
    (
        "x86_64-apple-darwin",
        "9cd649259d6bf4af60a235e772c83853df215b63f4c1967c2eed7f9376aefb89",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "53ca01952cb581145e4fb16b6b3b675da345a82879f350707b9f65863a0642f4",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "6fcf45163aa044fe58597885b0e27c4b1614c53ad088d4d0ebbbfdf5df510eaf",
    ),
    (
        "x86_64-unknown-linux-musl",
        "d1aa6f1a7bd9b746897562e73e99c0e66a099038faeda69d60c3cb00c157faa1",
    ),
    (
        "aarch64-unknown-linux-musl",
        "5bb1730daec296f2162ab32f7f6ac418920a565ce9110675103c70d1f25d47a0",
    ),
    (
        "x86_64-pc-windows-msvc",
        "6e054adb854dbed1f78f9933a1c65737d85a47d2446ce0e9b5f055fb3794fe5c",
    ),
    (
        "aarch64-pc-windows-msvc",
        "6226f348f07bd6d9b9928aab53ce3c694fde769633ebd805488866949adc562f",
    ),
];

/// The pinned SHA-256 of the archive for `target`, or `None` for a target
/// with no archive.
pub fn pinned_sha256(target: &str) -> Option<&'static str> {
    ARCHIVE_SHA256
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, hash)| *hash)
}
