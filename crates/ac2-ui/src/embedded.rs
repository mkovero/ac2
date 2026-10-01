//! Embedded daemon: the desktop app hosting `ac2d` in-process (PLAN §4.1) so a laptop needs
//! no separate daemon. The UI talks to it through the same client and protocol as to any
//! other daemon; only the endpoints differ.

use std::fmt;

use ac2_client::Endpoints;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedError {
    /// This build has no daemon to embed.
    Unavailable,
}

impl fmt::Display for EmbeddedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddedError::Unavailable => {
                f.write_str("this build has no embedded daemon; start ac2d separately")
            }
        }
    }
}

impl std::error::Error for EmbeddedError {}

/// Starts the embedded daemon and returns where it listens.
pub fn start_embedded() -> Result<Endpoints, EmbeddedError> {
    Err(EmbeddedError::Unavailable)
}
