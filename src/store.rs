// SPDX-License-Identifier: GPL-3.0-or-later
//! What every setting currently is, and the only path by which it changes.
//!
//! One `Mutex<Inner>` behind an `Arc`, the shape `wlrix-idle/src/dbus/mod.rs` uses: the D-Bus
//! interface has to answer a method call synchronously on whatever thread zbus dispatches it
//! on, and the watcher runs on the main thread, so the state belongs to neither of them.
//!
//! A write takes the lock for the whole read-modify-validate-write, which is a couple of
//! milliseconds of TOML editing and a rename. The portal's `async fn` machinery exists there
//! because `Start` waits on a person and must stay cancellable; nothing here waits on anything.
//!
//! ## Telling our own writes apart
//!
//! The daemon's own writes come back through the inotify watch looking exactly like a
//! hand-edit, and re-announcing them would have every settings panel echo its own change back
//! at itself. The discriminator is a **byte comparison**: the bytes just written are kept in
//! [`Inner::last_written`], recorded inside the same critical section as the rename, and a file
//! that comes back byte-identical is ours. Not a timestamp, which races; not a flag, which
//! leaks if the write fails. Config files are a few kilobytes, and the comparison is exact.
//!
//! ## A file that will not parse
//!
//! Reported, and then **nothing else happens**: the last-good values keep being served and the
//! owner is *not* signaled. Signaling it would be actively harmful -- the compositor's
//! reaction to a config it cannot parse is to fall back to built-in defaults, so poking it
//! about a file someone is halfway through editing would blank their whole configuration until
//! they finished.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::apply::{self, Outcome};
use crate::edit::{self, Document, Value};
use crate::paths::{File, Roots};
use crate::schema::{self, Kind, Setting};

/// Where a value came from, so a panel can gray out a Reset that would do nothing and be
/// honest about a value it cannot change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The user's own file.
    User,
    /// `/etc/wlrix`, because the user has no file of their own.
    System,
    /// Nothing sets it; this is the declared default.
    Default,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
            Self::Default => "default",
        }
    }
}

/// Why a request could not be carried out.
#[derive(Debug)]
pub enum Error {
    /// No such setting.
    UnknownKey(String),
    /// A setting the daemon deliberately does not describe yet, with the reason.
    Unsupported { key: String, why: &'static str },
    /// The right key, the wrong shape.
    WrongType {
        key: String,
        wanted: &'static str,
        got: &'static str,
    },
    /// A number outside the range the schema declares.
    OutOfRange {
        key: String,
        value: String,
        min: String,
        max: String,
    },
    /// A string that is not one of an enum's choices.
    InvalidValue {
        key: String,
        value: String,
        choices: Vec<&'static str>,
    },
    /// The file could not be read, parsed, or written.
    File(edit::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(f, "no such setting: {key}"),
            Self::Unsupported { key, why } => write!(f, "{key} is not settable here: {why}"),
            Self::WrongType { key, wanted, got } => {
                write!(f, "{key} is a {wanted}, not a {got}")
            }
            Self::OutOfRange {
                key,
                value,
                min,
                max,
            } => write!(f, "{key} must be between {min} and {max}, not {value}"),
            Self::InvalidValue {
                key,
                value,
                choices,
            } => write!(
                f,
                "{key} must be one of {}, not {value:?}",
                choices.join(", ")
            ),
            Self::File(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<edit::Error> for Error {
    fn from(err: edit::Error) -> Self {
        Self::File(err)
    }
}

/// Paths the daemon knows about but deliberately does not offer, and what to say instead.
///
/// Answering "no such setting" for these would be misleading -- they are real settings that a
/// person can put in the file by hand. Each is a keyed collection or a free-form map rather
/// than a scalar leaf, and needs add/remove/reorder rather than get/set; that surface is a
/// later version, and nothing in the current signatures blocks it.
const DEFERRED: &[(&str, &str)] = &[
    (
        "background.output",
        "a per-monitor wallpaper is a picture, a mode and a color that only mean anything \
         together with the connector name they hang off, so it is a collection rather than a \
         setting. Unlike the compositor's [[output]] this one would apply live, so it is first in \
         line when the collection surface lands. Edit background.toml by hand for now",
    ),
    (
        "compositor.output",
        "per-monitor settings are not written here. The machine-written outputs.toml is \
         layered on top of them, so a value set here is overridden the moment the compositor \
         next saves, and the compositor does not re-resolve outputs on reload in any case. Use \
         wlr-output-management, which the compositor implements and which applies live and \
         atomically",
    ),
    (
        "idle.timeout",
        "a countdown is several fields that only mean anything together, so it is a collection \
         rather than a setting. Edit idle.toml by hand for now",
    ),
    (
        "session.app",
        "the session's app list is a collection rather than a setting. Edit session.toml by \
         hand for now",
    ),
    (
        "session.env",
        "the session's environment is a free-form map with no fixed keys to describe. Edit \
         session.toml by hand for now",
    ),
    (
        "portal.preview.tile",
        "the thumbnail size is a fixed-length pair of numbers, which none of the value shapes \
         here describes. Edit portal.toml by hand for now",
    ),
];

/// What a write, or a change seen on disk, amounts to.
#[derive(Debug, Default)]
pub struct Changed {
    /// The keys whose effective value is now different, and what it is now.
    pub values: BTreeMap<&'static str, Value>,
    /// What became of the change, per owning program.
    pub outcomes: BTreeMap<&'static str, Outcome>,
}

/// Something the watcher noticed that subscribers should hear about.
#[derive(Debug)]
pub enum Notice {
    Changed(Changed),
    /// A file was hand-edited into something that will not parse.
    Invalid {
        namespace: &'static str,
        path: PathBuf,
        message: String,
    },
    /// ...and then fixed.
    Recovered {
        namespace: &'static str,
        path: PathBuf,
    },
}

struct Inner {
    roots: Roots,
    /// The effective value of every declared key that has one.
    ///
    /// A key with no value in the file *and* no declared default -- `keyboard.layout`, where
    /// absent means "let libxkbcommon decide" -- is simply not in here.
    values: BTreeMap<&'static str, Value>,
    sources: BTreeMap<&'static str, Source>,
    /// Files that would not parse, and the parser's own message.
    invalid: BTreeMap<File, (PathBuf, String)>,
    /// The exact bytes of the last write the daemon made to each path.
    last_written: HashMap<PathBuf, Vec<u8>>,
}

pub struct Store {
    inner: Mutex<Inner>,
}

impl Store {
    /// Read every config file and work out what everything currently is.
    pub fn load(roots: Roots) -> Arc<Self> {
        let store = Arc::new(Self {
            inner: Mutex::new(Inner {
                roots,
                values: BTreeMap::new(),
                sources: BTreeMap::new(),
                invalid: BTreeMap::new(),
                last_written: HashMap::new(),
            }),
        });
        let mut inner = store.lock();
        for file in crate::paths::ALL {
            reread(&mut inner, *file);
        }
        drop(inner);
        store
    }

    /// A poisoned mutex means a method handler panicked mid-update.
    ///
    /// What it holds is a cache of what is on disk plus a map of recently written bytes;
    /// neither can be left half-written in a way that matters, and carrying on with it is
    /// better than taking the session's settings service down. Same reasoning as
    /// `wlrix-idle`'s `with_inhibits`.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(inner) => inner,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The effective value of one key.
    pub fn get(&self, key: &str) -> Result<Option<Value>, Error> {
        let setting = resolve(key)?;
        Ok(self.lock().values.get(setting.key).cloned())
    }

    /// Every key in a namespace that has a value, effective values.
    pub fn get_all(&self, namespace: &str) -> BTreeMap<&'static str, Value> {
        let inner = self.lock();
        schema::in_namespace(namespace)
            .filter_map(|setting| {
                inner
                    .values
                    .get(setting.key)
                    .map(|value| (setting.key, value.clone()))
            })
            .collect()
    }

    /// Where each key in a namespace gets its value from.
    pub fn sources(&self, namespace: &str) -> BTreeMap<&'static str, Source> {
        let inner = self.lock();
        schema::in_namespace(namespace)
            .map(|setting| {
                (
                    setting.key,
                    inner
                        .sources
                        .get(setting.key)
                        .copied()
                        .unwrap_or(Source::Default),
                )
            })
            .collect()
    }

    /// Whether a file is currently unparseable, and why.
    pub fn invalid(&self) -> Vec<(&'static str, PathBuf, String)> {
        self.lock()
            .invalid
            .iter()
            .map(|(file, (path, message))| (file.namespace(), path.clone(), message.clone()))
            .collect()
    }

    /// Write several settings at once, then tell their owners.
    ///
    /// One call is one transaction **per file**: every key destined for the same file is
    /// applied to one document and written once, and every owner is signaled once however
    /// many of its settings changed. That is what keeps a settings panel applying four
    /// keyboard fields from making the compositor recompile its keymap four times.
    ///
    /// Validation happens for *all* keys before *any* file is written, so a batch with one bad
    /// value leaves nothing half-applied. Across files there is no such guarantee and none is
    /// offered: two files cannot be renamed atomically, and an API that implied otherwise
    /// would be lying.
    pub fn set_many(&self, requested: &[(String, Value)]) -> Result<Changed, Error> {
        let mut wanted: Vec<(&'static Setting, Value)> = Vec::with_capacity(requested.len());
        for (key, value) in requested {
            let setting = resolve(key)?;
            wanted.push((setting, coerce(setting, value.clone())?));
        }
        self.commit(
            wanted
                .into_iter()
                .map(|(setting, value)| (setting, Some(value))),
        )
    }

    /// Remove settings from the file, so each falls back to its default.
    ///
    /// Deleting the key rather than writing the default in its place. With
    /// `#[serde(deny_unknown_fields)]` and `Option` fields throughout wlRIX, an absent key *is*
    /// the default -- and writing today's default literally would pin it, so a later change to
    /// what the default means would silently not reach anyone who had ever pressed Reset.
    pub fn reset(&self, keys: &[String]) -> Result<Changed, Error> {
        let mut wanted = Vec::with_capacity(keys.len());
        for key in keys {
            wanted.push((resolve(key)?, None));
        }
        self.commit(wanted.into_iter())
    }

    /// Remove every setting in a namespace.
    pub fn reset_namespace(&self, namespace: &str) -> Result<Changed, Error> {
        if File::from_namespace(namespace).is_none() {
            return Err(Error::UnknownKey(namespace.to_owned()));
        }
        self.commit(schema::in_namespace(namespace).map(|setting| (setting, None)))
    }

    /// Apply a set of edits: group by file, write each once, signal each owner once.
    fn commit(
        &self,
        edits: impl Iterator<Item = (&'static Setting, Option<Value>)>,
    ) -> Result<Changed, Error> {
        let mut by_file: BTreeMap<File, Vec<(&'static Setting, Option<Value>)>> = BTreeMap::new();
        for (setting, value) in edits {
            by_file
                .entry(setting.file)
                .or_default()
                .push((setting, value));
        }

        let mut inner = self.lock();
        let mut touched = BTreeSet::new();

        for (file, edits) in &by_file {
            let mut document = edit::load_for_write(&inner.roots, *file)?;
            let mut any = false;
            for (setting, value) in edits {
                match value {
                    Some(value) => {
                        document.set(setting.key, setting.path, value.clone())?;
                        any = true;
                    }
                    None => any |= document.remove(setting.path),
                }
            }
            // A Reset of keys that were not set changes nothing. Writing the file anyway would
            // create a user file that shadows the system one purely to hold nothing.
            if !any {
                continue;
            }

            let path = inner
                .roots
                .user_path(*file)
                .ok_or(edit::Error::NoConfigHome)?;
            // Hand the candidate to the program that will have to read it, before the rename.
            // Every setting in one file has the same owner, so the first names it.
            let owner = edits
                .first()
                .and_then(|(setting, _)| setting.owner.program());
            let bytes = edit::write_atomic(&path, &document.to_text(), owner)?;
            tracing::info!("wrote {}", path.display());
            inner.last_written.insert(path, bytes);
            touched.insert(*file);
        }

        // Re-read rather than assuming: what a key is *now* depends on the file as a whole, and
        // a Reset falls back to whatever is underneath rather than to what we just removed.
        let mut changed = Changed::default();
        for file in &touched {
            let before = snapshot(&inner, *file);
            reread(&mut inner, *file);
            changed.values.extend(difference(&before, &inner, *file));
        }

        // One signal per owner, after every file is on disk -- so a batch spanning two of the
        // compositor's sections cannot have it read the first while the second is still a
        // temporary file.
        for setting in by_file
            .values()
            .flatten()
            .map(|(setting, _)| setting)
            .filter(|setting| touched.contains(&setting.file))
        {
            let program = setting.owner.program().unwrap_or("");
            changed
                .outcomes
                .entry(program)
                .or_insert_with(|| apply::notify(setting.owner, setting.reload));
        }

        Ok(changed)
    }

    /// Re-read the files a watch says changed, and work out what to announce.
    ///
    /// Returns nothing at all for the daemon's own writes -- see the module docs.
    pub fn refresh(&self, files: &BTreeSet<File>) -> Vec<Notice> {
        let mut inner = self.lock();
        let mut notices = Vec::new();
        let mut changed = Changed::default();

        for file in files {
            if is_our_own_write(&mut inner, *file) {
                tracing::debug!("{} came back as we wrote it", file.namespace());
                continue;
            }

            let was_invalid = inner.invalid.contains_key(file);
            let before = snapshot(&inner, *file);
            reread(&mut inner, *file);

            match (was_invalid, inner.invalid.get(file)) {
                (false, Some((path, message))) => {
                    tracing::warn!("{} is not valid: {message}", path.display());
                    notices.push(Notice::Invalid {
                        namespace: file.namespace(),
                        path: path.clone(),
                        message: message.clone(),
                    });
                    // No values to announce and, deliberately, no owner to signal: the
                    // compositor's answer to a config it cannot parse is to fall back to its
                    // built-in defaults, so poking it about a file someone is halfway through
                    // editing would blank their configuration until they finished.
                    continue;
                }
                (true, None) => {
                    let path = inner.roots.effective_path(*file).unwrap_or_default();
                    tracing::info!("{} is valid again", path.display());
                    notices.push(Notice::Recovered {
                        namespace: file.namespace(),
                        path,
                    });
                }
                // Still broken, or never was.
                _ => {}
            }
            if inner.invalid.contains_key(file) {
                continue;
            }

            let difference = difference(&before, &inner, *file);
            if difference.is_empty() {
                continue;
            }
            changed.values.extend(difference);

            // Somebody edited the file by hand and did not signal anyone. Doing it for them is
            // the point: the compositor's own README tells people to send it SIGHUP, and this
            // means they no longer have to.
            for setting in schema::in_namespace(file.namespace()) {
                let program = setting.owner.program().unwrap_or("");
                changed
                    .outcomes
                    .entry(program)
                    .or_insert_with(|| apply::notify(setting.owner, setting.reload));
            }
        }

        if !changed.values.is_empty() {
            notices.push(Notice::Changed(changed));
        }
        notices
    }
}

/// The setting a key names, with a useful answer for the ones we deliberately do not offer.
fn resolve(key: &str) -> Result<&'static Setting, Error> {
    if let Some(setting) = schema::lookup(key) {
        return Ok(setting);
    }
    for (prefix, why) in DEFERRED {
        if key == *prefix || key.starts_with(&format!("{prefix}.")) {
            return Err(Error::Unsupported {
                key: key.to_owned(),
                why,
            });
        }
    }
    Err(Error::UnknownKey(key.to_owned()))
}

/// Check a value against what the schema declares, and coerce where it is safe to.
fn coerce(setting: &'static Setting, value: Value) -> Result<Value, Error> {
    let wrong = |got: &'static str| Error::WrongType {
        key: setting.key.to_owned(),
        wanted: setting.kind.name(),
        got,
    };

    match (&setting.kind, value) {
        (Kind::Bool { .. }, Value::Bool(v)) => Ok(Value::Bool(v)),
        (Kind::Int { min, max, .. }, Value::Int(v)) => {
            if v < *min || v > *max {
                return Err(Error::OutOfRange {
                    key: setting.key.to_owned(),
                    value: v.to_string(),
                    min: min.to_string(),
                    max: max.to_string(),
                });
            }
            Ok(Value::Int(v))
        }
        // A whole number is a perfectly good float, and it is what a client sends when the user
        // types `0` into a box for a fraction. TOML itself is stricter -- `0` and `0.0` are
        // different types and `deadzone = 0` would be refused by the owner's parser -- which is
        // exactly why the conversion has to happen here rather than being passed through.
        (Kind::Float { min, max, .. }, value @ (Value::Float(_) | Value::Int(_))) => {
            let v = match value {
                Value::Float(v) => v,
                Value::Int(v) => v as f64,
                _ => unreachable!(),
            };
            if !v.is_finite() || v < *min || v > *max {
                return Err(Error::OutOfRange {
                    key: setting.key.to_owned(),
                    value: v.to_string(),
                    min: min.to_string(),
                    max: max.to_string(),
                });
            }
            Ok(Value::Float(v))
        }
        (Kind::Str { .. }, Value::Str(v)) => Ok(Value::Str(v)),
        (Kind::Enum { choices, .. }, Value::Str(v)) => {
            if choices.iter().any(|choice| choice.value == v) {
                return Ok(Value::Str(v));
            }
            // The case this is really here for: the compositor's own tests assert that
            // "explicit" -- Motif's name for click-to-focus, and the word an IRIX hand would
            // reach for -- and a capitalized "Pointer" are both refused. Catching them here
            // means a clear message instead of a file its owner will not parse.
            Err(Error::InvalidValue {
                key: setting.key.to_owned(),
                value: v,
                choices: choices.iter().map(|choice| choice.value).collect(),
            })
        }
        (Kind::StrList { .. }, Value::StrList(v)) => Ok(Value::StrList(v)),
        (_, value) => Err(wrong(value.kind_name())),
    }
}

/// What the file says a key is, as the declared kind, or `None` if it does not say.
fn value_of(document: &Document, setting: &Setting) -> Option<Value> {
    let raw = document.get(setting.path)?;
    match (&setting.kind, raw) {
        (Kind::Bool { .. }, value @ Value::Bool(_)) => Some(value),
        (Kind::Int { .. }, value @ Value::Int(_)) => Some(value),
        (Kind::Float { .. }, Value::Int(v)) => Some(Value::Float(v as f64)),
        (Kind::Float { .. }, value @ Value::Float(_)) => Some(value),
        (Kind::Str { .. } | Kind::Enum { .. }, value @ Value::Str(_)) => Some(value),
        (Kind::StrList { .. }, value @ Value::StrList(_)) => Some(value),
        // The file holds something of the wrong shape. The owner's parser will reject the whole
        // file for the same reason, so this is reported as unset rather than guessed at.
        (_, other) => {
            tracing::debug!(
                "{} is a {} in the file, not a {}",
                setting.key,
                other.kind_name(),
                setting.kind.name()
            );
            None
        }
    }
}

/// The declared default, for a setting that has one.
pub fn default_value(setting: &Setting) -> Option<Value> {
    match &setting.kind {
        Kind::Bool { default } => default.map(Value::Bool),
        Kind::Int { default, .. } => default.map(Value::Int),
        Kind::Float { default, .. } => default.map(Value::Float),
        Kind::Str { default } | Kind::Enum { default, .. } => {
            default.map(|value| Value::Str(value.to_owned()))
        }
        Kind::StrList { default } => Some(Value::StrList(
            default.iter().map(|item| (*item).to_owned()).collect(),
        )),
    }
}

/// Re-read one file and update every value it decides.
fn reread(inner: &mut Inner, file: File) {
    let effective = inner.roots.effective_path(file);
    let from_user = inner.roots.is_user_file(file);

    let document = match edit::load(&inner.roots, file) {
        Ok(document) => {
            inner.invalid.remove(&file);
            document
        }
        Err(err) => {
            let (path, message) = match err {
                edit::Error::Parse { path, message } | edit::Error::Read { path, message } => {
                    (path, message)
                }
                other => (effective.clone().unwrap_or_default(), other.to_string()),
            };
            inner.invalid.insert(file, (path, message));
            // Leave the last-good values in place. A panel showing the value from before
            // someone broke their file is far more use than one showing nothing.
            return;
        }
    };

    for setting in schema::in_namespace(file.namespace()) {
        let from_file = document.as_ref().and_then(|doc| value_of(doc, setting));
        let (value, source) = match from_file {
            Some(value) => (
                Some(value),
                if from_user {
                    Source::User
                } else {
                    Source::System
                },
            ),
            None => (default_value(setting), Source::Default),
        };
        match value {
            Some(value) => {
                inner.values.insert(setting.key, value);
            }
            // No value in the file and no default to name: `keyboard.layout`, where absent
            // means "let libxkbcommon decide" and there is no string that says so.
            None => {
                inner.values.remove(setting.key);
            }
        }
        inner.sources.insert(setting.key, source);
    }
}

/// The values of one file's keys, for diffing against what they become.
fn snapshot(inner: &Inner, file: File) -> BTreeMap<&'static str, Option<Value>> {
    schema::in_namespace(file.namespace())
        .map(|setting| (setting.key, inner.values.get(setting.key).cloned()))
        .collect()
}

/// The keys of `file` whose value is not what it was, and what it is now.
///
/// A key that lost its value -- reset, with no default to fall back to -- is announced as its
/// declared default when it has one, and simply omitted when it does not. A client sees the
/// value it should now display rather than a hole.
fn difference(
    before: &BTreeMap<&'static str, Option<Value>>,
    inner: &Inner,
    file: File,
) -> BTreeMap<&'static str, Value> {
    let mut changed = BTreeMap::new();
    for setting in schema::in_namespace(file.namespace()) {
        let now = inner.values.get(setting.key);
        if before.get(setting.key).map(Option::as_ref) == Some(now) {
            continue;
        }
        if let Some(value) = now {
            changed.insert(setting.key, value.clone());
        }
    }
    changed
}

/// Whether a file on disk is byte-for-byte what the daemon last wrote there.
///
/// Consuming the record as it matches: the write has been accounted for, and holding onto the
/// bytes would only mean a later hand-edit that happened to undo itself exactly was also
/// mistaken for ours.
fn is_our_own_write(inner: &mut Inner, file: File) -> bool {
    let Some(path) = inner.roots.user_path(file) else {
        return false;
    };
    let Some(expected) = inner.last_written.get(&path) else {
        return false;
    };
    match std::fs::read(&path) {
        Ok(actual) if actual == *expected => {
            inner.last_written.remove(&path);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compositor() -> &'static Setting {
        schema::lookup("compositor.focus.policy").unwrap()
    }

    /// A store over scratch directories, and the roots it was built on.
    ///
    /// Not the environment: `set_var` is racy in a test binary that runs its tests on several
    /// threads, and this is the code whose bugs would cost somebody their config file, so it
    /// has to be testable for real rather than only in its pure parts.
    fn scratch(test: &str) -> (Arc<Store>, Roots, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wlrix-settings-store-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        let user = dir.join("config/wlrix");
        let system = dir.join("etc/wlrix");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let roots = Roots::new(Some(user), system);
        (Store::load(roots.clone()), roots, dir)
    }

    fn set(store: &Store, key: &str, value: Value) -> Changed {
        store
            .set_many(&[(key.to_owned(), value)])
            .unwrap_or_else(|err| panic!("{key}: {err}"))
    }

    #[test]
    fn a_fresh_session_reports_the_declared_defaults() {
        let (store, _, dir) = scratch("defaults");
        assert_eq!(
            store.get("compositor.focus.policy").unwrap(),
            Some(Value::Str("click".into()))
        );
        assert_eq!(
            store.get("compositor.keyboard.repeat_delay").unwrap(),
            Some(Value::Int(200))
        );
        // The one with no default the daemon can name: absent means "let libxkbcommon decide".
        assert_eq!(store.get("compositor.keyboard.layout").unwrap(), None);
        assert_eq!(
            store.sources("compositor")["compositor.focus.policy"],
            Source::Default
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_lands_in_the_file_and_comes_back_out() {
        let (store, roots, dir) = scratch("round-trip");
        let changed = set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );

        assert_eq!(
            changed.values["compositor.focus.policy"],
            Value::Str("pointer".into())
        );
        assert_eq!(
            store.get("compositor.focus.policy").unwrap(),
            Some(Value::Str("pointer".into()))
        );
        assert_eq!(
            store.sources("compositor")["compositor.focus.policy"],
            Source::User
        );
        // And the file is TOML the compositor's own parser would take.
        let text = std::fs::read_to_string(roots.user_path(File::Compositor).unwrap()).unwrap();
        let parsed: toml::Value = toml::from_str(&text).expect("should parse");
        assert_eq!(parsed["focus"]["policy"].as_str(), Some("pointer"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setting_several_keys_writes_the_file_once_and_signals_the_owner_once() {
        // The whole point of SetMany: a settings panel applying four keyboard fields must not
        // make the compositor recompile its keymap four times, and today's separate
        // write-then-reload is a real race -- the SIGHUP can land against a file that was
        // replaced again in between.
        let (store, roots, dir) = scratch("batch");
        let changed = store
            .set_many(&[
                ("compositor.keyboard.layout".into(), Value::Str("jp".into())),
                (
                    "compositor.keyboard.model".into(),
                    Value::Str("jp106".into()),
                ),
                ("compositor.keyboard.repeat_delay".into(), Value::Int(300)),
                (
                    "compositor.focus.policy".into(),
                    Value::Str("pointer".into()),
                ),
            ])
            .expect("should apply");

        assert_eq!(changed.values.len(), 4);
        // One owner, therefore one outcome, however many keys were in the batch.
        assert_eq!(changed.outcomes.len(), 1);
        assert!(changed.outcomes.contains_key("wlrix-compositor"));

        let text = std::fs::read_to_string(roots.user_path(File::Compositor).unwrap()).unwrap();
        let parsed: toml::Value = toml::from_str(&text).expect("should parse");
        assert_eq!(parsed["keyboard"]["layout"].as_str(), Some("jp"));
        assert_eq!(parsed["keyboard"]["model"].as_str(), Some("jp106"));
        assert_eq!(parsed["keyboard"]["repeat_delay"].as_integer(), Some(300));
        assert_eq!(parsed["focus"]["policy"].as_str(), Some("pointer"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_batch_spanning_two_files_signals_both_owners() {
        let (store, roots, dir) = scratch("two-files");
        let changed = store
            .set_many(&[
                (
                    "compositor.focus.policy".into(),
                    Value::Str("pointer".into()),
                ),
                ("idle.gamepad.enable".into(), Value::Bool(false)),
            ])
            .expect("should apply");

        assert_eq!(changed.outcomes.len(), 2);
        assert!(roots.user_path(File::Compositor).unwrap().is_file());
        assert!(roots.user_path(File::Idle).unwrap().is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_bad_value_leaves_the_whole_batch_unwritten() {
        // Validation happens for every key before any file is touched, so a panel that sends a
        // typo does not end up with half its settings applied.
        let (store, roots, dir) = scratch("atomic-batch");
        let err = store
            .set_many(&[
                (
                    "compositor.focus.policy".into(),
                    Value::Str("pointer".into()),
                ),
                ("compositor.keyboard.repeat_rate".into(), Value::Int(9999)),
            ])
            .expect_err("should refuse");
        assert!(matches!(err, Error::OutOfRange { .. }), "{err:?}");
        assert!(
            !roots.user_path(File::Compositor).unwrap().exists(),
            "nothing should have been written"
        );
        assert_eq!(
            store.get("compositor.focus.policy").unwrap(),
            Some(Value::Str("click".into()))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reset_falls_back_to_the_default_rather_than_writing_it() {
        let (store, roots, dir) = scratch("reset");
        set(&store, "compositor.keyboard.repeat_delay", Value::Int(500));

        let changed = store
            .reset(&["compositor.keyboard.repeat_delay".into()])
            .expect("should reset");
        assert_eq!(
            changed.values["compositor.keyboard.repeat_delay"],
            Value::Int(200),
            "the client should be told the value it must now display"
        );

        // The key is gone from the file, not set to 200. Writing today's default literally
        // would pin it, so a later change to what the default means would never reach anyone
        // who had pressed Reset.
        let text = std::fs::read_to_string(roots.user_path(File::Compositor).unwrap()).unwrap();
        assert!(!text.contains("repeat_delay"), "{text}");
        assert_eq!(
            store.sources("compositor")["compositor.keyboard.repeat_delay"],
            Source::Default
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resetting_what_was_never_set_writes_nothing_at_all() {
        // Otherwise a Reset on a fresh session would create a user file whose only purpose is
        // to shadow the system one -- and shadowing is total here, so that would quietly
        // discard every value the administrator set.
        let (_, roots, dir) = scratch("empty-reset");
        std::fs::write(
            roots.system_path(File::Compositor),
            "[keyboard]\nlayout = \"fr\"\n",
        )
        .unwrap();
        let store = Store::load(roots.clone());

        let changed = store.reset(&["compositor.focus.policy".into()]).unwrap();
        assert!(changed.values.is_empty());
        assert!(
            !roots.user_path(File::Compositor).unwrap().exists(),
            "no user file should have been created"
        );
        assert_eq!(
            store.get("compositor.keyboard.layout").unwrap(),
            Some(Value::Str("fr".into())),
            "the system value must still be in force"
        );
        drop(store);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_system_file_is_reported_as_the_system_and_seeded_on_first_write() {
        let (_, roots, dir) = scratch("system");
        std::fs::write(
            roots.system_path(File::Compositor),
            "# the administrator's\n[keyboard]\nlayout = \"fr\"\nmodel = \"pc105\"\n",
        )
        .unwrap();
        let store = Store::load(roots.clone());

        assert_eq!(
            store.get("compositor.keyboard.layout").unwrap(),
            Some(Value::Str("fr".into()))
        );
        assert_eq!(
            store.sources("compositor")["compositor.keyboard.layout"],
            Source::System
        );

        set(
            &store,
            "compositor.keyboard.layout",
            Value::Str("jp".into()),
        );
        let text = std::fs::read_to_string(roots.user_path(File::Compositor).unwrap()).unwrap();
        assert!(text.contains("layout = \"jp\""), "{text}");
        // Everything else the administrator set came along, because a user file shadows the
        // system one outright rather than merging with it.
        assert!(text.contains("model = \"pc105\""), "{text}");
        assert!(text.contains("# the administrator's"), "{text}");
        assert_eq!(
            store.sources("compositor")["compositor.keyboard.model"],
            Source::User
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_write_coming_back_is_not_announced() {
        // The echo the whole byte-comparison exists for: without it every settings panel would
        // hear its own change announced back at it and fight its own debounce.
        let (store, _, dir) = scratch("echo");
        set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );

        let notices = store.refresh(&BTreeSet::from([File::Compositor]));
        assert!(notices.is_empty(), "{notices:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_edit_is_announced_with_only_the_keys_that_moved() {
        let (store, roots, dir) = scratch("hand-edit");
        set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );
        let _ = store.refresh(&BTreeSet::from([File::Compositor]));

        // Someone opens the file and changes one thing.
        let path = roots.user_path(File::Compositor).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace("pointer", "click") + "[keyboard]\nlayout = \"jp\"\n",
        )
        .unwrap();

        let notices = store.refresh(&BTreeSet::from([File::Compositor]));
        let Some(Notice::Changed(changed)) = notices.first() else {
            panic!("{notices:?}");
        };
        assert_eq!(
            changed.values.keys().copied().collect::<Vec<_>>(),
            ["compositor.focus.policy", "compositor.keyboard.layout"],
            "only what actually moved"
        );
        assert_eq!(
            changed.values["compositor.keyboard.layout"],
            Value::Str("jp".into())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewriting_a_file_without_changing_a_value_announces_nothing() {
        // `touch`, or an editor saving a file nobody edited. The diff is against the values,
        // not against the bytes, so a reformat is correctly silent.
        let (store, roots, dir) = scratch("no-op-edit");
        set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );
        let _ = store.refresh(&BTreeSet::from([File::Compositor]));

        let path = roots.user_path(File::Compositor).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("# a new comment\n{text}")).unwrap();

        assert!(
            store
                .refresh(&BTreeSet::from([File::Compositor]))
                .is_empty()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_edited_into_nonsense_is_reported_and_the_last_good_values_are_kept() {
        let (store, roots, dir) = scratch("broken");
        set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );
        let _ = store.refresh(&BTreeSet::from([File::Compositor]));

        let path = roots.user_path(File::Compositor).unwrap();
        std::fs::write(&path, "[keyboard\nlayout = ").unwrap();
        let notices = store.refresh(&BTreeSet::from([File::Compositor]));
        assert!(
            matches!(notices.as_slice(), [Notice::Invalid { .. }]),
            "{notices:?}"
        );
        // Still serving what was last known good, rather than nothing or the defaults.
        assert_eq!(
            store.get("compositor.focus.policy").unwrap(),
            Some(Value::Str("pointer".into()))
        );
        assert_eq!(store.invalid().len(), 1);

        // And when they finish typing.
        std::fs::write(&path, "[focus]\npolicy = \"click\"\n").unwrap();
        let notices = store.refresh(&BTreeSet::from([File::Compositor]));
        assert!(
            matches!(
                notices.as_slice(),
                [Notice::Recovered { .. }, Notice::Changed(_)]
            ),
            "{notices:?}"
        );
        assert!(store.invalid().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hand_edit_that_only_reformats_is_still_not_our_write() {
        // The byte comparison must not be so eager that it swallows a real change which
        // happens to follow one of ours closely.
        let (store, roots, dir) = scratch("not-ours");
        set(
            &store,
            "compositor.focus.policy",
            Value::Str("pointer".into()),
        );

        let path = roots.user_path(File::Compositor).unwrap();
        std::fs::write(&path, "[focus]\npolicy = \"click\"\n").unwrap();
        let notices = store.refresh(&BTreeSet::from([File::Compositor]));
        assert!(
            matches!(notices.as_slice(), [Notice::Changed(_)]),
            "{notices:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deferred_section_says_what_to_do_instead() {
        // "No such setting" would be a lie: `[[output]]` is real, and someone can write it by
        // hand. The message has to send them to wlr-output-management rather than leaving them
        // to conclude the daemon is broken.
        for key in [
            "background.output",
            "background.output.image",
            "compositor.output",
            "compositor.output.mode",
            "idle.timeout.after_secs",
            "session.app.command",
            "session.env.QT_QPA_PLATFORM",
            "portal.preview.tile",
        ] {
            match resolve(key) {
                Err(Error::Unsupported { why, .. }) => assert!(!why.is_empty()),
                other => panic!("{key}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_key_that_is_not_a_setting_at_all_is_unknown() {
        for key in [
            "compositor.keyboard.layuot",
            "greeter.theme",
            "compositor",
            "",
        ] {
            assert!(
                matches!(resolve(key), Err(Error::UnknownKey(_))),
                "{key}: {:?}",
                resolve(key)
            );
        }
    }

    #[test]
    fn a_deferred_prefix_does_not_swallow_a_real_setting() {
        // `session.compositor` starts with neither `session.app` nor `session.env`, but a
        // sloppy prefix match on `session` would have caught it.
        assert!(resolve("session.compositor").is_ok());
        // Likewise `background.output` must not swallow `background.image`, which is the one
        // real setting in that namespace whose name is closest to it.
        assert!(resolve("background.image").is_ok());
    }

    #[test]
    fn the_wrong_shape_is_refused_with_both_names() {
        let err = coerce(compositor(), Value::Int(1)).expect_err("should refuse");
        match err {
            Error::WrongType { wanted, got, .. } => {
                assert_eq!(wanted, "enum");
                assert_eq!(got, "int");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_focus_policy_the_compositor_would_reject_is_refused_here_first() {
        // `wlrix-compositor/src/config.rs` has a test asserting that both of these are errors.
        // Catching them here is the difference between a clear message and a config file its
        // owner refuses to parse -- which, with deny_unknown_fields, costs the user the file.
        for bad in ["explicit", "Pointer", ""] {
            let err = coerce(compositor(), Value::Str(bad.into())).expect_err(bad);
            match err {
                Error::InvalidValue { choices, .. } => {
                    assert_eq!(choices, ["click", "pointer"]);
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
        assert!(coerce(compositor(), Value::Str("pointer".into())).is_ok());
    }

    #[test]
    fn a_number_outside_its_range_is_refused() {
        let rate = schema::lookup("compositor.keyboard.repeat_rate").unwrap();
        assert!(coerce(rate, Value::Int(25)).is_ok());
        assert!(coerce(rate, Value::Int(0)).is_ok(), "0 switches repeat off");
        assert!(matches!(
            coerce(rate, Value::Int(-1)),
            Err(Error::OutOfRange { .. })
        ));
        assert!(matches!(
            coerce(rate, Value::Int(1_000)),
            Err(Error::OutOfRange { .. })
        ));
    }

    #[test]
    fn a_whole_number_is_accepted_where_a_fraction_is_wanted() {
        // What a client sends when someone types `0` into a box for a fraction. TOML is
        // stricter than that -- `0` and `0.0` are different types, and `deadzone = 0` is a
        // parse error for the owner -- so the conversion has to happen before the write.
        let deadzone = schema::lookup("idle.gamepad.deadzone").unwrap();
        assert_eq!(
            coerce(deadzone, Value::Int(0)).unwrap_err().to_string(),
            "idle.gamepad.deadzone must be between 0.02 and 0.9, not 0"
        );
        assert_eq!(
            coerce(deadzone, Value::Float(0.5)).ok(),
            Some(Value::Float(0.5))
        );
        // A whole number inside the range converts rather than being refused for its shape.
        let interval = schema::lookup("idle.gamepad.min_interval_ms").unwrap();
        assert_eq!(
            coerce(interval, Value::Int(500)).ok(),
            Some(Value::Int(500))
        );
    }

    #[test]
    fn a_non_finite_number_is_refused() {
        // JSON has no way to write these but D-Bus doubles do, and one reaching a TOML file
        // would produce `inf`, which the owner's parser reads as a bare key and rejects.
        let deadzone = schema::lookup("idle.gamepad.deadzone").unwrap();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                matches!(
                    coerce(deadzone, Value::Float(bad)),
                    Err(Error::OutOfRange { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_default_is_reported_for_the_settings_that_have_one() {
        assert_eq!(
            default_value(schema::lookup("compositor.keyboard.repeat_delay").unwrap()),
            Some(Value::Int(200))
        );
        assert_eq!(
            default_value(compositor()),
            Some(Value::Str("click".into()))
        );
        assert_eq!(
            default_value(schema::lookup("desktop.open").unwrap()),
            Some(Value::StrList(Vec::new()))
        );
        // And absent for the one where "absent" is itself the meaning.
        assert_eq!(
            default_value(schema::lookup("compositor.keyboard.layout").unwrap()),
            None
        );
    }

    #[test]
    fn a_value_of_the_wrong_shape_in_the_file_reads_as_unset() {
        // The owner's parser will reject the whole file for the same reason, so guessing at
        // what was meant would be inventing a value nothing is actually using.
        let document = Document::parse("[focus]\npolicy = 3\n").unwrap();
        assert_eq!(value_of(&document, compositor()), None);
    }

    #[test]
    fn a_whole_number_in_the_file_reads_as_the_fraction_it_is() {
        // Someone hand-writing `deadzone = 0` has written something the owner will refuse, but
        // reporting it as unset would hide that from a panel showing the file's contents.
        let deadzone = schema::lookup("idle.gamepad.deadzone").unwrap();
        let document = Document::parse("[gamepad]\ndeadzone = 1\n").unwrap();
        assert_eq!(value_of(&document, deadzone), Some(Value::Float(1.0)));
    }
}
