//! "Did you mean?", for every name the application can misspell.
//!
//! A misspelt name is the common failure, and a silent one is the worst
//! possible answer to it. The core needs this for a middleware name that no
//! plugin offers; `brocanth-config` needs the same courtesy for a configuration
//! section that no plugin claims. So it lives here, in the core, and the plugin
//! borrows it — the dependency arrow points from the plugin to the core, as it
//! always does.

/// The closest known name, when it is close enough to be a plausible typo.
///
/// The budget grows with the length of the name, so a short name does not
/// attract a wild suggestion. Returns `None` when nothing is close, because a
/// misleading suggestion is worse than none.
///
/// ```
/// use brocanth::suggest::nearest_name;
///
/// assert_eq!(
///     nearest_name("loggr", &["logger", "server"]),
///     Some("logger".to_string())
/// );
/// assert_eq!(nearest_name("telemetry", &["logger"]), None);
/// ```
pub fn nearest_name<S: AsRef<str>>(name: &str, known: &[S]) -> Option<String> {
    let budget = (name.chars().count() / 3).max(2);
    let mut sorted: Vec<&str> = known.iter().map(|s| s.as_ref()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .into_iter()
        .map(|k| (levenshtein(name, k), k))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, k)| (*d, *k))
        .map(|(_, k)| k.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_wrong_letter_is_within_budget() {
        assert_eq!(
            nearest_name("logger.request_logs", &["logger.request_log"]),
            Some("logger.request_log".to_string())
        );
    }

    #[test]
    fn a_wildly_wrong_name_gets_no_suggestion() {
        assert_eq!(nearest_name("telemetry", &["logger", "server"]), None);
    }

    #[test]
    fn ties_break_on_the_alphabet_so_the_message_is_stable() {
        // "ab" is one edit from both; the lower name wins, every time.
        assert_eq!(nearest_name("ab", &["zb", "bb"]), Some("bb".to_string()));
    }
}
