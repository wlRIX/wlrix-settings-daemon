// SPDX-License-Identifier: GPL-3.0-or-later
//! One setting the *desktop* has, which several config files each hold a copy of.
//!
//! [`table`](super::table) ties a key to a single file and a single owner, which is right for
//! almost everything: `[keyboard] layout` is the compositor's and nobody else's. A color scheme
//! is not like that. It has to reach the compositor's window chrome, the desktop's icons, the
//! tray and the screenshot overlay at once, and each of those reads it out of its own file --
//! so declaring it once per component would offer four switches for one setting, and a person
//! who moved three of them would be left with a desktop that half changed.
//!
//! A [`Group`] is the answer, and it is deliberately a thin one. The members are ordinary
//! settings, declared in the ordinary table, writable and resettable on their own by anyone who
//! genuinely wants the screenshot overlay dark and everything else light. The group adds one
//! key on top that fans out to all of them, and [`crate::store`] expands it before the existing
//! commit path -- which already groups edits by file, writes each file once, and signals each
//! owner once. Nothing about the transaction is new.
//!
//! ## Why the value is not an enum
//!
//! `appearance.palette` is a [`Kind::Str`], not a `Kind::Enum` listing the schemes this build
//! ships. The schemes live in `wlrix-assets/palette/*.json` and are generated into `wlrix-ui`
//! and into the Avalonia theme; a list of them here would be a fourth hand-kept copy, and a new
//! scheme would be unsettable until this file caught up. A client enumerates schemes from the
//! generated catalog it already links, and an id this build does not know is not an error
//! anyway -- every consumer resolves an unknown scheme to the default and says so in its log.

use super::{Kind, Setting};

/// A key that stands for the same setting in several files.
#[derive(Debug)]
pub struct Group {
    /// The D-Bus key. Its prefix is a *category*, not a namespace: there is no
    /// `appearance.toml` and `ListNamespaces` does not name one. `Groups` lists these instead.
    pub key: &'static str,
    /// The keys it expands to, in the order they are written. Every one is a real
    /// [`Setting`] in [`SETTINGS`](super::SETTINGS) with the same [`Kind`]; the tests below are
    /// what keeps that true.
    pub members: &'static [&'static str],
    /// The type of the value, which is every member's type.
    pub kind: Kind,
    /// One line, for a label or a tooltip.
    pub summary: &'static str,
    /// The longer version, for the panel's help text.
    pub description: &'static str,
}

impl Group {
    /// The members, resolved. Panics only if the tests below are not being run.
    pub fn settings(&self) -> impl Iterator<Item = &'static Setting> + use<'_> {
        self.members.iter().filter_map(|key| super::lookup(key))
    }
}

/// Every fan-out key the daemon serves.
pub static GROUPS: &[Group] = &[Group {
    key: "appearance.palette",
    // The compositor first: it draws the window chrome, which is the surface a person watching
    // for the change looks at. The order is otherwise the file order of `paths::ALL`.
    members: &[
        "compositor.appearance.palette",
        "desktop.appearance.palette",
        "screenshot.appearance.palette",
        "tray.appearance.palette",
    ],
    kind: Kind::Str { default: None },
    summary: "Color scheme",
    description: "The scheme everything wlRIX draws is colored from -- window chrome, desktop \
                  icons, the tray, the screenshot overlay and the applications. A scheme id \
                  from wlrix-ui: classic, classic-g10, classic-g24, gotham. Empty or \
                  unrecognized means the default, with a line in each component's log for the \
                  latter. Writing this writes every component's own key; each can still be set \
                  on its own for a session that wants one of them different.",
}];

/// The group this key names, if it is one.
pub fn lookup_group(key: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|group| group.key == key)
}

/// The group a member key belongs to, if any.
///
/// The reverse lookup, used when announcing a change: a hand-edit of `compositor.toml` has to
/// reach a client watching only `appearance.palette`.
pub fn group_of(member: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|group| group.members.contains(&member))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_member_is_a_real_setting() {
        // The whole mechanism rests on this: `set_many` expands a group into its members and
        // then resolves each one, so a member naming nothing would turn one Set into an
        // UnknownKey for a key the caller never mentioned.
        for group in GROUPS {
            for member in group.members {
                assert!(
                    super::super::lookup(member).is_some(),
                    "{}: no setting called {member}",
                    group.key
                );
            }
        }
    }

    #[test]
    fn a_group_and_its_members_agree_about_the_type() {
        // One value is coerced against each member in turn. A member declared as something else
        // would refuse the value the others accepted, after some of them had been written.
        for group in GROUPS {
            for setting in group.settings() {
                assert_eq!(
                    setting.kind.name(),
                    group.kind.name(),
                    "{} is {} but {} is {}",
                    group.key,
                    group.kind.name(),
                    setting.key,
                    setting.kind.name()
                );
            }
        }
    }

    #[test]
    fn a_group_key_is_not_also_a_setting() {
        // `resolve` tries the table first, so a collision would make the group unreachable
        // rather than ambiguous -- silently, and only for that one key.
        for group in GROUPS {
            assert!(
                super::super::lookup(group.key).is_none(),
                "{} is both a group and a setting",
                group.key
            );
        }
    }

    #[test]
    fn no_key_belongs_to_two_groups() {
        // `group_of` answers the first match, so a member in two groups would announce changes
        // under one of them and never the other.
        let mut seen = std::collections::BTreeSet::new();
        for group in GROUPS {
            for member in group.members {
                assert!(seen.insert(*member), "{member} is in more than one group");
            }
        }
    }

    #[test]
    fn a_group_names_no_key_twice() {
        for group in GROUPS {
            let unique: std::collections::BTreeSet<_> = group.members.iter().collect();
            assert_eq!(
                unique.len(),
                group.members.len(),
                "{} lists a member twice",
                group.key
            );
        }
    }

    #[test]
    fn the_scheme_reaches_everything_that_draws() {
        // Named rather than counted: the point of the group is that adding a component which
        // draws chrome and forgetting to list it here is the failure, and a count would pass.
        let group = lookup_group("appearance.palette").expect("the scheme is a group");
        for expected in [
            "compositor.appearance.palette",
            "desktop.appearance.palette",
            "screenshot.appearance.palette",
            "tray.appearance.palette",
        ] {
            assert!(group.members.contains(&expected), "{expected} is missing");
        }
        assert_eq!(
            group_of("tray.appearance.palette").map(|g| g.key),
            Some(group.key)
        );
        assert_eq!(group_of("compositor.keyboard.layout").map(|g| g.key), None);
    }
}
