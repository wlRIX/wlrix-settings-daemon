// SPDX-License-Identifier: GPL-3.0-or-later
//! Telling the owning program that its config changed.
//!
//! Writing the file is only half of applying a setting. The compositor and `wlrix-idle` each
//! drop their pid in a well-known file and re-read their config on `SIGHUP`; this sends it. The
//! logic is `wlrix-desktop/src/session.rs`'s, which does the same thing with `SIGTERM` to log
//! out -- one more hand-kept copy, because the repos build standalone and there is no shared
//! crate to put it in.
//!
//! ## Not every owner can be told
//!
//! - **`wlrix-compositor`**, **`wlrix-idle`**, **`wlrix-desktop`**, **`wlrix-bg`**: pidfile and
//!   `SIGHUP`. Works.
//! - **`xdg-desktop-portal-wlrix`**: has no reload *deliberately* -- its own `signals.rs` says
//!   a screen share is not something to reconfigure underneath. The right action would be
//!   `systemctl --user try-restart`, but only when no cast is live, and the daemon has no way
//!   to know that. So it reports and leaves the decision to the person.
//! - **`wlrix-session`**: reads its config before there is a session. Next login.
//!
//! What comes back is an [`Outcome`] per owner rather than a bare success, so a settings panel
//! can say "the compositor is not running; this applies at next login" instead of appearing to
//! have done nothing.
//!
//! ## A stale pidfile is the awkward case
//!
//! A crashed compositor leaves its pidfile behind, and pids are recycled. Signaling whatever
//! process has since taken that number would be considerably worse than doing nothing, so
//! `ESRCH` is reported as "not running" rather than trusted blindly -- and the pidfile's
//! contents are checked before they are believed.

use std::path::PathBuf;

use crate::schema::{Owner, Reload};

/// What became of a change, from the point of view of whoever has to see it take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The owner was signaled and re-read its config.
    Applied,
    /// Written, but the owner is not running. It will be read at the next start.
    NotRunning,
    /// Written, but the owner has to be restarted before it takes effect.
    RestartRequired,
    /// Written, and read at the next login.
    NextLogin,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NotRunning => "not-running",
            Self::RestartRequired => "restart-required",
            Self::NextLogin => "next-login",
        }
    }
}

/// Tell `owner` that a setting with this reload behavior changed.
///
/// Never fails: every way this can go wrong is a fact about the session rather than an error in
/// the request, and the write it follows has already succeeded. The value is on disk either
/// way, which is what the outcome says.
pub fn notify(owner: Owner, reload: Reload) -> Outcome {
    match reload {
        Reload::NextLogin => Outcome::NextLogin,
        Reload::None | Reload::Restart => Outcome::RestartRequired,
        Reload::Live => signal_owner(owner),
    }
}

fn signal_owner(owner: Owner) -> Outcome {
    let Some(name) = owner.pidfile() else {
        // The schema tests forbid this pairing, so reaching it means the table changed without
        // the code that reads it.
        tracing::warn!("{owner} is marked live but has no pidfile");
        return Outcome::RestartRequired;
    };
    reload_via_pidfile(&runtime_dir().join(name), owner)
}

/// Read a pidfile and `SIGHUP` what it names.
///
/// Split from [`signal_owner`] so the tests can point it at a scratch directory rather than
/// reaching into the environment -- `set_var` is racy in a test binary that runs its tests on
/// several threads, and this is not worth a mutex.
fn reload_via_pidfile(path: &std::path::Path, owner: Owner) -> Outcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("{owner} is not running (no {})", path.display());
            return Outcome::NotRunning;
        }
        Err(err) => {
            tracing::warn!("could not read {}: {err}", path.display());
            return Outcome::NotRunning;
        }
    };

    let Some(pid) = parse_pid(&text) else {
        tracing::warn!("{} does not contain a pid", path.display());
        return Outcome::NotRunning;
    };

    match signal(pid, libc::SIGHUP) {
        Ok(()) => {
            tracing::info!("told {owner} (pid {pid}) to reload");
            Outcome::Applied
        }
        Err(why) => {
            tracing::warn!("could not tell {owner} to reload: {why}");
            Outcome::NotRunning
        }
    }
}

/// `$XDG_RUNTIME_DIR`, else the temp directory -- the fallback every wlRIX pidfile uses.
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
}

/// The pid a pidfile's contents name, if it is one.
fn parse_pid(text: &str) -> Option<i32> {
    text.trim().parse::<i32>().ok().filter(|pid| *pid > 1)
}

/// Send `signal` to `pid`, telling a stale pidfile apart from a real failure.
fn signal(pid: i32, signal: i32) -> Result<(), String> {
    // SAFETY: `kill` with a positive pid and a valid signal number. It either signals that
    // process or reports why it could not; nothing here depends on the process existing.
    if unsafe { libc::kill(pid, signal) } == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // The pidfile outlived the program that wrote it.
        Some(libc::ESRCH) => Err(format!("no process {pid}; the pidfile is stale")),
        Some(libc::EPERM) => Err(format!("not allowed to signal process {pid}")),
        _ => Err(format!("could not signal process {pid}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pid parsing and signaling below are lifted from `wlrix-desktop/src/session.rs`,
    // tests included: they are the part where being wrong means signaling a stranger's
    // process.

    #[test]
    fn a_pidfile_is_a_bare_number() {
        assert_eq!(parse_pid("1234"), Some(1234));
        // Every wlRIX pidfile is written with a trailing newline.
        assert_eq!(parse_pid("1234\n"), Some(1234));
        assert_eq!(parse_pid("  1234  \n"), Some(1234));
    }

    #[test]
    fn nonsense_in_the_pidfile_is_not_a_pid() {
        for text in ["", "\n", "not a pid", "12x", "-1", "3.5"] {
            assert_eq!(parse_pid(text), None, "{text:?}");
        }
    }

    #[test]
    fn pid_one_and_zero_are_refused() {
        // 0 signals the whole process group and 1 is init; a pidfile naming either is corrupt,
        // and acting on it would be far worse than doing nothing.
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("1"), None);
    }

    #[test]
    fn signalling_a_process_that_is_gone_says_the_file_is_stale() {
        // A pid that cannot be running: the maximum is well below this on Linux.
        let why = signal(0x7fff_fffe, 0).expect_err("should fail");
        assert!(why.contains("stale"), "{why}");
    }

    #[test]
    fn signalling_ourselves_with_signal_zero_succeeds() {
        // Signal 0 checks for existence without delivering anything, so this proves the success
        // path without stopping the test run.
        let me = std::process::id() as i32;
        assert!(signal(me, 0).is_ok());
    }

    #[test]
    fn the_pidfiles_are_the_ones_the_owners_write() {
        // Kept in step with each component's own `pidfile.rs` by hand. A change there that is
        // not mirrored here would make every live setting silently stop applying.
        assert_eq!(Owner::Compositor.pidfile(), Some("wlrix-compositor.pid"));
        assert_eq!(Owner::Idle.pidfile(), Some("wlrix-idle.pid"));
        assert_eq!(Owner::Desktop.pidfile(), Some("wlrix-desktop.pid"));
        // Named for the binary, `wlrix-bg`, not for its config file's stem, `background`.
        assert_eq!(Owner::Background.pidfile(), Some("wlrix-bg.pid"));
        // The portal has none deliberately: its own signals.rs says a screen share is not
        // something to reconfigure underneath it, so there is nothing a signal could ask for.
        assert_eq!(Owner::Portal.pidfile(), None);
    }

    #[test]
    fn a_setting_nobody_can_be_told_about_says_so_without_looking() {
        // No pidfile is read and no signal is sent for these, so they answer the same whether
        // or not anything is running.
        assert_eq!(notify(Owner::None, Reload::NextLogin), Outcome::NextLogin);
        assert_eq!(
            notify(Owner::Portal, Reload::None),
            Outcome::RestartRequired
        );
        assert_eq!(
            notify(Owner::Idle, Reload::Restart),
            Outcome::RestartRequired
        );
    }

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wlrix-settings-apply-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make the scratch directory");
        dir
    }

    #[test]
    fn no_pidfile_at_all_means_not_running() {
        // The point of the outcome: the value is on disk and will be read at the next start,
        // and a panel should say that rather than reporting a failure.
        let dir = scratch("missing");
        assert_eq!(
            reload_via_pidfile(&dir.join("wlrix-compositor.pid"), Owner::Compositor),
            Outcome::NotRunning
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_pidfile_means_not_running_rather_than_signalling_a_stranger() {
        // pids are recycled. A crashed compositor's pidfile naming a number some unrelated
        // process has since been given is the case this is really guarding.
        let dir = scratch("stale");
        let path = dir.join("wlrix-compositor.pid");
        std::fs::write(&path, "2147483646\n").unwrap();
        assert_eq!(
            reload_via_pidfile(&path, Owner::Compositor),
            Outcome::NotRunning
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pidfile_full_of_nonsense_means_not_running() {
        let dir = scratch("nonsense");
        let path = dir.join("wlrix-idle.pid");
        for text in ["", "1\n", "0\n", "not a pid\n"] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                reload_via_pidfile(&path, Owner::Idle),
                Outcome::NotRunning,
                "{text:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
