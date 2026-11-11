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
    /// The site name on every row the server counts, and part of the
    /// visitor hash. One application is one site; a static site that sends
    /// beacons is a site named by its origin's host.
    pub site: String,
    /// Paths never counted, each with everything under it: `/admin`
    /// covers `/admin/users` and not `/administrators`. Empty by default:
    /// this crate does not know where an application mounts its admin. A
    /// generated project adds the admin's path with
    /// [`exclude_path`](Self::exclude_path).
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
    /// The path the collector answers at, for page views a static site
    /// sends with the `beacon` snippet.
    pub collector_path: String,
    /// The origins allowed to send to the collector:
    /// `https://docs.example.com`. Empty, the default, installs no
    /// collector at all.
    pub collector_origins: Vec<String>,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            site: "app".to_string(),
            exclude: Vec::new(),
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
            collector_path: "/_a/v".to_string(),
            collector_origins: Vec::new(),
        }
    }
}

impl AnalyticsConfig {
    /// Adds a prefix to [`exclude`](Self::exclude).
    ///
    /// For a path another module owns, read from its module file:
    /// `.exclude_path(crate::modules::admin::MODULE.mount_at)`. A generated
    /// project calls it after the file and the flags were read, so a file
    /// that sets `exclude` cannot count the admin, and a moved admin needs
    /// no edit here.
    #[must_use]
    pub fn exclude_path(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        if !self.exclude.contains(&prefix) {
            self.exclude.push(prefix);
        }
        self
    }

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
        let config = AnalyticsConfig::default().exclude_path("/admin");
        assert!(config.excludes("/admin"));
        assert!(config.excludes("/admin/users/3"));
        assert!(!config.excludes("/administrators"));
        assert!(!config.excludes("/"));
    }

    #[test]
    fn the_default_excludes_nothing_and_an_added_path_is_kept_once() {
        assert!(!AnalyticsConfig::default().excludes("/admin"));
        let config = AnalyticsConfig {
            exclude: vec!["/drafts".to_string()],
            ..AnalyticsConfig::default()
        }
        .exclude_path("/backoffice")
        .exclude_path("/backoffice");
        assert_eq!(config.exclude, ["/drafts", "/backoffice"]);
        assert!(config.excludes("/backoffice/users/3"));
    }
}
