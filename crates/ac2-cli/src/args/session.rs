//! `ac2 session …`: the saved sessions of the measurements and traces.

use std::path::PathBuf;

use clap::Subcommand;

/// `session …`.
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// The working session: name, unsaved changes, what could be recovered, the autosave.
    Status,
    /// Start a new session: no measurements or traces. Refused while there are unsaved
    /// changes, unless --discard (what it replaces stays recoverable: `session recover`).
    New {
        /// Replace unsaved changes.
        #[arg(long)]
        discard: bool,
    },
    /// Save measurements and traces (with slots and display edits) to the session they
    /// belong to (`session status`). One loaded from or exported to a file on a client's
    /// machine is written with `save-as PATH`: the daemon knows that file by name only.
    Save,
    /// Save measurements and traces as another session; they then belong to it.
    SaveAs {
        /// Session name (in the daemon's session directory), a directory path on the
        /// daemon's host, or an .ac2session file on this machine.
        session: String,
        /// Replace an existing session of that name.
        #[arg(long)]
        force: bool,
    },
    /// Open a saved session: replaces measurements and traces; always comes up disarmed.
    /// Refused while there are unsaved changes, unless --discard.
    Load {
        /// Session name, a directory path on the daemon's host, or an .ac2session file (or
        /// session folder, for a daemon on another host) on this machine.
        session: String,
        /// Replace unsaved changes (they stay recoverable: `session recover`).
        #[arg(long)]
        discard: bool,
    },
    /// Bring back the measurements and traces the last `new` or `load` replaced while
    /// they had unsaved changes.
    Recover,
    /// Bring a session that cannot be opened (another format version) into the current
    /// measurements and traces. From a folder or .ac2session file on this machine: its
    /// measurements with their settings, colours, layouts and delays, its traces under
    /// their measurements, and the running ones started where the audio I/O captures
    /// their inputs, with a summary of what was restored, skipped or not read as saved.
    /// From a name or a path of the daemon's host: its traces only.
    Salvage {
        /// A session folder or .ac2session file here, or as for `load`.
        session: String,
    },
    /// Write the measurements and traces (or a saved session) to an .ac2session file on
    /// this machine.
    Export {
        /// File, or a folder for the suggested file name.
        file: PathBuf,
        /// A saved session instead of the current measurements and traces.
        #[arg(long)]
        session: Option<String>,
    },
    /// Store an .ac2session file or session folder from this machine on the daemon as a
    /// saved session, without opening it.
    Import {
        /// File or folder.
        file: PathBuf,
        /// Session name; the file's name without its extension when omitted.
        #[arg(long)]
        name: Option<String>,
        /// Replace an existing session of that name.
        #[arg(long)]
        force: bool,
    },
    /// Saved sessions in the daemon's session directory.
    List,
    /// Delete a saved session from the daemon's session directory, by name. The open
    /// session is not deleted (save it under another name or start a new one first).
    Delete {
        /// Session name.
        name: String,
    },
    /// Rename a saved session; never replaces another. The open session keeps belonging
    /// to it under the new name.
    Rename {
        /// Session name.
        name: String,
        /// New name.
        to: String,
    },
}
