//! Two backends for development: one writes each message to a folder, one
//! hands each message to a function.
//!
//! Mail in development should be looked at, and it must not leave the
//! machine. [`Memory`](crate::Memory) keeps a message where only a test can
//! see it. A local SMTP catcher works, but it is one more process to start.
//! These two need nothing:
//!
//! * [`Folder`] writes each message as an `.eml` file. A mail client opens
//!   it, and `grep` finds the reset link in it.
//! * [`Log`] calls a function with each message. The `mail` module points it
//!   at the application's log, so the link is on the terminal.
//!
//! Neither one delivers anything, and that is the point: a development
//! configuration copied to production sends no mail to real people. The
//! `mail` module warns at startup when the transport is not SMTP.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::error::MailError;
use crate::mailer::Mailer;
use crate::message::Message;

/// Writes each message to a file in a folder: `<millis>-<n>.eml`.
///
/// The file is the message as SMTP would carry it, so a mail client opens
/// it. The folder is made on the first send.
///
/// The write blocks the calling thread for the length of one small file.
/// That is acceptable in development, and this backend is for development
/// only.
#[derive(Debug)]
pub struct Folder {
    dir: PathBuf,
    /// Tells two messages in one millisecond apart.
    count: AtomicU64,
}

impl Folder {
    /// A backend that writes into `dir`. Nothing is created until the first
    /// send.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            count: AtomicU64::new(0),
        }
    }

    /// The folder the messages go to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[async_trait]
impl Mailer for Folder {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            MailError::transport(format!(
                "cannot make the mail folder {}: {e}\nhelp: set the folder to a path this \
                 process can write",
                self.dir.display()
            ))
        })?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let n = self.count.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(format!("{millis}-{n}.eml"));
        std::fs::write(&path, message.render()).map_err(|e| {
            MailError::transport(format!(
                "cannot write {}: {e}\nhelp: set the folder to a path this process can write",
                path.display()
            ))
        })
    }
}

/// The function a [`Log`] calls with each message.
type Sink = dyn Fn(&Message) + Send + Sync;

/// Calls a function with each message, and delivers nothing.
///
/// The function decides what to show. The `mail` module writes the
/// recipients, the subject and the text body to the application's log, so
/// a link in the body is on the terminal.
pub struct Log {
    sink: Box<Sink>,
}

impl Log {
    /// A backend that calls `sink` with each message.
    pub fn new(sink: impl Fn(&Message) + Send + Sync + 'static) -> Self {
        Self {
            sink: Box::new(sink),
        }
    }
}

impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log").finish_non_exhaustive()
    }
}

#[async_trait]
impl Mailer for Log {
    async fn send(&self, message: &Message) -> Result<(), MailError> {
        (self.sink)(message);
        Ok(())
    }
}
