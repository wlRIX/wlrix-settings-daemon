// SPDX-License-Identifier: GPL-3.0-or-later
//! The wlRIX settings daemon: the one program that writes the wlRIX config files.
//!
//! Every wlRIX component reads its own TOML file and always will. What this replaces is the
//! *writing*: before it, a settings app hand-rolled a TOML editor, hand-rolled a pidfile read
//! and hand-rolled a `kill(pid, SIGHUP)` -- and the next settings app would have copied all
//! three. Here that happens once, in a language with a real TOML parser, behind
//! `com.wlrix.Settings` on the session bus.
//!
//! Three things it is careful about, each of which is the whole reason a piece of it exists:
//!
//! - **Your file stays your file.** Comments, key order, spacing, and every section it was not
//!   asked to touch survive a write intact. See [`edit`].
//! - **Hand-editing keeps working.** An inotify watch notices, and any panel with that file
//!   open is told. See [`watch`] and [`store`].
//! - **The desktop works without it.** Nothing depends on this program; it is a writer and a
//!   notifier, never a source of truth. A session with the binary removed is a complete
//!   session -- just one where settings apps have nothing to talk to.
//!
//! Bus-activated, so it does nothing at all until something asks.

mod apply;
mod dbus;
mod edit;
mod logging;
mod paths;
mod schema;
mod store;
mod watch;

use std::process::ExitCode;

const USAGE: &str = "\
wlrix-settings-daemon -- the wlRIX settings service

Usage: wlrix-settings-daemon [options]

Options:
  --replace        Take com.wlrix.Settings from whoever holds it
  --dump-schema    Print every setting at its default, as an annotated config file
  --check          Report what is wrong with the config files, then exit
  -h, --help       This
  -V, --version    Print the version

Normally started by D-Bus activation rather than by hand. RUST_LOG picks the log level;
RUST_LOG=debug shows every file read, every value written and every signal sent.
";

fn main() -> ExitCode {
    // Hand-rolled, as everywhere else in wlRIX: four flags is not worth an argument parser, and
    // the workspace is consistent about it.
    let (mut replace, mut dump_schema, mut check_only) = (false, false, false);
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--replace" => replace = true,
            "--dump-schema" => dump_schema = true,
            "--check" => check_only = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("wlrix-settings-daemon {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("wlrix-settings-daemon: unknown argument {other}\n");
                eprint!("{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    if dump_schema {
        print!("{}", schema::dump());
        return ExitCode::SUCCESS;
    }

    logging::init();
    let roots = paths::Roots::from_environment();
    let store = store::Store::load(roots.clone());

    if check_only {
        return check(&store);
    }

    // The object is served and the name taken before the watch starts, so a client that arrives
    // the instant the name appears -- which, under bus activation, is every client -- finds a
    // working service rather than a half-built one.
    let connection = match dbus::serve(store.clone(), replace) {
        Ok(connection) => connection,
        Err(why) => {
            tracing::error!("{why}");
            return ExitCode::FAILURE;
        }
    };

    let mut watch = match watch::Watch::new(&roots) {
        Ok(watch) => watch,
        Err(err) => {
            // Not fatal. Without the watch a hand-edited file goes unnoticed until something
            // calls `Reload` -- a worse daemon, but a working one, and refusing to start would
            // leave the session with no settings service at all.
            tracing::error!(
                "could not watch the config directories ({err}); hand edits will go unnoticed \
                 until Reload"
            );
            return serve_forever();
        }
    };
    match roots.user_dir() {
        Some(dir) if watch.is_watching_user_dir() => tracing::info!("watching {}", dir.display()),
        Some(dir) => tracing::info!("{} does not exist yet; waiting for it", dir.display()),
        None => tracing::warn!("no config directory; nothing can be written"),
    }

    // The main thread *is* the watcher. zbus dispatches method calls on its own executor behind
    // the blocking connection, so there is nothing else for this one to do -- and no calloop,
    // because unlike wlrix-idle and the portal this program holds no Wayland or PipeWire file
    // descriptor that would have to share a poll with the inotify one.
    loop {
        match watch.wait() {
            Ok(files) if files.is_empty() => continue,
            Ok(files) => {
                let notices = store.refresh(&files);
                dbus::announce(&connection, &notices);
            }
            Err(err) => {
                tracing::error!("the config watch failed ({err}); carrying on without it");
                return serve_forever();
            }
        }
    }
}

/// Keep answering method calls with no watch to run.
///
/// zbus is already serving on its own executor and the connection is alive for as long as the
/// process is; this thread only has to not exit. Parking is the honest way to say that.
fn serve_forever() -> ExitCode {
    loop {
        std::thread::park();
    }
}

/// `--check`: say what is wrong with the config files, and what is in them if nothing is.
///
/// Reads exactly what the daemon reads, so "why is my setting not taking effect" can be
/// answered without starting a daemon or attaching to a bus.
fn check(store: &store::Store) -> ExitCode {
    let invalid = store.invalid();
    if !invalid.is_empty() {
        for (namespace, path, message) in invalid {
            eprintln!("{namespace}: {} is not valid: {message}", path.display());
        }
        return ExitCode::FAILURE;
    }

    for namespace in schema::namespaces() {
        let sources = store.sources(namespace);
        let set = sources
            .values()
            .filter(|source| **source != store::Source::Default)
            .count();
        println!("{namespace}: {set} of {} settings are set", sources.len());
    }
    ExitCode::SUCCESS
}
