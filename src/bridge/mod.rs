// SPDX-License-Identifier: GPL-3.0-or-later
//! Carrying a wlRIX setting to something that does not read wlRIX config files.
//!
//! Everything under [`crate::schema`] describes a value in a TOML file this daemon owns, and
//! everything under [`crate::apply`] tells the program that reads that file. A bridge is
//! neither: it is a *derived* file, written somewhere else, in somebody else's format, because
//! the program that reads it has no idea wlRIX exists.
//!
//! That is why a bridge is not a member of an `appearance.palette` group in the
//! [`crate::schema::group`] sense -- a member has to be a declared `Setting` with a `File`
//! behind it. A bridge is fired by the same event and writes something else entirely.
//!
//! There is one so far, for GTK. A Qt one belongs here when it exists.

pub mod gtk;
