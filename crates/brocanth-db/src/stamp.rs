//! The UTC stamp at the start of a migration's name: `YYYYMMDD_HHMMSS`.
//!
//! Every kind of migration in a brocanth project uses it: the SQL files of
//! [`migrate`](crate::migrate), the Rust files `brocanth-migrate` writes and
//! the ones `brocanth-rls` writes. One stamp, so the files of all three sort
//! the same way.
//!
//! Outside the `runtime` feature: it needs no driver, so a tool that only
//! writes files (`brocanth-migrate`) takes this crate with no default
//! features and links nothing else of it.

/// Seconds since 1970-01-01 UTC, now. 0 when the clock is before 1970.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The stamp for now.
pub fn now() -> String {
    at(now_secs())
}

/// The stamp for `secs` seconds since 1970-01-01 UTC.
pub fn at(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}_{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Whether `name` is a migration name: a lower-case ASCII letter, then
/// lower-case letters, digits and `_`.
pub fn is_snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_utc_and_sorts_by_time() {
        assert_eq!(at(0), "19700101_000000");
        // 2026-11-05 10:15:00 UTC.
        assert_eq!(at(1_793_873_700), "20261105_101500");
        // 2024-02-29T23:59:59Z, a leap day.
        assert_eq!(at(1_709_251_199), "20240229_235959");
    }

    #[test]
    fn a_name_is_snake_case() {
        assert!(is_snake_case("rename_title"));
        assert!(is_snake_case("add_posts2"));
        assert!(!is_snake_case("RenameTitle"));
        assert!(!is_snake_case("2fast"));
        assert!(!is_snake_case("rename-title"));
        assert!(!is_snake_case(""));
    }
}
