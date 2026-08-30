// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading and writing the config files without spoiling them.
//!
//! This is the part of the daemon that exists to be *careful*. Every file it writes is one the
//! user also edits by hand, so a write has to come back looking like the file they wrote, with
//! one value different and nothing else touched -- not their comments, not their key order, not
//! the blank line they left between two sections.
//!
//! ## Why `toml_edit` and not the thing this replaces
//!
//! `toml` parses to a value and serializes from one; a round trip through it discards every
//! comment in the file. `toml_edit` parses to a document that remembers its own formatting and
//! lets one key be replaced in place, which is the whole requirement.
//!
//! The alternative was porting `Wlrix.Settings.Keyboard/Services/CompositorConfig.cs`, the
//! hand-rolled line editor this replaces. It is worth naming what that could not do, because it
//! is the shape of every "just parse the lines" config editor:
//!
//! - It matched a section header by exact string equality, so `[keyboard]  # japanese` was not
//!   found -- and the not-found branch **appends another `[keyboard]` table**. `toml` rejects a
//!   duplicate table, so the compositor then reports the whole file invalid and falls back to
//!   built-in defaults. One click, and the user's config is gone.
//! - It ended a section at any line starting with `[`, so a multi-line array value ended it
//!   early.
//! - It could not see a dotted key (`keyboard.layout = "jp"`), an inline table
//!   (`keyboard = { layout = "jp" }`) or a multi-line string -- all of which are ordinary TOML
//!   that every wlRIX component accepts.
//!
//! [`Document`] handles all of those, because `toml_edit` does.
//!
//! ## Seeding, and why a first write is not just a write
//!
//! wlRIX reads the first config file it finds and does **not** merge: `~/.config/wlrix/x.toml`
//! shadows `/etc/wlrix/x.toml` entirely. So creating a user file containing one key, while a
//! system file exists, silently discards every other value the administrator set. A write in
//! that situation therefore starts from the *system file's own text* -- comments and all -- and
//! applies the edit to that. See [`load_for_write`].

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use crate::paths::{File, Roots};

/// A value the daemon can read out of, or write into, a config file.
///
/// Deliberately smaller than TOML: these are the shapes [`crate::schema::Kind`] describes, and
/// anything else in a user's file is something the daemon has no opinion about and leaves
/// alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    StrList(Vec<String>),
}

impl Value {
    /// The value a TOML item holds, if it is one of the shapes we handle.
    fn from_toml(item: &Item) -> Option<Self> {
        let value = item.as_value()?;
        match value {
            toml_edit::Value::Boolean(v) => Some(Self::Bool(*v.value())),
            toml_edit::Value::Integer(v) => Some(Self::Int(*v.value())),
            toml_edit::Value::Float(v) => Some(Self::Float(*v.value())),
            toml_edit::Value::String(v) => Some(Self::Str(v.value().clone())),
            toml_edit::Value::Array(array) => {
                // All-strings or nothing. A mixed array is not a setting we describe, and
                // silently dropping the elements we did not understand would be worse than
                // saying we do not understand it.
                let mut items = Vec::with_capacity(array.len());
                for element in array {
                    items.push(element.as_str()?.to_owned());
                }
                Some(Self::StrList(items))
            }
            // Neither an inline table nor a date is a shape any wlRIX setting has. Answering
            // `None` is what lets a user keep such a value in their file untouched.
            toml_edit::Value::InlineTable(_) | toml_edit::Value::Datetime(_) => None,
        }
    }

    fn into_toml(self) -> toml_edit::Value {
        match self {
            Self::Bool(v) => v.into(),
            Self::Int(v) => v.into(),
            Self::Float(v) => v.into(),
            Self::Str(v) => v.into(),
            Self::StrList(items) => items.into_iter().collect::<toml_edit::Array>().into(),
        }
    }

    /// The name of the shape, for an error message that says what was offered.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "double",
            Self::Str(_) => "string",
            Self::StrList(_) => "string-list",
        }
    }
}

/// What can go wrong between a request and a file on disk.
#[derive(Debug)]
pub enum Error {
    /// The file exists but is not valid TOML. Carries the parser's own message, which is far
    /// more useful than anything this could say about it.
    Parse {
        path: PathBuf,
        message: String,
    },
    Read {
        path: PathBuf,
        message: String,
    },
    Write {
        path: PathBuf,
        message: String,
    },
    /// Neither `$XDG_CONFIG_HOME` nor `$HOME` is set, so there is nowhere to write.
    NoConfigHome,
    /// The path a setting names is occupied by a section, not a value. Overwriting it would
    /// delete whatever is inside.
    NotAValue {
        key: String,
    },
    /// The owning program's own parser refused the file we were about to install.
    ///
    /// This is the schema table having drifted from the type it describes, caught before it
    /// could cost anybody their config. See [`validate`].
    Refused {
        program: String,
        message: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { path, message } => {
                write!(f, "{} is not valid: {message}", path.display())
            }
            Self::Read { path, message } => {
                write!(f, "could not read {}: {message}", path.display())
            }
            Self::Write { path, message } => {
                write!(f, "could not write {}: {message}", path.display())
            }
            Self::NoConfigHome => {
                f.write_str("neither XDG_CONFIG_HOME nor HOME is set; there is nowhere to write")
            }
            Self::NotAValue { key } => {
                write!(f, "{key} names a table, not a value")
            }
            Self::Refused { program, message } => {
                write!(f, "{program} would not accept the change: {message}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A config file, parsed, with its formatting intact.
pub struct Document {
    doc: DocumentMut,
    /// Whether the file this came from used CRLF line endings.
    ///
    /// `toml_edit` keeps the endings of lines it does not touch, but writes `\n` for anything it
    /// inserts, which would leave a Windows-edited file with mixed endings. The C# editor this
    /// replaces got that right and the replacement must not regress it.
    crlf: bool,
}

impl Document {
    /// An empty document, for a config file that does not exist yet.
    pub fn empty() -> Self {
        Self {
            doc: DocumentMut::new(),
            crlf: false,
        }
    }

    pub fn parse(text: &str) -> Result<Self, toml_edit::TomlError> {
        Ok(Self {
            doc: text.parse::<DocumentMut>()?,
            crlf: text.contains("\r\n"),
        })
    }

    /// The value at `path`, if the file sets it to something we understand.
    ///
    /// A one-element path is a top-level key; two elements are a key inside a table. Both a
    /// `[table]` section and an inline `table = { … }` are read the same way, as is a dotted
    /// key -- `toml_edit` models all three as table-likes, which is exactly why the hand-rolled
    /// editor could not.
    pub fn get(&self, path: &[&str]) -> Option<Value> {
        Value::from_toml(self.item(path)?)
    }

    fn item(&self, path: &[&str]) -> Option<&Item> {
        match path {
            [key] => self.doc.get(key),
            [table, key] => self.doc.get(table)?.as_table_like()?.get(key),
            _ => None,
        }
    }

    /// Set `path` to `value`, creating the table if it is not there.
    ///
    /// An existing key keeps its own decor -- the spacing around the `=` and any trailing
    /// comment -- because that is the user's, not ours. The line
    /// `repeat_delay = 200  # feels right` becomes `repeat_delay = 300  # feels right` rather
    /// than losing the remark.
    pub fn set(&mut self, key_name: &str, path: &[&str], value: Value) -> Result<(), Error> {
        let new = value.into_toml();
        match path {
            [key] => set_in(self.doc.as_table_mut(), key_name, key, new),
            [name, key] => {
                let table = self.table_mut(key_name, name)?;
                set_in_table_like(table, key_name, key, new)
            }
            _ => Err(Error::NotAValue {
                key: key_name.to_owned(),
            }),
        }
    }

    /// Remove `path`, so the value falls back to whatever the owner's default is.
    ///
    /// Returns whether anything was there. An emptied table is **left in place**: an empty
    /// `[keyboard]` is valid TOML that every wlRIX component parses to its defaults, and
    /// removing a section the user wrote in order to tidy up after ourselves would be taking a
    /// liberty with their file that no setting change asked for.
    pub fn remove(&mut self, path: &[&str]) -> bool {
        match path {
            [key] => self.doc.as_table_mut().remove(key).is_some(),
            [name, key] => self
                .doc
                .get_mut(name)
                .and_then(Item::as_table_like_mut)
                .is_some_and(|table| table.remove(key).is_some()),
            _ => false,
        }
    }

    /// The table `name`, creating it as a real `[name]` section if it is absent.
    ///
    /// Explicitly rather than through `IndexMut`, which on a missing key creates an *inline*
    /// table -- `keyboard = { layout = "jp" }`. That is valid TOML and the compositor would
    /// read it, but it is not what anyone hand-editing the file expects to find, and it is not
    /// what the rest of their file looks like.
    fn table_mut(
        &mut self,
        key_name: &str,
        name: &str,
    ) -> Result<&mut dyn toml_edit::TableLike, Error> {
        if self.doc.get(name).is_none() {
            let mut table = Table::new();
            // Without this the header is not written at all: an implicit table is one that only
            // exists to hold `[a.b]`, and `[a]` never appears.
            table.set_implicit(false);
            self.doc.as_table_mut().insert(name, Item::Table(table));
        }
        self.doc
            .get_mut(name)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| Error::NotAValue {
                key: key_name.to_owned(),
            })
    }

    /// The document as text, ready to write.
    pub fn to_text(&self) -> String {
        let text = self.doc.to_string();
        if self.crlf {
            // Normalize rather than patch: lines `toml_edit` left alone already end in CRLF,
            // and anything it inserted ends in LF. Going through LF first makes the result
            // independent of which is which.
            return text.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        text
    }
}

fn set_in(
    table: &mut Table,
    key_name: &str,
    key: &str,
    new: toml_edit::Value,
) -> Result<(), Error> {
    match table.get_mut(key) {
        Some(item) => replace_value(item, key_name, new),
        None => {
            table.insert(key, Item::Value(new));
            Ok(())
        }
    }
}

fn set_in_table_like(
    table: &mut dyn toml_edit::TableLike,
    key_name: &str,
    key: &str,
    new: toml_edit::Value,
) -> Result<(), Error> {
    match table.get_mut(key) {
        Some(item) => replace_value(item, key_name, new),
        None => {
            table.insert(key, Item::Value(new));
            Ok(())
        }
    }
}

/// Put `new` where `item` is, keeping whatever decor the old value carried.
fn replace_value(item: &mut Item, key_name: &str, mut new: toml_edit::Value) -> Result<(), Error> {
    let Some(old) = item.as_value() else {
        // A `[keyboard.layout]` section where a value was expected. Replacing it would delete
        // everything inside, which is not a thing a settings change should be able to do.
        return Err(Error::NotAValue {
            key: key_name.to_owned(),
        });
    };
    *new.decor_mut() = old.decor().clone();
    *item = Item::Value(new);
    Ok(())
}

/// Read a config file for *reading*: whichever of the user's and the system's is in force.
///
/// A file that does not parse is an error rather than a silent fallback to defaults. Every
/// component treats a broken config that way for itself -- it warns and carries on -- but this
/// daemon must not: reporting a stale value as the current one, or writing on top of a file it
/// could not understand, are both worse than saying the file is broken.
pub fn load(roots: &Roots, file: File) -> Result<Option<Document>, Error> {
    let Some(path) = roots.effective_path(file) else {
        return Ok(None);
    };
    read_document(&path).map(Some)
}

/// Read a config file for *writing*, seeding from the system file when there is no user one.
///
/// The seeding is the whole subtlety here, and it is invisible until it bites: wlRIX takes the
/// first config file it finds and does not merge, so a user file created with a single key in
/// it shadows -- and therefore discards -- everything the administrator put in `/etc/wlrix`.
/// Starting from the system file's own text keeps their comments and their values, and adds
/// ours.
pub fn load_for_write(roots: &Roots, file: File) -> Result<Document, Error> {
    let user = roots.user_path(file).ok_or(Error::NoConfigHome)?;
    if user.is_file() {
        return read_document(&user);
    }
    let system = roots.system_path(file);
    if system.is_file() {
        return read_document(&system);
    }
    Ok(Document::empty())
}

fn read_document(path: &Path) -> Result<Document, Error> {
    let text = fs::read_to_string(path).map_err(|err| Error::Read {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    Document::parse(&text).map_err(|err| Error::Parse {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

/// Replace `path`'s contents with `text`, atomically.
///
/// Write a temporary file beside it, flush it to the disk, then rename over the target, and
/// flush the directory. The rename is what makes the swap atomic -- a reader either sees the
/// old file or the new one, never a half-written one -- and the two flushes are what keep that
/// true across a power cut.
///
/// `wlrix-compositor/src/outputs.rs` does the same dance without the flushes, and is right not
/// to: `outputs.toml` is machine-written state that is rebuilt from the hardware anyway. A
/// hand-owned config file is not, and losing one to a crash would not be recoverable by
/// anything.
///
/// `check` names the program whose own parser has the last word: before the rename, the
/// temporary file is handed to `<program> --check-config <path>`, and a refusal aborts the
/// write with the file untouched. See [`validate`] for why that is the load-bearing part of
/// this whole program.
///
/// Returns the bytes written, which the caller keeps so it can recognize its own change coming
/// back through the watch.
pub fn write_atomic(path: &Path, text: &str, check: Option<&str>) -> Result<Vec<u8>, Error> {
    let failed = |err: std::io::Error| Error::Write {
        path: path.to_path_buf(),
        message: err.to_string(),
    };

    let directory = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(directory).map_err(failed)?;

    // In the same directory, so the rename cannot cross a filesystem, and dot-prefixed so it is
    // hidden from a listing if we die before the rename. Named after the target rather than
    // replacing its extension: `compositor.tmp` beside `compositor.toml` is one letter away
    // from looking like a config file of its own.
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = directory.join(format!(".{name}.tmp"));

    let existing_mode = fs::metadata(path).ok().map(|meta| {
        use std::os::unix::fs::PermissionsExt as _;
        meta.permissions().mode()
    });

    // Leaving a stray dotfile in the user's config directory on every failed write would be its
    // own small mess, so every path out of here after this point clears it up.
    let cleanup = || {
        let _ = fs::remove_file(&temporary);
    };

    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        if let Some(mode) = existing_mode {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(err) = written {
        cleanup();
        return Err(failed(err));
    }

    // The owner's own parser has the last word, on a file that is still only a temporary.
    if let Some(program) = check
        && let Err(message) = validate(program, &temporary)
    {
        cleanup();
        return Err(Error::Refused {
            program: program.to_owned(),
            message,
        });
    }

    let committed = (|| -> std::io::Result<()> {
        fs::rename(&temporary, path)?;
        // The rename itself needs flushing, or the directory entry can be lost while the file
        // contents survive -- which presents as the config having vanished.
        fs::File::open(directory)?.sync_all()
    })();
    if let Err(err) = committed {
        cleanup();
        return Err(failed(err));
    }

    Ok(text.as_bytes().to_vec())
}

/// How long a `--check-config` is given before it is assumed not to be one.
///
/// Parsing a few kilobytes of TOML is microseconds. This is generous by four orders of
/// magnitude, and exists for the case in the doc comment below rather than for slow parsing.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Ask the owning program whether it would accept this file.
///
/// **This is the part that makes the daemon safe to trust.** `src/schema/table.rs` is a
/// hand-kept copy of what five other repos' serde types accept, and a hand-kept copy drifts.
/// With `#[serde(deny_unknown_fields)]` everywhere in wlRIX, a drifted key does not produce a
/// wrong setting -- it makes the owner reject the *whole file* and fall back to built-in
/// defaults, which is the user's entire configuration gone. Running their own parser over the
/// candidate before the rename puts correctness back with the type that defines it, and demotes
/// the schema table to what a UI needs: ranges, choices and prose.
///
/// Two ways it declines to answer, both of which are "carry on without the check" rather than
/// failures. A settings daemon that refused to write because the compositor was not installed
/// on `PATH` would be worse than one that wrote without asking.
///
/// - **The program is not there.** An unusual install, or a component the user does not have.
/// - **It did not exit.** The hazard worth naming: `wlrix-compositor` ignored unknown arguments
///   until this flag existed, so a *new* daemon against an *old* compositor would have started
///   a compositor rather than checked a file. Killing it after a timeout turns that from a
///   catastrophe into a log line. (The other three have always rejected unknown arguments.)
fn validate(program: &str, path: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let child = Command::new(program)
        .arg("--check-config")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("{program} is not installed; writing without checking");
            return Ok(());
        }
        Err(err) => {
            tracing::warn!("could not run {program} --check-config ({err}); writing anyway");
            return Ok(());
        }
    };

    let deadline = std::time::Instant::now() + CHECK_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!(
                    "{program} --check-config did not exit; it is probably too old to know the \
                     flag. Writing without checking."
                );
                return Ok(());
            }
            Err(err) => {
                tracing::warn!("could not wait for {program} ({err}); writing anyway");
                return Ok(());
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => {
            tracing::warn!("could not read {program}'s answer ({err}); writing anyway");
            return Ok(());
        }
    };
    if output.status.success() {
        return Ok(());
    }

    // Its own message, which names the line and the key. Anything this could say instead would
    // be a worse version of it.
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if message.is_empty() {
        format!("exit status {}", output.status)
    } else {
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Roots;

    fn set(text: &str, path: &[&str], value: Value) -> String {
        let mut doc = Document::parse(text).expect("should parse");
        doc.set("test.key", path, value).expect("should set");
        doc.to_text()
    }

    #[test]
    fn a_value_is_replaced_in_place() {
        let out = set(
            "[keyboard]\nlayout = \"us\"\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        assert_eq!(out, "[keyboard]\nlayout = \"jp\"\n");
    }

    #[test]
    fn a_section_header_with_a_comment_after_it_is_found() {
        // The exact regression in the C# editor this replaces: it compared the trimmed line to
        // "[keyboard]" and so did not find this one, then took the not-found branch and
        // appended a *second* `[keyboard]` table. `toml` rejects a duplicate table, so the
        // compositor reported the whole file invalid and fell back to built-in defaults -- one
        // click in the settings app, and the user's config was gone.
        let out = set(
            "[keyboard]  # japanese\nlayout = \"us\"\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        assert_eq!(out, "[keyboard]  # japanese\nlayout = \"jp\"\n");
        assert_eq!(out.matches("[keyboard]").count(), 1, "one table, not two");
        // And the proof that matters: it still parses.
        assert!(toml::from_str::<toml::Value>(&out).is_ok());
    }

    #[test]
    fn comments_and_unrelated_sections_survive() {
        let original = "\
# my compositor
# hands off

[keyboard]
# the layout I actually use
layout = \"us\"
model = \"pc105\"   # a trailing remark

[[output]]
name = \"DP-1\"
mode = \"2560x1440@144\"

[focus]
policy = \"pointer\"
";
        let out = set(original, &["keyboard", "layout"], Value::Str("jp".into()));
        assert!(out.contains("# my compositor"));
        assert!(out.contains("# hands off"));
        assert!(out.contains("# the layout I actually use"));
        assert!(out.contains("[[output]]"));
        assert!(out.contains("mode = \"2560x1440@144\""));
        assert!(out.contains("policy = \"pointer\""));
        assert!(out.contains("layout = \"jp\""));
        // Everything but the one line is byte-identical.
        assert_eq!(out, original.replace("layout = \"us\"", "layout = \"jp\""));
    }

    #[test]
    fn a_trailing_comment_on_the_value_is_kept() {
        // It is the user's remark about their own setting. The line editor this replaces threw
        // it away, because it rebuilt the whole line from scratch.
        let out = set(
            "[keyboard]\nrepeat_delay = 200  # feels right\n",
            &["keyboard", "repeat_delay"],
            Value::Int(300),
        );
        assert_eq!(out, "[keyboard]\nrepeat_delay = 300  # feels right\n");
    }

    #[test]
    fn key_order_is_not_disturbed() {
        let out = set(
            "[keyboard]\nrepeat_rate = 25\nlayout = \"us\"\nmodel = \"pc105\"\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        assert_eq!(
            out,
            "[keyboard]\nrepeat_rate = 25\nlayout = \"jp\"\nmodel = \"pc105\"\n"
        );
    }

    #[test]
    fn a_missing_key_is_appended_to_its_own_section() {
        let out = set(
            "[keyboard]\nlayout = \"jp\"\n\n[focus]\npolicy = \"click\"\n",
            &["keyboard", "model"],
            Value::Str("jp106".into()),
        );
        assert!(out.contains("model = \"jp106\""));
        // Inside `[keyboard]`, not after `[focus]` -- which is where a naive append would put
        // it, and would silently make it a focus setting.
        let parsed: toml::Value = toml::from_str(&out).expect("should parse");
        assert_eq!(parsed["keyboard"]["model"].as_str(), Some("jp106"));
        assert!(parsed["focus"].get("model").is_none());
    }

    #[test]
    fn a_missing_section_is_created_as_a_real_table() {
        let out = set(
            "[focus]\npolicy = \"pointer\"\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        // A `[keyboard]` header, not `keyboard = { layout = "jp" }` -- which is what
        // `toml_edit`'s IndexMut would have produced, and is not what the rest of the file
        // looks like.
        assert!(out.contains("[keyboard]"), "{out}");
        assert!(!out.contains('{'), "{out}");
        let parsed: toml::Value = toml::from_str(&out).expect("should parse");
        assert_eq!(parsed["keyboard"]["layout"].as_str(), Some("jp"));
        assert_eq!(parsed["focus"]["policy"].as_str(), Some("pointer"));
    }

    #[test]
    fn a_top_level_key_goes_before_the_sections() {
        // `desktop.toml` has both: `snap_to_grid` at the top level and `[metrics]` below it. A
        // bare key written after a section header would belong to that section, so this is not
        // tidiness -- it is the difference between a valid file and a wrong one.
        let out = set(
            "[metrics]\nicon = 64\n",
            &["snap_to_grid"],
            Value::Bool(true),
        );
        let parsed: toml::Value = toml::from_str(&out).expect("should parse");
        assert_eq!(parsed["snap_to_grid"].as_bool(), Some(true));
        assert!(parsed["metrics"].get("snap_to_grid").is_none(), "{out}");
    }

    #[test]
    fn an_empty_file_gets_a_whole_section() {
        let out = set("", &["keyboard", "layout"], Value::Str("jp".into()));
        let parsed: toml::Value = toml::from_str(&out).expect("should parse");
        assert_eq!(parsed["keyboard"]["layout"].as_str(), Some("jp"));
    }

    #[test]
    fn a_dotted_key_is_seen_and_written_where_it_already_is() {
        // Legal TOML that every wlRIX component accepts, and that the line editor this replaces
        // could not see at all -- it would have appended a second, conflicting `[keyboard]`.
        let doc = Document::parse("keyboard.layout = \"us\"\n").expect("should parse");
        assert_eq!(
            doc.get(&["keyboard", "layout"]),
            Some(Value::Str("us".into()))
        );

        let out = set(
            "keyboard.layout = \"us\"\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        assert_eq!(out, "keyboard.layout = \"jp\"\n");
    }

    #[test]
    fn an_inline_table_is_seen_and_written_where_it_already_is() {
        let out = set(
            "keyboard = { layout = \"us\", model = \"pc105\" }\n",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        );
        assert_eq!(out, "keyboard = { layout = \"jp\", model = \"pc105\" }\n");
    }

    #[test]
    fn a_multi_line_array_does_not_end_the_section_early() {
        // The line editor ended a section at any line starting with `[`, so the second element
        // below closed `[gamepad]` and `deny` was written into the wrong place.
        let out = set(
            "[gamepad]\nallow = [\n  \"Xbox\",\n  \"DualSense\",\n]\nenable = true\n",
            &["gamepad", "enable"],
            Value::Bool(false),
        );
        let parsed: toml::Value = toml::from_str(&out).expect("should parse");
        assert_eq!(parsed["gamepad"]["enable"].as_bool(), Some(false));
        assert_eq!(parsed["gamepad"]["allow"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn crlf_survives_a_round_trip() {
        // A file edited on Windows, or over a share. Mixed endings in one file are the failure
        // to avoid -- some editors show them as stray characters.
        let out = set(
            "[keyboard]\r\nlayout = \"us\"\r\n",
            &["keyboard", "model"],
            Value::Str("jp106".into()),
        );
        assert!(out.contains("model = \"jp106\""));
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    }

    #[test]
    fn an_lf_file_stays_lf() {
        let out = set(
            "[keyboard]\nlayout = \"us\"\n",
            &["keyboard", "model"],
            Value::Str("jp106".into()),
        );
        assert!(!out.contains('\r'));
    }

    #[test]
    fn every_kind_round_trips() {
        for value in [
            Value::Bool(true),
            Value::Int(-3),
            Value::Float(0.25),
            Value::Str("hello # not a comment".into()),
            Value::StrList(vec!["a".into(), "b \"quoted\"".into()]),
            Value::StrList(Vec::new()),
        ] {
            let out = set("", &["section", "key"], value.clone());
            let doc = Document::parse(&out).expect("should parse");
            assert_eq!(doc.get(&["section", "key"]), Some(value.clone()), "{out}");
            // And it is TOML the owner's parser would take.
            assert!(toml::from_str::<toml::Value>(&out).is_ok(), "{out}");
        }
    }

    #[test]
    fn a_hash_in_a_string_is_not_a_comment() {
        // The one place the hand-rolled editor's quote-aware comment stripper had to be right,
        // and the one place a naive one is wrong.
        let doc = Document::parse("[lock]\ncommand = \"swaylock -c #000000\"\n").unwrap();
        assert_eq!(
            doc.get(&["lock", "command"]),
            Some(Value::Str("swaylock -c #000000".into()))
        );
    }

    #[test]
    fn removing_a_key_leaves_the_section_and_the_rest() {
        let mut doc =
            Document::parse("[keyboard]\n# mine\nlayout = \"jp\"\nmodel = \"jp106\"\n").unwrap();
        assert!(doc.remove(&["keyboard", "layout"]));
        let out = doc.to_text();
        assert!(!out.contains("layout"), "{out}");
        assert!(out.contains("model = \"jp106\""), "{out}");
        assert!(out.contains("[keyboard]"), "{out}");
    }

    #[test]
    fn removing_the_last_key_keeps_the_empty_section() {
        // An empty `[keyboard]` parses to the defaults everywhere in wlRIX -- the compositor
        // has a test asserting exactly that for `[focus]`. Deleting a section the user wrote,
        // to tidy up after ourselves, is a liberty no setting change asked for.
        let mut doc = Document::parse("[keyboard]\nlayout = \"jp\"\n").unwrap();
        assert!(doc.remove(&["keyboard", "layout"]));
        let out = doc.to_text();
        assert!(out.contains("[keyboard]"), "{out}");
        assert!(toml::from_str::<toml::Value>(&out).is_ok());
    }

    #[test]
    fn removing_what_is_not_there_is_not_an_error() {
        let mut doc = Document::parse("[keyboard]\nlayout = \"jp\"\n").unwrap();
        assert!(!doc.remove(&["keyboard", "model"]));
        assert!(!doc.remove(&["focus", "policy"]));
        assert!(!doc.remove(&["nothing"]));
    }

    #[test]
    fn a_section_where_a_value_belongs_is_refused() {
        // `[keyboard.layout]` is a table. Writing a string over it would delete everything
        // inside, which no settings change should be able to do -- so it is an error, and the
        // user gets told what is in their way.
        let mut doc = Document::parse("[keyboard.layout]\nsomething = 1\n").unwrap();
        let err = doc
            .set(
                "compositor.keyboard.layout",
                &["keyboard", "layout"],
                Value::Str("jp".into()),
            )
            .expect_err("should refuse");
        assert!(matches!(err, Error::NotAValue { .. }), "{err:?}");
    }

    #[test]
    fn reading_a_value_we_do_not_model_answers_nothing() {
        let doc =
            Document::parse("[a]\nmixed = [1, \"two\"]\ninline = { x = 1 }\nwhen = 1979-05-27\n")
                .unwrap();
        assert_eq!(doc.get(&["a", "mixed"]), None);
        assert_eq!(doc.get(&["a", "inline"]), None);
        assert_eq!(doc.get(&["a", "when"]), None);
        // And a path that is not there at all.
        assert_eq!(doc.get(&["a", "absent"]), None);
        assert_eq!(doc.get(&["nowhere", "at", "all"]), None);
    }

    // --- The write path, on scratch files ---

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wlrix-settings-edit-{test}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("make the scratch directory");
        dir
    }

    #[test]
    fn an_atomic_write_leaves_the_contents_and_no_temporary() {
        let dir = scratch("atomic");
        let path = dir.join("compositor.toml");
        let bytes =
            write_atomic(&path, "[keyboard]\nlayout = \"jp\"\n", None).expect("should write");

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[keyboard]\nlayout = \"jp\"\n"
        );
        assert_eq!(bytes, b"[keyboard]\nlayout = \"jp\"\n");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "compositor.toml")
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_creates_the_directory_it_needs() {
        // A fresh install has no `~/.config/wlrix` at all, and the first setting anyone changes
        // must not fail because of it.
        let dir = scratch("makedir");
        let path = dir.join("wlrix").join("compositor.toml");
        write_atomic(&path, "x = 1\n", None).expect("should write");
        assert!(path.is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_first_write_is_seeded_from_the_system_file() {
        // The subtlety that is invisible until it bites. wlRIX takes the first config file it
        // finds and does not merge, so a user file created to hold one key shadows -- and
        // therefore discards -- everything the administrator put in /etc/wlrix.
        let dir = scratch("seeding");
        let user = dir.join("config/wlrix");
        let system = dir.join("etc/wlrix");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(
            system.join("compositor.toml"),
            "# set up by the administrator\n[keyboard]\nlayout = \"fr\"\nmodel = \"pc105\"\n\n\
             [focus]\npolicy = \"pointer\"\n",
        )
        .unwrap();
        let roots = Roots::new(Some(user.clone()), system);

        let mut doc = load_for_write(&roots, File::Compositor).expect("should load");
        doc.set(
            "compositor.keyboard.layout",
            &["keyboard", "layout"],
            Value::Str("jp".into()),
        )
        .unwrap();
        write_atomic(&user.join("compositor.toml"), &doc.to_text(), None).expect("should write");

        let written = fs::read_to_string(user.join("compositor.toml")).unwrap();
        assert!(written.contains("layout = \"jp\""), "{written}");
        assert!(written.contains("model = \"pc105\""), "{written}");
        assert!(written.contains("policy = \"pointer\""), "{written}");
        assert!(
            written.contains("# set up by the administrator"),
            "{written}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_files_at_all_a_write_starts_from_nothing() {
        let dir = scratch("first-ever");
        let roots = Roots::new(Some(dir.join("config/wlrix")), dir.join("etc/wlrix"));
        let doc = load_for_write(&roots, File::Compositor).expect("should load");
        assert_eq!(doc.to_text(), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_is_not_installed_does_not_stop_a_write() {
        // A settings daemon that refused to write because the compositor was not on PATH would
        // be worse than one that wrote without asking.
        let dir = scratch("no-such-program");
        let path = dir.join("compositor.toml");
        write_atomic(&path, "x = 1\n", Some("wlrix-no-such-program-exists"))
            .expect("should write anyway");
        assert!(path.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_refuses_leaves_the_file_alone() {
        // The whole point of the gate. `false` stands in for an owner whose parser rejected the
        // candidate: the real file must be untouched and the temporary must be gone.
        let dir = scratch("refused");
        let path = dir.join("compositor.toml");
        fs::write(&path, "# the original\n").unwrap();

        let err =
            write_atomic(&path, "nonsense = true\n", Some("false")).expect_err("should refuse");
        assert!(matches!(err, Error::Refused { .. }), "{err:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "# the original\n");
        assert!(
            !dir.join(".compositor.toml.tmp").exists(),
            "the candidate should have been cleaned up"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_accepts_lets_the_write_through() {
        let dir = scratch("accepted");
        let path = dir.join("compositor.toml");
        write_atomic(&path, "x = 1\n", Some("true")).expect("should write");
        assert_eq!(fs::read_to_string(&path).unwrap(), "x = 1\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_that_never_exits_is_killed_and_the_write_goes_ahead() {
        // The hazard this timeout exists for: `wlrix-compositor` ignored unknown arguments
        // until `--check-config` was added to it, so a new daemon against an old compositor
        // would have *started a compositor* instead of checking a file. `sleep` stands in for
        // that. It must cost a delay and a log line, not a wedged settings service.
        //
        // A script rather than a coreutil, because every coreutil that would hang rejects the
        // unknown option first -- which is exactly the behavior that makes the other three
        // components safe, and so cannot stand in for the one that was not.
        let dir = scratch("hangs");
        let hang = dir.join("pretends-to-be-a-compositor");
        fs::write(&hang, "#!/bin/sh\nsleep 60\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&hang, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let path = dir.join("compositor.toml");
        let started = std::time::Instant::now();
        write_atomic(&path, "x = 1\n", Some(&hang.to_string_lossy())).expect("should write anyway");
        assert!(path.is_file(), "the write must go ahead unchecked");
        assert!(
            started.elapsed() < CHECK_TIMEOUT * 3,
            "should not have waited for the child"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_keeps_the_files_existing_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch("mode");
        let path = dir.join("compositor.toml");
        fs::write(&path, "x = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&path, "x = 2\n", None).expect("should write");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a file the user made private must stay private"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
