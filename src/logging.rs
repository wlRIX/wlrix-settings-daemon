// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the daemon's output goes.
//!
//! stderr, and only stderr. This is bus-activated and runs as a systemd user unit, so stderr is
//! the journal -- already the right place, already rotated, already interleaved with the
//! compositor's and the session's. `wlrix-idle` and `wlrix-desktop` hand-roll a macro pair
//! instead because `wlrix-session` redirects their output into its own log file; nothing
//! redirects this one.
//!
//! `RUST_LOG` picks the level. The default is `info`; `RUST_LOG=debug` shows every file read,
//! every value written, and every signal delivered, which is what to reach for when a setting
//! appears not to apply.

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        // No timestamps: the journal stamps every line already, and a second clock in the text
        // only makes the lines longer.
        //
        // No color either. stderr here is the journal or a redirected file, neither of which
        // renders escape codes -- and `tracing` puts them *between* a field's name and its `=`,
        // so `grep 'key='` silently matches nothing in a log full of `key=…`. That cost real
        // time during the portal's development; see its `src/logging.rs`.
        .with(
            fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(std::io::stderr),
        )
        .init();
}
