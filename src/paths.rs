// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the config files are, and where the daemon is allowed to write.
//!
//! The convention is the whole stack's, and this module only follows it: a hand-edited file
//! under `$XDG_CONFIG_HOME/wlrix/`, with `/etc/wlrix/` consulted when the user has none of
//! their own. See `wlrix-compositor/src/config.rs` and `wlrix-desktop/src/xdg.rs` -- the lookup
//! is duplicated in every component because the repos build standalone, and this is one more
//! copy rather than a new rule.
//!
//! Two things about that convention matter more here than anywhere else, because this is the
//! program that *writes*:
//!
//! - **The first file found wins outright; nothing is merged.** So creating a user file with
//!   one key in it, while a system file exists, silently discards every other system default.
//!   The write path seeds the user file from the system one first; see [`crate::edit`].
//! - **The daemon only ever writes the user path.** `/etc/wlrix` belongs to whoever installed
//!   the machine, and a session daemon has no business there even when it could.
//!
//! The two directories are resolved once, into [`Roots`], and passed down rather than looked up
//! again wherever they are needed. That is what lets the store and the watcher be tested
//! against scratch directories: `std::env::set_var` is racy in a test binary that runs its
//! tests on several threads, and a daemon whose riskiest code could only be exercised by
//! mutating the environment would not get tested properly.

use std::path::{Path, PathBuf};

/// Consulted when the user has no config of their own.
const SYSTEM_CONFIG_DIR: &str = "/etc";
/// The directory both the user's and the system's files sit in.
const SUBDIR: &str = "wlrix";

/// One config file the daemon knows about.
///
/// The variants are the namespaces of the D-Bus API: a key is `<namespace>.<toml path>`, and
/// the namespace is this file's stem. `wlrix-greeter` is deliberately absent -- its config is
/// greetd's, root-owned under `/etc/greetd/`, and not a session setting at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum File {
    Background,
    Compositor,
    Desktop,
    Files,
    Idle,
    Portal,
    Screenshot,
    Session,
    Tray,
}

/// Every file, in the order they are listed and dumped.
pub const ALL: &[File] = &[
    File::Background,
    File::Compositor,
    File::Desktop,
    File::Files,
    File::Idle,
    File::Portal,
    File::Screenshot,
    File::Session,
    File::Tray,
];

impl File {
    /// The namespace a key in this file is prefixed with, which is also the file's stem.
    pub fn namespace(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Compositor => "compositor",
            Self::Desktop => "desktop",
            Self::Files => "files",
            Self::Idle => "idle",
            Self::Portal => "portal",
            Self::Screenshot => "screenshot",
            Self::Session => "session",
            Self::Tray => "tray",
        }
    }

    /// The file's name on disk.
    pub fn file_name(self) -> String {
        format!("{}.toml", self.namespace())
    }

    /// The file this namespace names, if it is one.
    pub fn from_namespace(namespace: &str) -> Option<Self> {
        ALL.iter()
            .copied()
            .find(|file| file.namespace() == namespace)
    }
}

/// The two directories the daemon reads from, one of which it writes to.
#[derive(Debug, Clone)]
pub struct Roots {
    /// Where writes go. `None` when neither `$XDG_CONFIG_HOME` nor `$HOME` is set, which for a
    /// session daemon means something has gone wrong further up than this.
    user: Option<PathBuf>,
    system: PathBuf,
}

impl Roots {
    /// The real ones, from the environment.
    pub fn from_environment() -> Self {
        Self {
            user: user_config_dir().map(|dir| dir.join(SUBDIR)),
            system: Path::new(SYSTEM_CONFIG_DIR).join(SUBDIR),
        }
    }

    /// Made-up ones, for a test.
    #[cfg(test)]
    pub fn new(user: Option<PathBuf>, system: PathBuf) -> Self {
        Self { user, system }
    }

    /// The directory the daemon writes into, and watches.
    pub fn user_dir(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// The config directory itself, which is [`Roots::user_dir`]'s parent.
    ///
    /// Only a bridge wants this: everything else in the daemon writes inside `wlrix/`, and a
    /// bridge writes into somebody else's directory beside it -- `gtk-3.0/`, for one. See
    /// [`crate::bridge`].
    pub fn config_home(&self) -> Option<&Path> {
        self.user.as_deref().and_then(Path::parent)
    }

    /// The system directory, watched because a change there changes the effective value of
    /// every key the user has not overridden.
    pub fn system_dir(&self) -> &Path {
        &self.system
    }

    /// Where a write lands: the user's file, always.
    pub fn user_path(&self, file: File) -> Option<PathBuf> {
        self.user.as_ref().map(|dir| dir.join(file.file_name()))
    }

    /// The system fallback. Read from, never written.
    pub fn system_path(&self, file: File) -> PathBuf {
        self.system.join(file.file_name())
    }

    /// The file actually in force: the user's if it exists, else the system's if it does.
    ///
    /// `None` means neither exists, which is the ordinary case on a fresh install -- every
    /// component treats a missing config as "all defaults".
    pub fn effective_path(&self, file: File) -> Option<PathBuf> {
        if let Some(path) = self.user_path(file)
            && path.is_file()
        {
            return Some(path);
        }
        let system = self.system_path(file);
        system.is_file().then_some(system)
    }

    /// Whether the file in force is the user's own, which is what decides whether a value is
    /// reported as coming from `user` or from `system`.
    pub fn is_user_file(&self, file: File) -> bool {
        self.effective_path(file)
            .zip(self.user_path(file))
            .is_some_and(|(effective, user)| effective == user)
    }
}

/// `$XDG_CONFIG_HOME`, or `~/.config` as the spec says to assume.
fn user_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".config"))
}

/// Whether a filename in a watched directory is one of ours.
///
/// The watch is on the directory, not on individual files -- an atomic rename replaces the
/// inode and would kill a per-file watch on first save -- so an editor's swapfile lands in the
/// same events. This is what keeps `.compositor.toml.swp` from costing a rescan.
pub fn is_known_file(name: &str) -> bool {
    ALL.iter().any(|file| file.file_name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespace_is_the_files_stem() {
        for file in ALL {
            assert_eq!(file.file_name(), format!("{}.toml", file.namespace()));
            assert_eq!(File::from_namespace(file.namespace()), Some(*file));
        }
    }

    #[test]
    fn an_unknown_namespace_names_no_file() {
        // `greeter` is the one someone will actually try: it is a wlRIX component with a config
        // file, just not one of these. Answering `None` is what turns that into UnknownKey.
        for name in ["greeter", "", "compositor.toml", "Compositor"] {
            assert_eq!(File::from_namespace(name), None, "{name:?}");
        }
    }

    #[test]
    fn only_our_own_files_are_interesting() {
        assert!(is_known_file("compositor.toml"));
        assert!(is_known_file("session.toml"));
        // An editor's leavings, and the daemon's own temp file, sit in the same directory.
        for name in [
            ".compositor.toml.swp",
            ".compositor.toml.tmp",
            "compositor.toml~",
            "autostart.toml",
            "outputs.toml",
        ] {
            assert!(!is_known_file(name), "{name:?}");
        }
    }

    #[test]
    fn the_real_roots_are_the_ones_every_component_reads() {
        // Hand-kept in step with each component's own lookup; a change here that is not
        // mirrored there would have the daemon writing somewhere nothing reads.
        let roots = Roots::from_environment();
        assert_eq!(
            roots.system_path(File::Compositor),
            PathBuf::from("/etc/wlrix/compositor.toml")
        );
        assert!(
            roots
                .user_path(File::Compositor)
                .is_none_or(|path| path.ends_with(".config/wlrix/compositor.toml")
                    || path.ends_with("wlrix/compositor.toml"))
        );
    }

    #[test]
    fn the_user_file_wins_over_the_system_one() {
        let dir = std::env::temp_dir().join("wlrix-settings-paths-precedence");
        let _ = std::fs::remove_dir_all(&dir);
        let user = dir.join("config/wlrix");
        let system = dir.join("etc/wlrix");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let roots = Roots::new(Some(user.clone()), system.clone());

        // Neither exists.
        assert_eq!(roots.effective_path(File::Idle), None);
        assert!(!roots.is_user_file(File::Idle));

        // Only the system's.
        std::fs::write(system.join("idle.toml"), "").unwrap();
        assert_eq!(
            roots.effective_path(File::Idle),
            Some(system.join("idle.toml"))
        );
        assert!(!roots.is_user_file(File::Idle));

        // The user's shadows it -- outright, with no merging, which is why a first write has
        // to seed from the system file.
        std::fs::write(user.join("idle.toml"), "").unwrap();
        assert_eq!(
            roots.effective_path(File::Idle),
            Some(user.join("idle.toml"))
        );
        assert!(roots.is_user_file(File::Idle));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
