// SPDX-License-Identifier: GPL-3.0-or-later
//! `com.wlrix.Settings1` -- the interface itself.
//!
//! ## Keys read like the file
//!
//! A key is `<namespace>.<toml path>`: `compositor.keyboard.layout`, `idle.lock.command`,
//! `desktop.metrics.icon`. The namespace is the config file's stem, so a key greps like the
//! file it is in -- and splitting at the first dot gives the (namespace, key) pair
//! xdg-desktop-portal's own Settings interface uses, which makes a future proxy a rename rather
//! than a redesign.
//!
//! ## Batches, not keys
//!
//! `SetMany` is the primitive and `Set` is sugar for a one-entry call, because a settings panel
//! applying four keyboard fields must not make the compositor recompile its keymap four times.
//! One call is one write per file and one signal per owner. It also removes a race that exists
//! today: `Wlrix.Settings.Keyboard` writes the file and *then* sends `SIGHUP`, two steps between
//! which the file can be replaced again.
//!
//! Writes are transactional **per file** -- every key is validated before any file is touched,
//! so a batch with one bad value leaves nothing half-applied. Across files there is no such
//! guarantee, and none is offered: two files cannot be renamed atomically, and an API implying
//! otherwise would be lying.
//!
//! ## Describing rather than assuming
//!
//! `Describe`/`DescribeAll` hand out the schema -- type, range, choices, unit, default, prose --
//! as `a{sv}` rather than a fixed struct. Adding a field to a struct signature breaks every
//! client; adding a dict entry breaks none.
//!
//! The `has_default` entry is the one worth not losing: `compositor.keyboard.layout` genuinely
//! has no default (absent means "let libxkbcommon decide"), while `repeat_delay` defaults to
//! 200, and a UI has to show "system default" and "200" as different things.

use std::collections::HashMap;
use std::sync::Arc;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

use crate::edit;
use crate::paths::File;
use crate::schema::{self, Group, Kind, Reload, Setting};
use crate::store::{self, Changed, Store};

/// The errors this interface can answer with.
///
/// Named rather than folded into `org.freedesktop.DBus.Error.InvalidArgs`, so a client can tell
/// "you asked for a setting that does not exist" from "that value is out of range" from "the
/// file on disk is broken" without parsing English.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "com.wlrix.Settings1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// No such setting.
    UnknownKey(String),
    /// A real setting, deliberately not offered here yet, with what to do instead.
    Unsupported(String),
    /// The right key, the wrong shape.
    WrongType(String),
    /// A string that is not one of an enum's choices.
    InvalidValue(String),
    /// A number outside the range the schema declares.
    OutOfRange(String),
    /// The file on disk will not parse, so its values cannot be trusted or written over.
    FileInvalid(String),
    /// The owning program's own parser refused the file, so it was not installed.
    ///
    /// Distinct from `WriteFailed`, which is a filesystem problem: this one means the change
    /// was *understood* and rejected, and the message names the key. Nothing was changed.
    Refused(String),
    /// The file could not be written.
    WriteFailed(String),
}

impl From<store::Error> for Error {
    fn from(err: store::Error) -> Self {
        let message = err.to_string();
        match err {
            store::Error::UnknownKey(_) => Self::UnknownKey(message),
            store::Error::Unsupported { .. } => Self::Unsupported(message),
            store::Error::WrongType { .. } => Self::WrongType(message),
            store::Error::InvalidValue { .. } => Self::InvalidValue(message),
            store::Error::OutOfRange { .. } => Self::OutOfRange(message),
            store::Error::File(edit::Error::Parse { .. }) => Self::FileInvalid(message),
            store::Error::File(edit::Error::Refused { .. }) => Self::Refused(message),
            store::Error::File(_) => Self::WriteFailed(message),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

pub struct Settings {
    store: Arc<Store>,
}

impl Settings {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

#[zbus::interface(name = "com.wlrix.Settings1")]
impl Settings {
    /// Every namespace served, in file order.
    fn list_namespaces(&self) -> Vec<&'static str> {
        schema::namespaces()
    }

    /// What one setting is: type, range, choices, unit, default, who owns it, and prose.
    ///
    /// Also answers for a fan-out key such as `appearance.palette`, whose description carries a
    /// `members` list instead of an `owner` — see `Groups`.
    fn describe(&self, key: &str) -> Result<HashMap<String, OwnedValue>> {
        if let Some(group) = schema::lookup_group(key) {
            return Ok(describe_group(group));
        }
        let setting = schema::lookup(key).ok_or_else(|| {
            // Route through the store so a deliberately-deferred path gets its explanation
            // rather than a bare "no such setting".
            Error::from(
                self.store
                    .get(key)
                    .expect_err("lookup failed, so the store must too"),
            )
        })?;
        Ok(describe(setting))
    }

    /// The whole of one namespace, so a panel needs one round trip rather than one per field.
    fn describe_all(
        &self,
        namespace: &str,
    ) -> Result<HashMap<String, HashMap<String, OwnedValue>>> {
        known(namespace)?;
        Ok(schema::in_namespace(namespace)
            .map(|setting| (setting.key.to_owned(), describe(setting)))
            .collect())
    }

    /// The effective value of one setting: what the file says, or its declared default.
    ///
    /// A setting with neither — `compositor.keyboard.layout` on a machine that has never set
    /// one — answers an empty string of its own type rather than an error. There is nothing
    /// wrong with the request; the answer is simply "nothing", and `Sources` says so properly.
    fn get(&self, key: &str) -> Result<OwnedValue> {
        let kind = match schema::lookup_group(key) {
            Some(group) => &group.kind,
            None => {
                &schema::lookup(key)
                    .ok_or_else(|| {
                        Error::from(
                            self.store
                                .get(key)
                                .expect_err("lookup failed, so the store must too"),
                        )
                    })?
                    .kind
            }
        };
        match self.store.get(key)? {
            Some(value) => Ok(to_variant(&value)),
            None => Ok(empty_of(kind)),
        }
    }

    /// Every setting in a namespace that has a value.
    fn get_all(&self, namespace: &str) -> Result<HashMap<String, OwnedValue>> {
        known(namespace)?;
        Ok(self
            .store
            .get_all(namespace)
            .into_iter()
            .map(|(key, value)| (key.to_owned(), to_variant(&value)))
            .collect())
    }

    /// Where each value comes from: `user`, `system` or `default`.
    ///
    /// What lets a panel gray out a Reset that would do nothing, and be honest that a value it
    /// is showing came from `/etc/wlrix` rather than from the person using it.
    fn sources(&self, namespace: &str) -> Result<HashMap<String, String>> {
        known(namespace)?;
        Ok(self
            .store
            .sources(namespace)
            .into_iter()
            .map(|(key, source)| (key.to_owned(), source.name().to_owned()))
            .collect())
    }

    /// Set one setting. Sugar for a one-entry [`Settings::set_many`].
    async fn set(
        &self,
        key: &str,
        value: Value<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<HashMap<String, String>> {
        let owned =
            OwnedValue::try_from(value).map_err(|err| Error::WrongType(format!("{key}: {err}")))?;
        self.set_many(HashMap::from([(key.to_owned(), owned)]), emitter, header)
            .await
    }

    /// Set several settings at once: one write per file, one signal per owner.
    ///
    /// Answers a map of owning program to outcome — `applied`, `not-running`,
    /// `restart-required` or `next-login` — so a panel can say "the compositor is not running;
    /// this applies at next login" rather than appearing to have done nothing.
    async fn set_many(
        &self,
        values: HashMap<String, OwnedValue>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<HashMap<String, String>> {
        let mut requested = Vec::with_capacity(values.len());
        for (key, value) in values {
            let parsed = from_variant(&value).ok_or_else(|| {
                Error::WrongType(format!(
                    "{key}: {} is not a value this interface carries",
                    value.value_signature()
                ))
            })?;
            requested.push((key, parsed));
        }
        // Sorted so a batch is applied in a stable order and the log reads the same twice.
        requested.sort_by(|a, b| a.0.cmp(&b.0));

        let changed = self.store.set_many(&requested)?;
        announce(&emitter, &changed, origin(&header)).await;
        Ok(outcomes(&changed))
    }

    /// Remove settings, so each falls back to its default.
    ///
    /// Deleting the key rather than writing the default in its place: with `Option` fields and
    /// `deny_unknown_fields` throughout wlRIX, an absent key *is* the default, and writing
    /// today's default literally would pin it forever.
    async fn reset(
        &self,
        keys: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<HashMap<String, String>> {
        let changed = self.store.reset(&keys)?;
        announce(&emitter, &changed, origin(&header)).await;
        Ok(outcomes(&changed))
    }

    /// Remove every setting in a namespace.
    async fn reset_namespace(
        &self,
        namespace: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<HashMap<String, String>> {
        let changed = self.store.reset_namespace(namespace)?;
        announce(&emitter, &changed, origin(&header)).await;
        Ok(outcomes(&changed))
    }

    /// Re-read every file from disk and announce anything that moved.
    ///
    /// The watch makes this unnecessary in normal use. It is here for the cases the watch
    /// cannot cover: a file changed on a filesystem inotify does not report on, or a client
    /// that wants to be certain before it draws.
    async fn reload(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<HashMap<String, String>> {
        let files = crate::paths::ALL.iter().copied().collect();
        let mut result = HashMap::new();
        for notice in self.store.refresh(&files) {
            if let crate::store::Notice::Changed(changed) = notice {
                announce(&emitter, &changed, "external".to_owned()).await;
                result.extend(outcomes(&changed));
            }
        }
        Ok(result)
    }

    /// Which files will not currently parse, as key-to-message.
    ///
    /// A panel opened while a file is broken has to be able to find that out without waiting
    /// for a signal it already missed.
    #[zbus(property)]
    fn invalid(&self) -> HashMap<String, String> {
        self.store
            .invalid()
            .into_iter()
            .map(|(namespace, path, message)| {
                (
                    namespace.to_owned(),
                    format!("{}: {message}", path.display()),
                )
            })
            .collect()
    }

    /// What this build of the interface can do, so a client can tell an older daemon apart from
    /// one that has simply not been asked yet.
    ///
    /// 2 added fan-out keys — the `Groups` property, and `Describe`/`Get`/`Set`/`Reset`
    /// answering for one. A client that needs `appearance.palette` and finds 1 is talking to a
    /// daemon that will answer `UnknownKey`, and should say so rather than appear to do nothing.
    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }

    #[zbus(property)]
    fn namespaces(&self) -> Vec<&'static str> {
        schema::namespaces()
    }

    /// The keys that stand for the same setting in several files at once.
    ///
    /// Deliberately not entries in `Namespaces`: the prefix of `appearance.palette` is a
    /// category, not a config file, so `GetAll`, `Sources` and `ResetNamespace` have nothing to
    /// answer for it. `Describe` and `Get` do, and `Set` writes every member.
    #[zbus(property)]
    fn groups(&self) -> Vec<&'static str> {
        schema::GROUPS.iter().map(|group| group.key).collect()
    }

    /// Settings whose effective value changed, and who caused it.
    ///
    /// One signal per transaction rather than one per key, so a batch stays a batch on the wire.
    ///
    /// `origin` is the unique bus name of whoever called `Set`, or the literal `external` for a
    /// hand-edited file. A client compares it against its own unique name and drops the echo of
    /// its own write — which is what stops a settings panel from fighting its own debounce
    /// timer.
    #[zbus(signal)]
    pub async fn changed(
        emitter: &SignalEmitter<'_>,
        values: HashMap<String, OwnedValue>,
        origin: &str,
    ) -> zbus::Result<()>;

    /// A config file was edited into something that will not parse.
    ///
    /// Worth its own signal rather than silence: the daemon keeps serving the last-good values
    /// and deliberately does *not* signal the owner (the compositor's answer to a config it
    /// cannot parse is to fall back to built-in defaults, so poking it about a file someone is
    /// halfway through editing would blank their configuration). A panel that knows can say so
    /// instead of showing values that are no longer what the file holds.
    #[zbus(signal)]
    pub async fn file_invalid(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        path: &str,
        message: &str,
    ) -> zbus::Result<()>;

    /// ...and then fixed. A `Changed` with whatever moved follows.
    #[zbus(signal)]
    pub async fn file_recovered(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        path: &str,
    ) -> zbus::Result<()>;
}

/// Refuse a namespace that is not one of ours, before doing any work with it.
fn known(namespace: &str) -> Result<File> {
    File::from_namespace(namespace)
        .ok_or_else(|| Error::UnknownKey(format!("no such namespace: {namespace}")))
}

/// The unique bus name behind a method call, or `external` when there is none.
///
/// Empty only on a peer-to-peer connection, which is not how any of this is reached.
fn origin(header: &zbus::message::Header<'_>) -> String {
    header
        .sender()
        .map(ToString::to_string)
        .unwrap_or_else(|| "external".to_owned())
}

fn outcomes(changed: &Changed) -> HashMap<String, String> {
    changed
        .outcomes
        .iter()
        .filter(|(program, _)| !program.is_empty())
        .map(|(program, outcome)| ((*program).to_owned(), outcome.name().to_owned()))
        .collect()
}

/// Emit `Changed`, unless nothing did.
async fn announce(emitter: &SignalEmitter<'_>, changed: &Changed, origin: String) {
    if changed.values.is_empty() {
        return;
    }
    let values = changed
        .values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), to_variant(value)))
        .collect();
    if let Err(err) = Settings::changed(emitter, values, &origin).await {
        tracing::warn!("could not emit Changed: {err}");
    }
}

/// The metadata for one fan-out key, in the same shape as [`describe`].
///
/// Same keys, so a client can render one without a special case, with three differences it can
/// notice if it wants to: `members` lists what a write expands to, `owner` and `file` are empty
/// because there are several of each, and `reload` is the **worst** member's — a group is only
/// as live as its least live part, and saying otherwise would have a panel report `applied`
/// while one component still needed restarting.
fn describe_group(group: &'static Group) -> HashMap<String, OwnedValue> {
    let mut described = HashMap::new();
    let mut put = |name: &str, value: OwnedValue| {
        described.insert(name.to_owned(), value);
    };

    put("key", string(group.key));
    put("namespace", string(category(group.key)));
    put("signature", string(group.kind.signature()));
    put("kind", string(group.kind.name()));
    put("unit", string(""));
    put("summary", string(group.summary));
    put("description", string(group.description));
    put("owner", string(""));
    put("reload", string(worst_reload(group).name()));
    put("has_default", OwnedValue::from(group.kind.has_default()));
    put("default", empty_of(&group.kind));
    put("file", string(""));
    put(
        "members",
        list(group.members.iter().map(|key| (*key).to_owned()).collect()),
    );

    described
}

/// The part of a group key before the first dot. Not a namespace; see the `Groups` property.
fn category(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

/// The least live of a group's members, which is how live the group is.
fn worst_reload(group: &'static Group) -> Reload {
    group
        .settings()
        .map(|setting| setting.reload)
        .max_by_key(|reload| match reload {
            Reload::Live => 0,
            Reload::Restart => 1,
            Reload::NextLogin => 2,
            Reload::None => 3,
        })
        .unwrap_or(Reload::None)
}

/// The metadata for one setting, as the `a{sv}` a client renders from.
fn describe(setting: &'static Setting) -> HashMap<String, OwnedValue> {
    let mut described = HashMap::new();
    let mut put = |name: &str, value: OwnedValue| {
        described.insert(name.to_owned(), value);
    };

    put("key", string(setting.key));
    put("namespace", string(setting.namespace()));
    put("signature", string(setting.kind.signature()));
    put("kind", string(setting.kind.name()));
    put("unit", string(setting.unit.name()));
    put("summary", string(setting.summary));
    put("description", string(setting.description));
    put("owner", string(setting.owner.program().unwrap_or("")));
    put("reload", string(setting.reload.name()));
    put("has_default", OwnedValue::from(setting.kind.has_default()));

    // Always present, even when there is no default, so a client can read it without checking
    // -- `has_default` is what says whether it means anything.
    put(
        "default",
        crate::store::default_value(setting)
            .as_ref()
            .map_or_else(|| empty_of(&setting.kind), to_variant),
    );

    match &setting.kind {
        Kind::Int { min, max, .. } => {
            put("min", OwnedValue::from(*min));
            put("max", OwnedValue::from(*max));
        }
        Kind::Float { min, max, .. } => {
            put("min", OwnedValue::from(*min));
            put("max", OwnedValue::from(*max));
        }
        Kind::Enum { choices, .. } => {
            let values: Vec<String> = choices.iter().map(|c| c.value.to_owned()).collect();
            let labels: Vec<String> = choices.iter().map(|c| c.label.to_owned()).collect();
            put("choices", list(values));
            put("choice_labels", list(labels));
        }
        _ => {}
    }

    // Where a write would land, so a panel can tell somebody which file to open.
    let roots = crate::paths::Roots::from_environment();
    put(
        "file",
        string(
            &roots
                .user_path(setting.file)
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        ),
    );

    described
}

fn string(value: &str) -> OwnedValue {
    Value::from(value).try_into().expect("a string is a value")
}

fn list(values: Vec<String>) -> OwnedValue {
    Value::from(values).try_into().expect("an array is a value")
}

/// A value of the right type carrying nothing, for a setting that is not set.
fn empty_of(kind: &Kind) -> OwnedValue {
    match kind {
        Kind::Bool { .. } => OwnedValue::from(false),
        Kind::Int { .. } => OwnedValue::from(0i64),
        Kind::Float { .. } => OwnedValue::from(0f64),
        Kind::Str { .. } | Kind::Enum { .. } => string(""),
        Kind::StrList { .. } => list(Vec::new()),
    }
}

pub(super) fn to_variant(value: &edit::Value) -> OwnedValue {
    match value {
        edit::Value::Bool(v) => OwnedValue::from(*v),
        edit::Value::Int(v) => OwnedValue::from(*v),
        edit::Value::Float(v) => OwnedValue::from(*v),
        edit::Value::Str(v) => string(v),
        edit::Value::StrList(v) => list(v.clone()),
    }
}

/// Read a value off the wire, accepting every integer width a client might send.
///
/// Deliberately liberal on input. The schema says a setting is an `x`, but `busctl` sends `i`
/// by default, and a language binding will send whatever its own integer type maps to. Refusing
/// a perfectly unambiguous `u32` because it was not an `i64` would be pedantry that shows up as
/// "the settings app does not work".
fn from_variant(value: &Value<'_>) -> Option<edit::Value> {
    match value {
        // A client that wrapped its value twice, which several bindings do for `a{sv}`.
        Value::Value(inner) => from_variant(inner),
        Value::Bool(v) => Some(edit::Value::Bool(*v)),
        Value::U8(v) => Some(edit::Value::Int(i64::from(*v))),
        Value::I16(v) => Some(edit::Value::Int(i64::from(*v))),
        Value::U16(v) => Some(edit::Value::Int(i64::from(*v))),
        Value::I32(v) => Some(edit::Value::Int(i64::from(*v))),
        Value::U32(v) => Some(edit::Value::Int(i64::from(*v))),
        Value::I64(v) => Some(edit::Value::Int(*v)),
        Value::U64(v) => i64::try_from(*v).ok().map(edit::Value::Int),
        Value::F64(v) => Some(edit::Value::Float(*v)),
        Value::Str(v) => Some(edit::Value::Str(v.to_string())),
        Value::Array(array) => {
            let mut items = Vec::with_capacity(array.len());
            for element in array.iter() {
                match from_variant(element)? {
                    edit::Value::Str(item) => items.push(item),
                    // A list of anything else is not a shape any setting has, and quietly
                    // stringifying the elements would write something nobody asked for.
                    _ => return None,
                }
            }
            Some(edit::Value::StrList(items))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `<!-- ... -->` in `xml`, as an XML parser would delimit them.
    fn comments(xml: &str) -> Vec<&str> {
        let mut found = Vec::new();
        let mut rest = xml;
        while let Some(open) = rest.find("<!--") {
            let body = &rest[open + 4..];
            let Some(close) = body.find("-->") else {
                // An unterminated comment is its own kind of broken, and the assertion below is
                // not the place to discover it.
                break;
            };
            found.push(&body[..close]);
            rest = &body[close + 3..];
        }
        found
    }

    /// `--` is forbidden inside an XML comment, and zbus writes the `///` doc comments on this
    /// interface's methods into one **verbatim** -- no escaping, see `to_xml_docs` in
    /// zbus_macros. The workspace writes its em-dashes as `--`, which everywhere else is fine
    /// and here produces introspection that no strict XML parser will read.
    ///
    /// Not cosmetic: introspection is what a client's proxy generator consumes, and
    /// `Wlrix.Settings.Client`'s could not be generated at all until this was fixed. The same
    /// latent defect was found in `xdg-desktop-portal-wlrix` and fixed there, with the same
    /// test.
    ///
    /// The rendered XML is checked rather than the source text, so this cannot be fooled by
    /// where the comment happens to be written -- and it fails for the real reason rather than
    /// for a proxy of it. (The `--` in this very comment is safe: `//` and `//!` comments are
    /// not exported, and neither are the doc comments on a test.)
    #[test]
    fn no_doc_comment_can_break_the_introspection_xml() {
        use zbus::object_server::Interface;

        let settings = Settings::new(Store::load(crate::paths::Roots::from_environment()));
        let mut xml = String::new();
        settings.introspect_to_writer(&mut xml, 0);

        // A sanity check on the test itself: an interface that rendered nothing would pass the
        // real assertion trivially.
        assert!(xml.contains("<interface"), "introspected to nothing");
        for comment in comments(&xml) {
            assert!(
                !comment.contains("--"),
                "this doc comment makes the introspection XML unparseable:\n{comment}"
            );
        }
    }

    #[test]
    fn every_integer_width_a_client_might_send_is_accepted() {
        // `busctl set-property`-style callers send `i`; .NET sends `i` or `x`; a script may
        // send `u`. All of them mean the same number.
        for value in [
            Value::U8(7),
            Value::I16(7),
            Value::U16(7),
            Value::I32(7),
            Value::U32(7),
            Value::I64(7),
            Value::U64(7),
        ] {
            assert_eq!(from_variant(&value), Some(edit::Value::Int(7)), "{value:?}");
        }
    }

    #[test]
    fn a_doubly_wrapped_value_is_unwrapped() {
        // Several bindings put a variant inside the variant when building `a{sv}`.
        // `Value::Value` explicitly, not `Value::from` -- that is the identity conversion here
        // and would leave this testing nothing, which is what clippy caught.
        let wrapped = Value::Value(Box::new(Value::from("jp")));
        assert_eq!(from_variant(&wrapped), Some(edit::Value::Str("jp".into())));
        // Two deep, for a binding that wraps on both sides.
        let twice = Value::Value(Box::new(wrapped));
        assert_eq!(from_variant(&twice), Some(edit::Value::Str("jp".into())));
    }

    #[test]
    fn a_list_of_strings_comes_through_and_a_list_of_anything_else_does_not() {
        assert_eq!(
            from_variant(&Value::from(vec!["a".to_owned(), "b".to_owned()])),
            Some(edit::Value::StrList(vec!["a".into(), "b".into()]))
        );
        // Quietly stringifying these would write something nobody asked for.
        assert_eq!(from_variant(&Value::from(vec![1i32, 2i32])), None);
    }

    #[test]
    fn a_u64_too_large_for_the_file_is_refused_rather_than_wrapped() {
        assert_eq!(from_variant(&Value::U64(u64::MAX)), None);
    }

    #[test]
    fn a_value_round_trips_through_the_wire_types() {
        for value in [
            edit::Value::Bool(true),
            edit::Value::Int(-3),
            edit::Value::Float(0.25),
            edit::Value::Str("jp,us".into()),
            edit::Value::StrList(vec!["xdg-open".into()]),
        ] {
            let variant = to_variant(&value);
            assert_eq!(from_variant(&variant), Some(value.clone()), "{value:?}");
        }
    }

    #[test]
    fn a_group_describes_itself_in_the_same_shape_as_a_setting() {
        // A client renders a group with the code it already has for a setting, so every key an
        // ordinary description carries has to be here too -- with `members` on top.
        let group = schema::lookup_group("appearance.palette").expect("the scheme is a group");
        let described = describe_group(group);
        let ordinary = describe(schema::lookup("compositor.focus.policy").unwrap());
        for key in ordinary.keys() {
            assert!(
                described.contains_key(key) || key == "choices" || key == "choice_labels",
                "a group description is missing {key}"
            );
        }

        assert_eq!(described["kind"], string("string"));
        assert_eq!(described["signature"], string("s"));
        assert_eq!(described["namespace"], string("appearance"));
        assert_eq!(
            described["owner"],
            string(""),
            "a group has four owners, so it names none"
        );
        assert_eq!(described["file"], string(""));
        assert_eq!(described["has_default"], OwnedValue::from(false));
        assert_eq!(
            described["members"],
            list(vec![
                "compositor.appearance.palette".into(),
                "desktop.appearance.palette".into(),
                "portal.appearance.palette".into(),
                "screenshot.appearance.palette".into(),
                "tray.appearance.palette".into(),
            ])
        );
    }

    #[test]
    fn a_group_is_only_as_live_as_its_least_live_member() {
        // Three of the scheme's members reload on SIGHUP; wlrix-screenshot is not running to be
        // told. Reporting `live` would have a panel say the change was applied everywhere.
        let group = schema::lookup_group("appearance.palette").unwrap();
        assert_eq!(worst_reload(group), Reload::None);
        assert_eq!(describe_group(group)["reload"], string("none"));
    }

    #[test]
    fn a_group_key_is_not_a_namespace() {
        // `GetAll`, `Sources` and `ResetNamespace` have nothing to answer for `appearance`, and
        // listing it would have a client ask them.
        assert!(!schema::namespaces().contains(&"appearance"));
        assert!(known("appearance").is_err());
    }

    #[test]
    fn a_description_carries_what_a_panel_needs_to_draw_a_dropdown() {
        let described = describe(schema::lookup("compositor.focus.policy").unwrap());
        assert_eq!(described["kind"], string("enum"));
        assert_eq!(described["signature"], string("s"));
        assert_eq!(described["owner"], string("wlrix-compositor"));
        assert_eq!(described["reload"], string("live"));
        assert_eq!(described["has_default"], OwnedValue::from(true));
        assert_eq!(described["default"], string("click"));
        assert_eq!(
            described["choices"],
            list(vec!["click".into(), "pointer".into()])
        );
        assert_eq!(
            described["choice_labels"],
            list(vec![
                "Click to focus".into(),
                "Focus follows pointer".into()
            ])
        );
        assert_ne!(described["summary"], string(""));
    }

    #[test]
    fn a_number_carries_its_range_and_its_unit() {
        let described = describe(schema::lookup("compositor.keyboard.repeat_rate").unwrap());
        assert_eq!(described["min"], OwnedValue::from(0i64));
        assert_eq!(described["max"], OwnedValue::from(255i64));
        assert_eq!(described["unit"], string("hz"));
        assert_eq!(described["default"], OwnedValue::from(25i64));
    }

    #[test]
    fn a_setting_with_no_default_says_so_rather_than_inventing_one() {
        // The distinction a UI has to render: "system default" is not the same as an empty
        // layout string, and a `default` of "" cannot carry it on its own.
        let described = describe(schema::lookup("compositor.keyboard.layout").unwrap());
        assert_eq!(described["has_default"], OwnedValue::from(false));
        assert_eq!(described["default"], string(""));
    }

    #[test]
    fn a_deferred_setting_is_described_as_unsupported_not_unknown() {
        let store = Store::load(crate::paths::Roots::from_environment());
        let settings = Settings::new(store);
        match settings.describe("compositor.output.mode") {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("wlr-output-management"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        match settings.describe("compositor.keyboard.layuot") {
            Err(Error::UnknownKey(_)) => {}
            other => panic!("{other:?}"),
        }
    }
}
