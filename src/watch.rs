// SPDX-License-Identifier: GPL-3.0-or-later
//! Noticing that someone edited a config file by hand.
//!
//! The daemon being the sole *writer* does not make it the sole author: these files are meant
//! to be opened in an editor, and a panel with one of them open should show the change rather
//! than sit there with a stale value and then overwrite it. That is what this is for.
//!
//! Modeled on `wlrix-desktop/src/watch.rs`, including the parent-watch trick and its test
//! harness. The differences are all consequences of watching a config directory rather than a
//! desktop:
//!
//! ## Directories, not files
//!
//! Every writer here -- this daemon, `vim`, `helix`, `install`, `mv` -- replaces the file by
//! renaming a new one over it. That leaves the old inode intact and a per-file watch pointed at
//! something nothing will ever write to again: the first save is seen and no save after it is.
//! So the watch is on the directory, and `MOVED_TO` is the event that matters.
//!
//! The cost of watching the directory is that an editor's swapfile lands in the same events,
//! which is why the names are checked against [`crate::paths::is_known_file`].
//!
//! ## Two directories
//!
//! `$XDG_CONFIG_HOME/wlrix/` is where writes go, and `/etc/wlrix/` decides the effective value
//! of every key the user has not overridden -- wlRIX takes the first file it finds and does not
//! merge, so a system file appearing or changing is a real change for anyone without a file of
//! their own.
//!
//! ## A quiet period, not a timer
//!
//! Saving a file produces a burst: a rename, sometimes a create and a close before it. Acting
//! on each would mean re-reading and re-diffing several times and emitting several signals for
//! one save. [`Watch::wait`] blocks until something happens, then keeps polling with a short
//! timeout until a poll comes back empty, and reports the whole burst once. `poll` on the
//! inotify fd is the entire mechanism -- no timer wheel, and nothing to add to the loop.

use std::collections::BTreeSet;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::inotify;

use crate::paths::{self, File, Roots};

/// What we ask inotify for.
///
/// `MOVED_TO` is the load-bearing one: it is what an atomic write looks like from the outside,
/// and it is what this daemon's own writes are. `CLOSE_WRITE` covers an editor that writes in
/// place. `MOVED_FROM`/`DELETE` matter because removing a user file reveals the system one
/// underneath, which changes the effective value of everything in it.
const WATCHED: inotify::WatchFlags = inotify::WatchFlags::CREATE
    .union(inotify::WatchFlags::CLOSE_WRITE)
    .union(inotify::WatchFlags::MOVED_TO)
    .union(inotify::WatchFlags::MOVED_FROM)
    .union(inotify::WatchFlags::DELETE)
    .union(inotify::WatchFlags::DELETE_SELF)
    .union(inotify::WatchFlags::MOVE_SELF);

/// What the parent watch asks for: just enough to notice `wlrix/` appearing.
const WATCHED_PARENT: inotify::WatchFlags = inotify::WatchFlags::CREATE
    .union(inotify::WatchFlags::MOVED_TO)
    .union(inotify::WatchFlags::DELETE);

/// How long the directory has to stay quiet before a burst is called finished.
///
/// Long enough to cover an editor's create-write-rename, short enough that a panel updates
/// while the user is still looking at it.
const QUIET: Duration = Duration::from_millis(200);

/// A watch on the config directories.
pub struct Watch {
    fd: OwnedFd,
    /// The directory writes go to. `None` while it does not exist.
    user: Option<i32>,
    /// The watch on `$XDG_CONFIG_HOME`, held **only** while `user` is `None`.
    ///
    /// That directory is busy -- every application's config lives there -- so watching it costs
    /// a wake-up per unrelated write. It is worth watching for the one thing it can tell us
    /// that nothing else can, that `wlrix/` has appeared, and is dropped the moment it has.
    user_parent: Option<i32>,
    user_dir: Option<PathBuf>,
    /// `/etc/wlrix`. Re-attempted on every wake rather than watching `/etc` for it to appear:
    /// `/etc` is busier still, and a system config directory materializing mid-session means a
    /// package was installed, which is not a moment anyone expects live updates from.
    system: Option<i32>,
    system_dir: PathBuf,
}

impl Watch {
    /// Start watching. Fails only if inotify itself is unavailable.
    ///
    /// Neither directory needs to exist. A session with no config at all is the ordinary case
    /// on a fresh install, and the daemon creates the user directory itself on the first write.
    pub fn new(roots: &Roots) -> std::io::Result<Self> {
        // Non-blocking: the reader drains until the fd is empty and must not stall there.
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)?;
        let mut watch = Self {
            fd,
            user: None,
            user_parent: None,
            user_dir: roots.user_dir().map(Path::to_path_buf),
            system: None,
            system_dir: roots.system_dir().to_path_buf(),
        };
        watch.rewatch();
        Ok(watch)
    }

    /// (Re)attach both watches, and keep the parent watch in step.
    ///
    /// Called after every burst, because a directory that was deleted and recreated is a new
    /// inode and the old watch is dead. Cheap when nothing has changed: inotify returns the
    /// same descriptor for the same path.
    fn rewatch(&mut self) {
        if let Some(dir) = &self.user_dir {
            self.user = inotify::add_watch(&self.fd, dir, WATCHED).ok();
            match (self.user, self.user_parent) {
                // Watching the directory: the parent has nothing left to tell us.
                (Some(_), Some(parent)) => {
                    let _ = inotify::remove_watch(&self.fd, parent);
                    self.user_parent = None;
                }
                // No directory to watch, and not yet watching for one to appear.
                (None, None) => {
                    self.user_parent = dir.parent().and_then(|parent| {
                        inotify::add_watch(&self.fd, parent, WATCHED_PARENT).ok()
                    });
                }
                _ => {}
            }
        }
        self.system = inotify::add_watch(&self.fd, &self.system_dir, WATCHED).ok();
    }

    /// Whether the user's config directory is currently watched.
    ///
    /// A `false` means it does not exist, which is a legitimate state. Only interesting to the
    /// startup log line and to the tests.
    pub fn is_watching_user_dir(&self) -> bool {
        self.user.is_some()
    }

    /// Block until a config file changes, then report which ones.
    ///
    /// Returns an empty set only when something happened that turned out not to concern us --
    /// an editor's swapfile, or a directory appearing. The caller re-reads what is named and
    /// diffs; nothing here decides what a change *means*.
    pub fn wait(&mut self) -> std::io::Result<BTreeSet<File>> {
        // Anything left over from before, so a burst that arrived while we were busy writing is
        // not waited for a second time.
        let mut changed = self.drain();
        if changed.is_empty() {
            self.block()?;
            changed = self.drain();
        }

        // Settle: keep taking whatever else arrives until the directory goes quiet. A save is
        // several events and should be one signal.
        while self.wait_briefly()? {
            changed.extend(self.drain());
        }

        self.rewatch();
        Ok(changed)
    }

    /// Wait indefinitely for the fd to become readable.
    fn block(&self) -> std::io::Result<()> {
        loop {
            let borrowed = self.fd.as_fd();
            let mut fds = [PollFd::new(&borrowed, PollFlags::IN)];
            match rustix::event::poll(&mut fds, None) {
                Ok(_) => return Ok(()),
                // A signal arrived. Nothing here depends on which; go back to waiting.
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Wait out the quiet period, answering whether anything else turned up.
    fn wait_briefly(&self) -> std::io::Result<bool> {
        let timeout = Timespec {
            tv_sec: QUIET.as_secs() as _,
            tv_nsec: QUIET.subsec_nanos() as _,
        };
        loop {
            let borrowed = self.fd.as_fd();
            let mut fds = [PollFd::new(&borrowed, PollFlags::IN)];
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(0) => return Ok(false),
                Ok(_) => return Ok(true),
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Take every pending event and say which config files they name.
    ///
    /// Always drains fully, whatever it finds: leaving the fd readable would turn the next poll
    /// into a busy loop. An event with no name is a directory-level one (`DELETE_SELF`,
    /// `MOVE_SELF`) and means every file in it may have changed.
    fn drain(&mut self) -> BTreeSet<File> {
        let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
        let mut changed = BTreeSet::new();
        let mut reader = inotify::Reader::new(&self.fd, &mut buffer);
        loop {
            let event = match reader.next() {
                Ok(event) => event,
                // Drained, or nothing there yet. (`AGAIN` is the same value on Linux.)
                Err(rustix::io::Errno::WOULDBLOCK) => break,
                Err(rustix::io::Errno::INTR) => continue,
                // Anything else means the fd is unusable; stop rather than spin on it.
                Err(_) => break,
            };
            match event.file_name().and_then(|name| name.to_str().ok()) {
                Some(name) => {
                    if paths::is_known_file(name)
                        && let Some(file) = File::from_namespace(name.trim_end_matches(".toml"))
                    {
                        changed.insert(file);
                    }
                }
                // A nameless event is about a watch rather than a file in it -- but only some
                // of them mean anything changed.
                //
                // `DELETE_SELF`/`MOVE_SELF`/`UNMOUNT`: the directory is gone, so every file
                // that was in it is too. `QUEUE_OVERFLOW`: events were dropped and there is no
                // way to know which, so everything has to be re-read.
                //
                // `IGNORED` is neither. The kernel sends it whenever a watch stops existing,
                // *including* when we remove one ourselves -- which happens every time the
                // parent watch is dropped after the config directory appears. Reading that as
                // "all five files changed" would have the daemon re-read and re-diff the whole
                // config every time a directory came or went. It found nothing to report, so
                // it was invisible; a test is what caught it.
                None if event.events().intersects(
                    inotify::ReadFlags::DELETE_SELF
                        .union(inotify::ReadFlags::MOVE_SELF)
                        .union(inotify::ReadFlags::UNMOUNT)
                        .union(inotify::ReadFlags::QUEUE_OVERFLOW),
                ) =>
                {
                    changed.extend(paths::ALL.iter().copied())
                }
                None => {}
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Watch` pointed at scratch directories instead of the real ones.
    ///
    /// The environment is not touched: `set_var` is racy in a test binary that runs its tests
    /// on several threads, and the only thing `Watch::new` reads it for is these two paths.
    fn watching(user: Option<PathBuf>, system: PathBuf) -> Watch {
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
            .expect("inotify");
        let mut watch = Watch {
            fd,
            user: None,
            user_parent: None,
            user_dir: user,
            system: None,
            system_dir: system,
        };
        watch.rewatch();
        watch
    }

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wlrix-settings-watch-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("make the scratch directory");
        dir
    }

    /// inotify delivers asynchronously, so give it a moment before deciding nothing happened.
    ///
    /// Generous on purpose: this is a "did it arrive at all" check, not a latency measurement,
    /// and a short budget turns into a flaky test on a loaded machine.
    fn settled_drain(watch: &mut Watch) -> BTreeSet<File> {
        for _ in 0..400 {
            let changed = watch.drain();
            if !changed.is_empty() {
                return changed;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        BTreeSet::new()
    }

    /// Replace a file the way every writer here actually does it: write a temporary, rename
    /// over the target.
    fn atomic_write(path: &Path, text: &str) {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, text).unwrap();
        std::fs::rename(&temporary, path).unwrap();
    }

    #[test]
    fn an_atomic_write_is_noticed() {
        // The case a per-file watch gets wrong: the rename replaces the inode, so a watch on
        // the file itself would be pointed at something nothing writes to again.
        let dir = scratch("atomic");
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));
        assert!(watch.is_watching_user_dir());

        atomic_write(
            &dir.join("compositor.toml"),
            "[keyboard]\nlayout = \"jp\"\n",
        );
        assert_eq!(
            settled_drain(&mut watch),
            BTreeSet::from([File::Compositor])
        );

        // And again, which is what a per-file watch would already have missed.
        atomic_write(
            &dir.join("compositor.toml"),
            "[keyboard]\nlayout = \"us\"\n",
        );
        assert_eq!(
            settled_drain(&mut watch),
            BTreeSet::from([File::Compositor])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_in_place_edit_is_noticed() {
        // What an editor without atomic saves does, and what `>>` from a shell does.
        let dir = scratch("in-place");
        let path = dir.join("idle.toml");
        std::fs::write(&path, "[gamepad]\nenable = true\n").unwrap();
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));
        let _ = settled_drain(&mut watch);

        std::fs::write(&path, "[gamepad]\nenable = false\n").unwrap();
        assert_eq!(settled_drain(&mut watch), BTreeSet::from([File::Idle]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_a_user_file_is_a_change() {
        // It reveals `/etc/wlrix` underneath, which changes the effective value of everything
        // that file set -- wlRIX takes the first file it finds and does not merge.
        let dir = scratch("removed");
        let path = dir.join("desktop.toml");
        std::fs::write(&path, "snap_to_grid = true\n").unwrap();
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));
        let _ = settled_drain(&mut watch);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(settled_drain(&mut watch), BTreeSet::from([File::Desktop]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_swapfile_in_the_same_directory_is_not_a_config_change() {
        // The price of watching the directory rather than the file. `vim` leaves `.x.toml.swp`
        // and a `4913` probe file beside whatever it opens, and this daemon leaves its own
        // `.x.toml.tmp` there for a moment during every write.
        let dir = scratch("swapfile");
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));

        for name in [
            ".compositor.toml.swp",
            "compositor.toml~",
            "4913",
            "outputs.toml",
            "autostart.toml",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(watch.drain().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_system_directory_is_watched_too() {
        let dir = scratch("system");
        let user = dir.join("config");
        let system = dir.join("etc");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let mut watch = watching(Some(user), system.clone());

        atomic_write(&system.join("portal.toml"), "[preview]\ntick_ms = 250\n");
        assert_eq!(settled_drain(&mut watch), BTreeSet::from([File::Portal]));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quiet_directory_reports_nothing() {
        let dir = scratch("quiet");
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));
        let _ = watch.drain();
        assert!(watch.drain().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_config_directory_that_does_not_exist_yet_is_still_watchable() {
        // A fresh install has no `~/.config/wlrix`. Someone creating it by hand, or another
        // program creating it, has to be picked up -- otherwise the daemon watches nothing for
        // the rest of the session.
        let parent = scratch("late-directory");
        let dir = parent.join("wlrix");
        let mut watch = watching(Some(dir.clone()), parent.join("etc"));
        assert!(!watch.is_watching_user_dir());
        assert!(
            watch.user_parent.is_some(),
            "should be waiting on the parent"
        );

        std::fs::create_dir(&dir).unwrap();
        // The event names `wlrix`, which is a directory rather than a config file, so it is
        // filtered out and reported as no change -- correctly: nothing in it has been read yet.
        // What it is *for* is the rewatch that follows, which `wait` does on every wake.
        std::thread::sleep(Duration::from_millis(100));
        assert!(watch.drain().is_empty(), "a directory is not a setting");
        watch.rewatch();
        assert!(watch.is_watching_user_dir());
        assert!(
            watch.user_parent.is_none(),
            "the parent watch should be released"
        );

        // And the file written into it right afterwards is seen.
        atomic_write(&dir.join("session.toml"), "compositor = \"x\"\n");
        assert_eq!(settled_drain(&mut watch), BTreeSet::from([File::Session]));

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn losing_the_directory_puts_every_file_in_doubt() {
        // `DELETE_SELF` carries no filename, so there is nothing to filter on -- and it is
        // true that every file that was in there is gone.
        let parent = scratch("removed-directory");
        let dir = parent.join("wlrix");
        std::fs::create_dir(&dir).unwrap();
        let mut watch = watching(Some(dir.clone()), parent.join("etc"));
        assert!(watch.is_watching_user_dir());
        let _ = settled_drain(&mut watch);

        std::fs::remove_dir(&dir).unwrap();
        let changed = settled_drain(&mut watch);
        assert_eq!(changed.len(), paths::ALL.len(), "{changed:?}");

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn a_burst_of_events_is_one_answer() {
        // Unpacking a config from an archive, or `install`-ing several at once. The point of
        // draining fully rather than reporting the first event.
        let dir = scratch("burst");
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));

        for file in ["compositor.toml", "idle.toml", "desktop.toml"] {
            atomic_write(&dir.join(file), "x = 1\n");
        }
        // Give the whole burst time to land, then take it in one go.
        std::thread::sleep(Duration::from_millis(150));
        let changed = watch.drain();
        assert_eq!(
            changed,
            BTreeSet::from([File::Compositor, File::Idle, File::Desktop])
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_returns_the_whole_burst_once() {
        // `wait` blocks, so the writes have to come from another thread. This is the shape the
        // daemon actually runs in.
        let dir = scratch("wait");
        let mut watch = watching(Some(dir.clone()), dir.join("etc"));

        let writing = dir.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            atomic_write(&writing.join("compositor.toml"), "x = 1\n");
            std::thread::sleep(Duration::from_millis(20));
            atomic_write(&writing.join("idle.toml"), "y = 2\n");
        });

        let changed = watch.wait().expect("should wait");
        writer.join().unwrap();
        assert_eq!(changed, BTreeSet::from([File::Compositor, File::Idle]));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
