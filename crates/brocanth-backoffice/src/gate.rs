//! Which binary this process is, and which paths it serves
//! (docs/backoffice.md §7.2, B2).

use std::net::SocketAddr;

/// Which binary an application serves as.
///
/// A value, and not process-wide state. `src/bin/backoffice.rs` builds its
/// application with `Role::Backoffice` in the services
/// (`app::serve_with`), and the generated `backoffice` plugin reads it in
/// `register` and builds the [`Gate`] from it. An application built
/// without one is [`Role::App`]. So two applications in one process (two
/// tests, say) can serve as two roles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// The application: every path, and the backoffice's too while it is
    /// embedded.
    #[default]
    App,
    /// The backoffice binary: the backoffice's mount and nothing else.
    Backoffice,
}

/// Which paths an application answers.
///
/// The backoffice binary answers its mount and the paths under it. The
/// application answers every path, except the mount when the backoffice
/// is not embedded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    role: Role,
    mount: String,
    embedded: bool,
}

impl Gate {
    /// The gate of an application in `role`, with the backoffice at `mount`.
    /// `embedded` is whether the application serves the mount too.
    pub fn new(role: Role, mount: &str, embedded: bool) -> Gate {
        Gate {
            role,
            mount: mount.trim_end_matches('/').to_string(),
            embedded,
        }
    }

    /// Whether this application answers `path`.
    pub fn admits(&self, path: &str) -> bool {
        match self.role {
            Role::Backoffice => self.under_mount(path),
            Role::App => self.embedded || !self.under_mount(path),
        }
    }

    /// The role this gate is for.
    pub fn role(&self) -> Role {
        self.role
    }

    fn under_mount(&self, path: &str) -> bool {
        let mount = self.mount.as_str();
        mount.is_empty()
            || path == mount
            || path
                .strip_prefix(mount)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// Checks the backoffice binary's listen address, at start.
///
/// # Errors
///
/// When `addr` is not a socket address, with a `help:` line.
pub fn check_addr(addr: &str) -> Result<SocketAddr, String> {
    addr.parse().map_err(|_| {
        format!(
            "`[backoffice] addr` is {addr:?}, which is not an address and a port\n\
             help: set it to an address and a port, such as 127.0.0.1:3001 or 0.0.0.0:3001"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoffice_binary_answers_its_mount_and_nothing_else() {
        let gate = Gate::new(Role::Backoffice, "/admin", true);
        assert!(gate.admits("/admin"));
        assert!(gate.admits("/admin/post/1"));
        assert!(!gate.admits("/"));
        assert!(!gate.admits("/about"));
        assert!(
            !gate.admits("/administrator"),
            "a prefix of a segment is another path"
        );
    }

    #[test]
    fn the_application_hands_the_mount_over_when_it_is_not_embedded() {
        let embedded = Gate::new(Role::App, "/admin/", true);
        assert!(embedded.admits("/admin/post"));
        assert!(embedded.admits("/about"));

        let split = Gate::new(Role::App, "/admin", false);
        assert!(!split.admits("/admin"));
        assert!(!split.admits("/admin/post"));
        assert!(split.admits("/administrator"));
        assert!(split.admits("/"));
    }

    #[test]
    fn a_bad_address_names_the_setting_and_the_fix() {
        assert!(check_addr("127.0.0.1:3001").is_ok());
        let error = check_addr("localhost").expect_err("no port");
        assert!(error.contains("[backoffice] addr"), "{error}");
        assert!(error.contains("help:"), "{error}");
    }
}
