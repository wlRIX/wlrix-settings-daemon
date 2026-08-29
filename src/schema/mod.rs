// SPDX-License-Identifier: GPL-3.0-or-later
//! What a wlRIX setting *is*.
//!
//! Every setting the daemon will write is declared once, in [`table`], with the file it lives
//! in, its path within that file, its type and range, its default, which process owns it, and
//! how that process takes the change. A client asks for the declaration ([`Setting::describe`]
//! over D-Bus) and can render and validate a whole panel without hardcoding a single wlRIX key
//! name.
//!
//! ## The table is advisory; the owner's parser is the authority
//!
//! The daemon cannot link `wlrix-compositor`'s `Config` type -- the repos build standalone,
//! with no path dependencies between them -- so this table is a hand-kept copy of what those
//! serde structs accept, and a hand-kept copy will drift. That matters more here than it looks
//! like it should: every wlRIX config struct is `#[serde(deny_unknown_fields)]`, so *one* key
//! the owner does not recognize makes it reject the **whole file** and fall back to built-in
//! defaults. A drifted table is not a wrong setting; it is a lost config.
//!
//! So the table is not the last line of defense. [`crate::edit`] validates a candidate file
//! through the owning program's own parser (`<owner> --check-config <path>`) before committing
//! the rename, which puts correctness back with the type that defines it. What is declared here
//! is what a *UI* needs -- ranges, choices, units, prose -- plus enough structure to reject the
//! obvious mistakes early, with a clearer message than a TOML parse error.
//!
//! ## What is not here, and why
//!
//! Array-of-table sections (`[[output]]`, `[[timeout]]`, `[[app]]`) and free-form maps
//! (`[env]`) are deliberately absent. They are keyed collections, not scalar leaves, and they
//! need add/remove/reorder rather than get/set -- a different surface, planned for a later
//! version, which nothing about the current signatures blocks. `Set` on such a path answers
//! `Unsupported` rather than pretending. The README says so in full; `[[output]]` in particular
//! has reasons beyond its shape, and a Displays panel should be speaking
//! `wlr-output-management` instead.

pub mod table;

use std::fmt;

use crate::paths::File;

pub use table::SETTINGS;

/// One setting: what it is, where it lives and who has to be told when it changes.
#[derive(Debug)]
pub struct Setting {
    /// The D-Bus key, `<namespace>.<toml path>`. Always the namespace of [`Setting::file`]
    /// followed by [`Setting::path`] joined with dots -- asserted in the tests, because a key
    /// that does not match its own path is a lookup that silently finds nothing.
    pub key: &'static str,
    pub file: File,
    /// The path to the value within the document, outermost first. A one-element path is a
    /// top-level key; two elements are a key in a table.
    pub path: &'static [&'static str],
    pub kind: Kind,
    /// Who reads this, and therefore who is signaled after a write.
    pub owner: Owner,
    /// What that costs the user.
    pub reload: Reload,
    /// What the number means, for a UI to put after the spinner.
    pub unit: Unit,
    /// One line, for a label or a tooltip.
    pub summary: &'static str,
    /// The longer version, for the panel's help text.
    pub description: &'static str,
}

/// A setting's type, its range, and what it is when the file does not say.
///
/// The `Option` on every default is load-bearing and is what the D-Bus `has_default` flag
/// reports. `compositor.keyboard.layout` genuinely has no default: absent means "let
/// libxkbcommon decide", which is not a value the daemon can name (see
/// `wlrix-compositor/src/config.rs`, where `KeyboardConfig::xkb` maps `None` to `""`).
/// `repeat_delay` does have one, 200. A UI must be able to show "system default" and "200" as
/// different things, and a `default` of `0` cannot carry that distinction.
#[derive(Debug)]
pub enum Kind {
    Bool {
        default: Option<bool>,
    },
    Int {
        default: Option<i64>,
        min: i64,
        max: i64,
    },
    Float {
        default: Option<f64>,
        min: f64,
        max: f64,
    },
    Str {
        default: Option<&'static str>,
    },
    /// A string from a fixed set. The values are exactly what the owner's serde enum accepts.
    Enum {
        default: Option<&'static str>,
        choices: &'static [Choice],
    },
    /// A list of strings. Its default is the empty list rather than "absent": every list-valued
    /// setting in wlRIX means the same thing by an empty list and a missing key.
    StrList {
        default: &'static [&'static str],
    },
}

impl Kind {
    /// The D-Bus signature a value of this kind is carried as.
    pub fn signature(&self) -> &'static str {
        match self {
            Self::Bool { .. } => "b",
            Self::Int { .. } => "x",
            Self::Float { .. } => "d",
            Self::Str { .. } | Self::Enum { .. } => "s",
            Self::StrList { .. } => "as",
        }
    }

    /// The name a client switches on to pick a widget.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bool { .. } => "bool",
            Self::Int { .. } => "int",
            Self::Float { .. } => "double",
            Self::Str { .. } => "string",
            Self::Enum { .. } => "enum",
            Self::StrList { .. } => "string-list",
        }
    }

    /// Whether this setting has a default the daemon can name.
    ///
    /// A list always does -- the empty list. Everything else says so itself.
    pub fn has_default(&self) -> bool {
        match self {
            Self::Bool { default } => default.is_some(),
            Self::Int { default, .. } => default.is_some(),
            Self::Float { default, .. } => default.is_some(),
            Self::Str { default } | Self::Enum { default, .. } => default.is_some(),
            Self::StrList { .. } => true,
        }
    }
}

/// One value of an [`Kind::Enum`], with something to put in a dropdown.
///
/// The label is English and untranslated on purpose. Presentation belongs to the client, which
/// has a localization story the daemon does not; but a client with no translation for a wlRIX
/// enum still has to render *something*, and hardcoding wlRIX's values in every app is exactly
/// what this whole interface exists to avoid.
#[derive(Debug)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

/// Which running program reads a setting, and therefore gets told when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Background,
    Compositor,
    Desktop,
    Idle,
    Portal,
    /// `wlrix-screenshot`. Not a daemon: it runs for as long as one screenshot takes and reads
    /// its config each time, so there is never one to signal. It is an owner all the same,
    /// because this enum also names *whose parser validates a candidate file* -- which is the
    /// load-bearing half, and `Owner::None` would give up.
    Screenshot,
    /// Nothing to tell: the value is read once, by something that is not running yet.
    None,
}

impl Owner {
    /// The program's name, as it appears in a log line and on `PATH`.
    pub fn program(self) -> Option<&'static str> {
        match self {
            Self::Background => Some("wlrix-bg"),
            Self::Compositor => Some("wlrix-compositor"),
            Self::Desktop => Some("wlrix-desktop"),
            Self::Idle => Some("wlrix-idle"),
            Self::Portal => Some("xdg-desktop-portal-wlrix"),
            Self::Screenshot => Some("wlrix-screenshot"),
            Self::None => None,
        }
    }

    /// The pidfile the owner drops in `$XDG_RUNTIME_DIR`, if it drops one.
    ///
    /// Kept in step by hand with each component's own `pidfile.rs`; there is no shared constant
    /// to point at, and each of those files carries a test saying so.
    ///
    /// The portal has none deliberately -- its own `signals.rs` explains that a screen share is
    /// not something to reconfigure underneath it, so there is nothing a signal could ask for.
    /// `wlrix-screenshot` has none because it is not running: it is spawned per screenshot.
    pub fn pidfile(self) -> Option<&'static str> {
        match self {
            Self::Background => Some("wlrix-bg.pid"),
            Self::Compositor => Some("wlrix-compositor.pid"),
            Self::Desktop => Some("wlrix-desktop.pid"),
            Self::Idle => Some("wlrix-idle.pid"),
            Self::Portal | Self::Screenshot | Self::None => None,
        }
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.program().unwrap_or(""))
    }
}

/// What it takes for a change to this setting to be in effect.
///
/// Reported back from a write so a panel can say "the compositor is not running; this applies
/// at next login" instead of appearing to do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reload {
    /// The owner re-reads it on `SIGHUP` and it takes effect immediately.
    Live,
    /// The owner must be restarted. Either it has no reload path, or its reload deliberately
    /// refuses this section -- `wlrix-idle`'s `[dbus]` is the second case: dropping and
    /// retaking a bus name would void every inhibit applications currently hold.
    Restart,
    /// Only read when the session starts.
    NextLogin,
    /// Nothing reads it while running and nothing can be told. Distinct from `Restart` in that
    /// restarting the owner is not the user's job either.
    None,
}

impl Reload {
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Restart => "restart",
            Self::NextLogin => "next-login",
            Self::None => "none",
        }
    }
}

/// What a number means, so a UI can put a suffix after the spinner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    None,
    Milliseconds,
    Seconds,
    Hertz,
    Pixels,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Milliseconds => "ms",
            Self::Seconds => "s",
            Self::Hertz => "hz",
            Self::Pixels => "px",
        }
    }
}

impl Setting {
    /// The namespace this setting belongs to.
    pub fn namespace(&self) -> &'static str {
        self.file.namespace()
    }
}

/// The setting this key names, if there is one.
pub fn lookup(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| setting.key == key)
}

/// Every setting in one namespace, in declaration order.
///
/// Declaration order is the order of the file itself, which is what a generated example and a
/// generated panel both want. Alphabetical would interleave `[keyboard]` and `[focus]`.
pub fn in_namespace(namespace: &str) -> impl Iterator<Item = &'static Setting> {
    SETTINGS
        .iter()
        .filter(move |setting| setting.namespace() == namespace)
}

/// Every namespace the daemon serves, in file order.
pub fn namespaces() -> Vec<&'static str> {
    crate::paths::ALL
        .iter()
        .map(|file| file.namespace())
        .collect()
}

/// Every setting at its declared default, as annotated TOML.
///
/// This is the second layer of the drift defense described at the top of this module, and its
/// value is entirely in where it is *used*: copy the section for one component into that
/// component's own tests and assert that its serde types accept it. A key this table has that
/// the owner does not know about is then a failing test in the owner's repo rather than a
/// user's whole config file being rejected at runtime.
///
/// Deliberately not valid as a drop-in config: settings with no default are emitted commented
/// out, because writing `layout = ""` would be writing a value, and an empty layout is not what
/// "let libxkbcommon decide" means.
pub fn dump() -> String {
    let mut out = String::new();
    out.push_str(
        "# Every wlRIX setting the settings daemon knows about, at its declared default.\n\
         #\n\
         # Generated by `wlrix-settings-daemon --dump-schema`. Not a config file to install:\n\
         # each block below belongs in a different file, and settings with no default are\n\
         # commented out because absent and empty do not mean the same thing.\n",
    );

    for file in crate::paths::ALL {
        let mut settings = in_namespace(file.namespace()).peekable();
        if settings.peek().is_none() {
            continue;
        }
        out.push_str(&format!("\n# ---- {} ----\n", file.file_name()));

        let mut table: Option<&str> = None;
        for setting in settings {
            // Emit `[section]` once, when the path first enters it.
            if let [section, _] = setting.path
                && table != Some(section)
            {
                out.push_str(&format!("\n[{section}]\n"));
                table = Some(*section);
            }
            out.push_str(&format!("# {}\n", setting.summary));
            let leaf = setting.path.last().copied().unwrap_or_default();
            match default_literal(&setting.kind) {
                Some(value) => out.push_str(&format!("{leaf} = {value}\n")),
                None => out.push_str(&format!("# {leaf} =    # no default; unset by default\n")),
            }
        }
    }
    out
}

/// A setting's default as it would be written in TOML, if it has one.
fn default_literal(kind: &Kind) -> Option<String> {
    match kind {
        Kind::Bool { default } => default.map(|v| v.to_string()),
        Kind::Int { default, .. } => default.map(|v| v.to_string()),
        // Always with a decimal point: TOML's integers and floats are different types, and
        // `deadzone = 0` is a parse error for a field declared `f32`.
        Kind::Float { default, .. } => default.map(|v| {
            if v.fract() == 0.0 {
                format!("{v:.1}")
            } else {
                v.to_string()
            }
        }),
        Kind::Str { default } | Kind::Enum { default, .. } => default.map(|v| format!("{v:?}")),
        Kind::StrList { default } => Some(format!("{default:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // The integrity checks below are the cheapest layer of the drift defense described in the
    // module docs: they cost nothing, they run in CI, and they catch the mistakes that come
    // from editing a long const table by hand.

    #[test]
    fn every_key_is_unique() {
        let mut seen = HashSet::new();
        for setting in SETTINGS {
            assert!(seen.insert(setting.key), "duplicate key {}", setting.key);
        }
    }

    #[test]
    fn a_key_spells_out_its_own_path() {
        // The lookup is by key and the write is by path, so a key that disagrees with its path
        // writes somewhere other than where it reads -- which is invisible until someone's
        // setting stops sticking.
        for setting in SETTINGS {
            let expected = format!("{}.{}", setting.namespace(), setting.path.join("."));
            assert_eq!(setting.key, expected, "{}", setting.key);
        }
    }

    #[test]
    fn a_path_is_not_empty_and_is_not_too_deep() {
        // Two levels is all any wlRIX config file has. A third would need the write path to
        // create intermediate tables it has never been asked to create.
        for setting in SETTINGS {
            assert!(!setting.path.is_empty(), "{}", setting.key);
            assert!(setting.path.len() <= 2, "{}", setting.key);
            for part in setting.path {
                assert!(!part.is_empty(), "{}", setting.key);
            }
        }
    }

    #[test]
    fn a_default_is_inside_its_own_range() {
        for setting in SETTINGS {
            match &setting.kind {
                Kind::Int { default, min, max } => {
                    assert!(min <= max, "{}", setting.key);
                    if let Some(value) = default {
                        assert!(value >= min && value <= max, "{}", setting.key);
                    }
                }
                Kind::Float { default, min, max } => {
                    assert!(min <= max, "{}", setting.key);
                    if let Some(value) = default {
                        assert!(value >= min && value <= max, "{}", setting.key);
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn an_enum_default_is_one_of_its_own_choices() {
        for setting in SETTINGS {
            if let Kind::Enum { default, choices } = &setting.kind {
                assert!(!choices.is_empty(), "{}", setting.key);
                if let Some(value) = default {
                    assert!(
                        choices.iter().any(|choice| choice.value == *value),
                        "{} defaults to {value:?}, which is not a choice",
                        setting.key
                    );
                }
            }
        }
    }

    #[test]
    fn a_live_setting_has_someone_to_tell() {
        // `Reload::Live` means "SIGHUP the owner and it takes effect". Without a pidfile there
        // is nothing to signal, so the pair is a contradiction and the write would silently do
        // half its job.
        for setting in SETTINGS {
            if setting.reload == Reload::Live {
                assert!(
                    setting.owner.pidfile().is_some(),
                    "{} is live but {} has no pidfile",
                    setting.key,
                    setting.owner
                );
            }
        }
    }

    #[test]
    fn only_numbers_carry_units() {
        for setting in SETTINGS {
            if setting.unit != Unit::None {
                assert!(
                    matches!(setting.kind, Kind::Int { .. } | Kind::Float { .. }),
                    "{} is a {} with a unit",
                    setting.key,
                    setting.kind.name()
                );
            }
        }
    }

    #[test]
    fn everything_says_what_it_is() {
        // The prose is the whole point of describing a setting rather than just typing it: a
        // panel with an empty label is not a panel.
        for setting in SETTINGS {
            assert!(!setting.summary.is_empty(), "{}", setting.key);
            assert!(!setting.description.is_empty(), "{}", setting.key);
        }
    }

    #[test]
    fn a_lists_default_is_the_empty_list() {
        // wlRIX reads an empty list and a missing key as the same thing everywhere (see
        // `wlrix-desktop`'s `open_command`, `wlrix-idle`'s `allow`/`deny`), so a list with a
        // non-empty default would be a rule this daemon invented on its own.
        for setting in SETTINGS {
            if let Kind::StrList { default } = &setting.kind {
                assert!(default.is_empty(), "{}", setting.key);
            }
        }
    }

    #[test]
    fn every_namespace_has_at_least_one_setting() {
        // An empty namespace would show up in `ListNamespaces` and then answer `DescribeAll`
        // with nothing, which reads as a broken daemon rather than a deliberate omission.
        for namespace in namespaces() {
            assert!(
                in_namespace(namespace).next().is_some(),
                "{namespace} has no settings"
            );
        }
    }

    #[test]
    fn lookup_finds_what_the_table_declares() {
        assert!(lookup("compositor.focus.policy").is_some());
        assert!(lookup("compositor.keyboard.layout").is_some());
        // Deliberately absent: an array-of-table path is not a scalar leaf.
        assert!(lookup("compositor.output.mode").is_none());
        assert!(lookup("session.app.command").is_none());
        assert!(lookup("nonsense").is_none());
    }
}
