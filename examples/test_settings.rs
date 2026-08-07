// SPDX-License-Identifier: GPL-3.0-or-later
//! A probe client for `com.wlrix.Settings`.
//!
//! The workspace convention: every protocol gets a small client that exercises it without
//! needing the real application to exist, the way `wlrix-compositor/examples/test_desks.rs`
//! does for the desks protocol. This is that for the settings daemon -- the way to answer
//! "does a Set actually reach the compositor" before there is a C# panel to ask with.
//!
//! ```text
//! cargo run --example test_settings                            # every namespace and value
//! cargo run --example test_settings -- describe compositor     # the schema, as a panel sees it
//! cargo run --example test_settings -- get compositor.focus.policy
//! cargo run --example test_settings -- set compositor.focus.policy pointer
//! cargo run --example test_settings -- set-many compositor.keyboard.layout=jp \
//!                                              compositor.keyboard.model=jp106
//! cargo run --example test_settings -- reset compositor.keyboard.layout
//! cargo run --example test_settings -- sources compositor
//! cargo run --example test_settings -- watch                   # print signals until Ctrl+C
//! ```
//!
//! Values are typed from the schema rather than from the text: `set … repeat_delay=300` sends
//! an integer because the daemon says the setting is one. That is the whole point of
//! `Describe` existing, and this is the smallest thing that demonstrates it.
//!
//! Not part of the daemon; a dev tool only. `busctl --user introspect com.wlrix.Settings
//! /com/wlrix/Settings` is the zero-dependency alternative for anything this does not cover.

use std::collections::HashMap;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const NAME: &str = "com.wlrix.Settings";
const PATH: &str = "/com/wlrix/Settings";
const INTERFACE: &str = "com.wlrix.Settings1";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let connection = Connection::session()?;
    let proxy = Proxy::new(&connection, NAME, PATH, INTERFACE)?;

    match args.first().map(String::as_str) {
        None | Some("list") => list(&proxy)?,
        Some("describe") => describe(&proxy, arg(&args, 1, "a namespace")?)?,
        Some("get") => get(&proxy, arg(&args, 1, "a key")?)?,
        Some("sources") => sources(&proxy, arg(&args, 1, "a namespace")?)?,
        Some("set") => {
            let key = arg(&args, 1, "a key")?;
            let value = arg(&args, 2, "a value")?;
            apply(&proxy, &[(key.to_owned(), value.to_owned())])?;
        }
        Some("set-many") => {
            let mut pairs = Vec::new();
            for pair in &args[1..] {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("{pair}: expected key=value"))?;
                pairs.push((key.to_owned(), value.to_owned()));
            }
            if pairs.is_empty() {
                return Err("set-many needs at least one key=value".into());
            }
            apply(&proxy, &pairs)?;
        }
        Some("reset") => {
            let keys: Vec<String> = args[1..].to_vec();
            if keys.is_empty() {
                return Err("reset needs at least one key".into());
            }
            let outcomes: HashMap<String, String> = proxy.call("Reset", &(keys,))?;
            report(&outcomes);
        }
        Some("watch") => watch(&connection)?,
        Some(other) => return Err(format!("unknown command: {other}").into()),
    }
    Ok(())
}

fn arg<'a>(args: &'a [String], index: usize, what: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("expected {what}"))
}

/// Every namespace, every value, and where each came from.
fn list(proxy: &Proxy<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let namespaces: Vec<String> = proxy.call("ListNamespaces", &())?;
    let invalid: HashMap<String, String> = proxy.get_property("Invalid")?;

    for namespace in namespaces {
        println!("[{namespace}]");
        if let Some(why) = invalid.get(&namespace) {
            println!("  !! not valid: {why}");
        }
        let values: HashMap<String, OwnedValue> = proxy.call("GetAll", &(&namespace,))?;
        let sources: HashMap<String, String> = proxy.call("Sources", &(&namespace,))?;
        let mut keys: Vec<&String> = sources.keys().collect();
        keys.sort();
        for key in keys {
            let source = &sources[key];
            match values.get(key) {
                Some(value) => println!("  {key} = {} ({source})", show(value)),
                // No value in the file and no default to name -- `keyboard.layout`, where
                // absent means "let libxkbcommon decide".
                None => println!("  {key} = <unset>"),
            }
        }
        println!();
    }
    Ok(())
}

/// The schema, printed the way a panel would consume it.
fn describe(proxy: &Proxy<'_>, namespace: &str) -> Result<(), Box<dyn std::error::Error>> {
    let described: HashMap<String, HashMap<String, OwnedValue>> =
        proxy.call("DescribeAll", &(namespace,))?;
    let mut keys: Vec<&String> = described.keys().collect();
    keys.sort();

    for key in keys {
        let setting = &described[key];
        let text = |name: &str| {
            setting
                .get(name)
                .map(|value| show(value))
                .unwrap_or_default()
                .trim_matches('"')
                .to_owned()
        };
        println!("{key}");
        println!("  {}", text("summary"));
        print!("  {} ({})", text("kind"), text("signature"));
        if let (Some(min), Some(max)) = (setting.get("min"), setting.get("max")) {
            print!(", {} to {}", show(min), show(max));
        }
        if let Some(choices) = setting.get("choices") {
            print!(", one of {}", show(choices));
        }
        let unit = text("unit");
        if !unit.is_empty() {
            print!(", in {unit}");
        }
        println!();
        match setting
            .get("has_default")
            .map(|value| show(value))
            .as_deref()
        {
            Some("true") => println!("  default {}", show(&setting["default"])),
            // The distinction the `has_default` flag exists for.
            _ => println!("  no default (unset means the owner decides)"),
        }
        println!("  owned by {} ({})", text("owner"), text("reload"));
        println!("  in {}", text("file"));
        println!();
    }
    Ok(())
}

fn get(proxy: &Proxy<'_>, key: &str) -> Result<(), Box<dyn std::error::Error>> {
    let value: OwnedValue = proxy.call("Get", &(key,))?;
    println!("{key} = {}", show(&value));
    Ok(())
}

fn sources(proxy: &Proxy<'_>, namespace: &str) -> Result<(), Box<dyn std::error::Error>> {
    let sources: HashMap<String, String> = proxy.call("Sources", &(namespace,))?;
    let mut keys: Vec<&String> = sources.keys().collect();
    keys.sort();
    for key in keys {
        println!("{key}: {}", sources[key]);
    }
    Ok(())
}

/// Send a batch, typing each value from what the daemon says the setting is.
///
/// This is the part worth having a probe for: getting the wire type wrong is the mistake a
/// client makes, and the daemon is the thing that knows what is right.
fn apply(proxy: &Proxy<'_>, pairs: &[(String, String)]) -> Result<(), Box<dyn std::error::Error>> {
    let mut values: HashMap<String, Value<'_>> = HashMap::new();
    for (key, text) in pairs {
        let described: HashMap<String, OwnedValue> = proxy.call("Describe", &(key,))?;
        let kind = described
            .get("kind")
            .map(|value| show(value))
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned();
        let value = match kind.as_str() {
            "bool" => Value::from(matches!(text.as_str(), "true" | "yes" | "on" | "1")),
            "int" => Value::from(
                text.parse::<i64>()
                    .map_err(|_| format!("{key}: {text} is not a whole number"))?,
            ),
            "double" => Value::from(
                text.parse::<f64>()
                    .map_err(|_| format!("{key}: {text} is not a number"))?,
            ),
            // Comma-separated, because a shell argument is one string and every list-valued
            // wlRIX setting is a list of short words.
            "string-list" => Value::from(
                text.split(',')
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ),
            _ => Value::from(text.clone()),
        };
        values.insert(key.clone(), value);
    }

    let outcomes: HashMap<String, String> = proxy.call("SetMany", &(values,))?;
    report(&outcomes);
    Ok(())
}

/// Say what became of the change, per owning program.
fn report(outcomes: &HashMap<String, String>) {
    if outcomes.is_empty() {
        println!("nothing changed");
        return;
    }
    for (program, outcome) in outcomes {
        let note = match outcome.as_str() {
            "applied" => " -- it re-read its config",
            "not-running" => " -- written; it will be read at the next start",
            "restart-required" => " -- written; restart it to pick this up",
            "next-login" => " -- written; read at the next login",
            _ => "",
        };
        println!("{program}: {outcome}{note}");
    }
}

/// Print signals until Ctrl+C.
fn watch(connection: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    use zbus::MatchRule;
    use zbus::blocking::MessageIterator;

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)?
        .path(PATH)?
        .build();
    let messages = MessageIterator::for_match_rule(rule, connection, None)?;

    println!("watching {INTERFACE}; edit a config file by hand, or run `set` from another shell");
    for message in messages {
        let message = message?;
        let header = message.header();
        let member = header.member().map(|m| m.to_string()).unwrap_or_default();
        match member.as_str() {
            "Changed" => {
                let (values, origin): (HashMap<String, OwnedValue>, String) =
                    message.body().deserialize()?;
                // The origin is what a real client compares against its own unique name so it
                // does not act on the echo of its own write.
                println!("Changed (from {origin}):");
                let mut keys: Vec<&String> = values.keys().collect();
                keys.sort();
                for key in keys {
                    println!("  {key} = {}", show(&values[key]));
                }
            }
            "FileInvalid" => {
                let (namespace, path, message): (String, String, String) =
                    message.body().deserialize()?;
                println!("FileInvalid {namespace} ({path}): {message}");
            }
            "FileRecovered" => {
                let (namespace, path): (String, String) = message.body().deserialize()?;
                println!("FileRecovered {namespace} ({path})");
            }
            other => println!("{other}"),
        }
    }
    Ok(())
}

/// A value as a person would read it.
fn show(value: &Value<'_>) -> String {
    match value {
        Value::Value(inner) => show(inner),
        Value::Str(v) => format!("{:?}", v.as_str()),
        Value::Array(array) => {
            let items: Vec<String> = array.iter().map(|value| show(value)).collect();
            format!("[{}]", items.join(", "))
        }
        // Bare, not zvariant's `int64 100`: this is read by a person deciding what to type
        // next, and the width is not the interesting part.
        Value::I64(v) => v.to_string(),
        Value::F64(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        other => format!("{other}"),
    }
}
