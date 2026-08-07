// SPDX-License-Identifier: GPL-3.0-or-later
//! The bus side: owning the name, serving the interface, emitting the signals.
//!
//! ## `com.wlrix.Settings`
//!
//! Not a new namespace. `com.wlrix.*` is already the workspace's, used as the Wayland app id by
//! every Avalonia app -- `com.wlrix.toolchest`, `com.wlrix.desks`,
//! `com.wlrix.settings.keyboard`. Introducing a second reverse-DNS prefix here, in a project
//! that has already committed to one, would buy nothing. The shape matches Cosmic's
//! `com.system76.CosmicSettingsDaemon`, which is the model the workspace plan named.
//!
//! The version digit goes on the **interface** (`com.wlrix.Settings1`) and not on the bus name,
//! per the freedesktop convention that `org.freedesktop.login1` follows. A versioned bus name
//! would mean rewriting the activation files to add an interface.
//!
//! This is deliberately *not* `org.freedesktop.impl.portal.Settings`. That is
//! xdg-desktop-portal's read-only appearance interface for sandboxed applications, a different
//! job with a fixed set of keys; if wlRIX wants it, it belongs in `xdg-desktop-portal-wlrix`
//! proxying this daemon.
//!
//! ## A name already taken is fatal here
//!
//! `wlrix-idle` treats a name it cannot get as a message rather than a failure, because
//! refusing to start would take idle blanking down with it. The opposite applies to this one:
//! two settings daemons both writing the same files is precisely the race the program exists to
//! remove, and the second one carrying on regardless would reintroduce it. So it says who has
//! the name, and exits.
//!
//! ## Threads
//!
//! No calloop, no async, one thread. `wlrix-idle` and the portal need an event loop because
//! they hold Wayland and PipeWire file descriptors that must share one poll; this holds
//! neither. `main` builds the connection, then becomes the inotify loop, and zbus dispatches
//! method calls on its own executor behind the blocking connection.

mod settings;

use std::sync::Arc;

use zbus::blocking::Connection;
use zbus::fdo::RequestNameFlags;
use zbus::names::{BusName, WellKnownName};

use crate::store::{Notice, Store};

pub use settings::Settings;

/// The well-known name the daemon owns.
pub const NAME: &str = "com.wlrix.Settings";
/// The object everything is served at.
pub const PATH: &str = "/com/wlrix/Settings";
/// The interface, versioned as freedesktop does it.
pub const INTERFACE: &str = "com.wlrix.Settings1";

/// Serve the interface and take the name.
///
/// The object is served **before** the name is requested, so a client that resolves the name
/// the instant it is acquired never finds an empty connection behind it. That ordering is
/// `wlrix-idle`'s, and it is not theoretical: bus activation means the first thing to happen
/// after the name appears is a method call.
pub fn serve(store: Arc<Store>, replace: bool) -> Result<Connection, String> {
    let connection = Connection::session().map_err(|err| format!("no session bus: {err}"))?;

    connection
        .object_server()
        .at(PATH, Settings::new(store))
        .map_err(|err| format!("could not serve {PATH}: {err}"))?;

    let well_known =
        WellKnownName::try_from(NAME).map_err(|err| format!("{NAME} is not a bus name: {err}"))?;
    // `DoNotQueue`: standing in a queue would mean silently acquiring the name later, at the
    // moment another instance exited -- and by then whoever wanted us has given up.
    // `AllowReplacement` on every request, not only when `--replace` was passed: replacement is
    // granted by the *incumbent*, so a daemon that does not allow it can never be replaced, and
    // `--replace` would be a flag that could not work. (That exact mistake cost the portal
    // hours; see its `main.rs`.)
    let mut flags = RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement;
    if replace {
        flags |= RequestNameFlags::ReplaceExisting;
    }

    match connection.request_name_with_flags(well_known, flags) {
        Ok(_) => {
            tracing::info!("serving {NAME}");
            Ok(connection)
        }
        Err(err) => {
            let owner = owner_of(&connection, NAME).unwrap_or_else(|| "something else".to_owned());
            Err(format!(
                "{NAME} belongs to {owner}. Two settings daemons writing the same files is the \
                 race this program exists to remove, so this one is stopping. Use --replace to \
                 take over."
            ))
            .inspect_err(|_| {
                if !matches!(err, zbus::Error::NameTaken) {
                    tracing::warn!("could not take {NAME}: {err}");
                }
            })
        }
    }
}

/// Put what the watcher noticed on the bus.
///
/// Emitted through [`Connection::emit_signal`] rather than through the interface's generated
/// helpers, because those are `async fn`s taking a `SignalEmitter` and this is called from the
/// main thread, which is a plain inotify loop with no executor. The message on the wire is
/// identical either way -- the `#[zbus(signal)]` declarations next door are what put the
/// signals in the introspection XML, which is what a client generates its proxy from.
///
/// A failure to emit is logged and swallowed. The file has already been read and the owner
/// already signaled; a client that missed the announcement can always ask again, and taking
/// the daemon down over it would be worse.
pub fn announce(connection: &Connection, notices: &[Notice]) {
    for notice in notices {
        let sent = match notice {
            Notice::Changed(changed) => {
                let values: std::collections::HashMap<String, zbus::zvariant::OwnedValue> = changed
                    .values
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), settings::to_variant(value)))
                    .collect();
                // "external": nobody on the bus asked for this, somebody edited the file.
                emit(connection, "Changed", &(values, "external"))
            }
            Notice::Invalid {
                namespace,
                path,
                message,
            } => emit(
                connection,
                "FileInvalid",
                &(*namespace, path.display().to_string(), message.as_str()),
            ),
            Notice::Recovered { namespace, path } => emit(
                connection,
                "FileRecovered",
                &(*namespace, path.display().to_string()),
            ),
        };
        if let Err(err) = sent {
            tracing::warn!("could not emit a signal: {err}");
        }
    }
}

fn emit<B>(connection: &Connection, member: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    connection.emit_signal(None::<&str>, PATH, INTERFACE, member, body)
}

/// Who holds a name, described well enough to be recognized in a log.
///
/// Lifted from `wlrix-idle/src/dbus/mod.rs`, where it exists so a log line can say
/// `kwin_wayland` rather than `:1.34`.
fn owner_of(connection: &Connection, name: &str) -> Option<String> {
    let proxy = zbus::blocking::fdo::DBusProxy::new(connection).ok()?;
    let bus_name = BusName::try_from(name).ok()?;
    let owner = proxy.get_name_owner(bus_name).ok()?;
    let unique = BusName::from(owner.inner().clone());
    match proxy.get_connection_unix_process_id(unique.clone()) {
        Ok(pid) => Some(format!(
            "{} (pid {pid})",
            process_name(pid).unwrap_or_else(|| unique.to_string())
        )),
        Err(_) => Some(unique.to_string()),
    }
}

/// The command behind a pid, so the log names a program rather than a bus address.
fn process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}
