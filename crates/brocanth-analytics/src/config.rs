//! [`AnalyticsConfig`]: what to count, what to keep, and for how long.

/// The analytics settings: `[analytics]` in a config file.
///
/// Every field has a default, so an application that configures nothing
/// counts page views of its own site and keeps them 25 months.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct AnalyticsConfig {
    /// Count page views. `false` installs no hook at all.
    pub enabled: bool,
    /// The site name on every row, and part of the visitor hash. One
    /// application is one site.
    pub site: String,
    /// Paths never counted, each with everything under it: `/admin`
    /// covers `/admin/users` and not `/administrators`.
    pub exclude: Vec<String>,
    /// Query parameters kept on the stored path. Every other parameter is
    /// dropped before the row is built, so a token or an email in a URL
    /// never reaches the database.
    pub query_allow: Vec<String>,
    /// Days a row is kept. Older rows are deleted once a day. 0 keeps
    /// them forever.
    pub retention_days: u32,
    /// Seconds between two writes of the buffer.
    pub flush_interval: u64,
    /// Skip a request that sends `Sec-GPC: 1` or `DNT: 1`.
    ///
    /// Off by default: nothing stored can identify the visitor, so there
    /// is nothing for the signal to opt out of, and skipping undercounts.
    /// On, for a site that wants to honour the signal anyway.
    pub respect_gpc: bool,
    /// A header the edge sets to the client address, such as
    /// `fly-client-ip`. Unset, the socket's address is used.
    pub ip_header: Option<String>,
    /// A header the edge sets to a two-letter country code, such as
    /// `cf-ipcountry`. Unset, no country is stored: there is no GeoIP
    /// database here.
    pub country_header: Option<String>,
    /// Create the two tables on the first readiness check. Off, for a
    /// project that keeps `PgStore::SCHEMA` in a reviewed migration.
    pub migrate: bool,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            site: "app".to_string(),
            exclude: vec!["/admin".to_string()],
            query_allow: [
                "utm_source",
                "utm_medium",
                "utm_campaign",
                "utm_term",
                "utm_content",
            ]
            .map(String::from)
            .to_vec(),
            retention_days: 760,
            flush_interval: 10,
            respect_gpc: false,
            ip_header: None,
            country_header: None,
            migrate: true,
        }
    }
}

impl AnalyticsConfig {
    /// Whether `path` is excluded: equal to an entry, or under it.
    pub fn excludes(&self, path: &str) -> bool {
        self.exclude.iter().any(|prefix| {
            let prefix = prefix.trim_end_matches('/');
            if prefix.is_empty() {
                return true;
            }
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_excluded_prefix_covers_its_subtree_and_nothing_else() {
        let config = AnalyticsConfig::default();
        assert!(config.excludes("/admin"));
        assert!(config.excludes("/admin/users/3"));
        assert!(!config.excludes("/administrators"));
        assert!(!config.excludes("/"));
    }
}
