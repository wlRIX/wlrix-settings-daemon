# wlrix-settings-daemon

The wlRIX settings service: the one program that **writes** the wlRIX config files.

Every wlRIX component reads its own TOML file, and always will — the compositor reads
`compositor.toml`, `wlrix-idle` reads `idle.toml`, and none of them has gained a dependency on this. What this replaces
is the *writing*. Before it, a settings app had to hand-roll a TOML editor, hand-roll a pidfile read, and hand-roll a
`kill(pid, SIGHUP)`; the next settings app would have copied all three, and each copy would have had to learn a
different owning process's reload story. Here it happens once, behind `com.wlrix.Settings` on the session bus.

**The desktop works without it.** It is a writer and a notifier, never a source of truth. A session with this binary
removed is a complete session — just one where settings apps have nothing to talk to.

## Why not keep doing it in the app

`Wlrix.Settings.Keyboard` shipped with a 250-line line-based TOML editor, because `wlrix-apps`
has no TOML library and `compositor.toml` is a file the user also edits by hand, so a full rewrite would have destroyed
their comments. It could not see a dotted key, an inline table, a multi-line string, or a section header with a comment
after it — and on that last one it did something worse than fail:

```toml
[keyboard]  # japanese
layout = "us"
```

`FindSection` compared the trimmed line to `"[keyboard]"` exactly, did not find this one, took the not-found branch, and
**appended a second `[keyboard]` table**. `toml` rejects a duplicate table, so `wlrix-compositor` then reported the
whole file invalid and fell back to built-in defaults. One click in the settings app, and the user's entire compositor
config was gone.

That blast radius is not specific to that bug. Every wlRIX config struct is
`#[serde(deny_unknown_fields)]`, so *any* wrong key costs the whole file. Writing these files correctly is a job worth
doing once, carefully, in one place.

## The interface

`com.wlrix.Settings` · `/com/wlrix/Settings` · `com.wlrix.Settings1`

`com.wlrix.*` is not a new namespace — it is already the workspace's, used as the Wayland app id by every Avalonia app
(`com.wlrix.toolchest`, `com.wlrix.desks`, …). The version digit goes on the interface, not the bus name, per the
convention `org.freedesktop.login1` follows.

A key is `<namespace>.<toml path>`, where the namespace is the config file's stem:

```
background.mode                 ~/.config/wlrix/background.toml   mode
compositor.keyboard.layout      ~/.config/wlrix/compositor.toml   [keyboard] layout
compositor.focus.policy         ~/.config/wlrix/compositor.toml   [focus] policy
idle.lock.command               ~/.config/wlrix/idle.toml         [lock] command
desktop.metrics.icon            ~/.config/wlrix/desktop.toml      [metrics] icon
portal.preview.tick_ms          ~/.config/wlrix/portal.toml       [preview] tick_ms
session.compositor              ~/.config/wlrix/session.toml      compositor
```

…with one exception. `appearance.palette` is a **fan-out key**: its prefix is a category, not a file. Writing it writes
`[appearance] palette` into `compositor.toml`, `desktop.toml`, `screenshot.toml` *and* `tray.toml`, and signals all four
owners. See "One setting, four files" below.

| Member                          | Signature                          |                                                                    |
|---------------------------------|------------------------------------|--------------------------------------------------------------------|
| `ListNamespaces`                | `() → as`                          | `background`, `compositor`, `desktop`, `idle`, `portal`, `session` |
| `Describe` / `DescribeAll`      | `(s) → a{sv}` / `(s) → a{sa{sv}}`  | the schema for one key / a whole panel in one round trip           |
| `Get` / `GetAll`                | `(s) → v` / `(s) → a{sv}`          | the effective value: the file's, else the declared default         |
| `Sources`                       | `(s) → a{ss}`                      | key → `user` \| `system` \| `default`                              |
| `Set` / `SetMany`               | `(sv) → a{ss}` / `(a{sv}) → a{ss}` | write; answers what became of it, per owner                        |
| `Reset` / `ResetNamespace`      | `(as) → a{ss}` / `(s) → a{ss}`     | remove keys, so they fall back to their defaults                   |
| `Reload`                        | `() → a{ss}`                       | re-read everything from disk                                       |
| `Invalid`                       | property `a{ss}`                   | files that will not currently parse, and why                       |
| `Version`, `Namespaces`         | properties                         | `Version` is 2 since fan-out keys                                  |
| `Groups`                        | property `as`                      | the fan-out keys: `appearance.palette`                             |
| `Changed`                       | signal `(a{sv} values, s origin)`  | one per transaction, not one per key                               |
| `FileInvalid` / `FileRecovered` | signals                            | a hand-edit broke, or fixed, a file                                |

Four things are worth knowing before writing a client.

**`SetMany` is the primitive; `Set` is sugar.** One call is one write per file and one signal per owner, however many
keys are in it — so a panel applying four keyboard fields does not make the compositor recompile its keymap four times.
It also closes a race that exists today: writing the file and *then* sending `SIGHUP` are two steps, between which the
file can be replaced again. Validation runs for every key before any file is touched, so a batch with one bad value
leaves nothing half-applied. Across files there is no such guarantee and none is offered — two files cannot be renamed
atomically.

**`Reset` deletes the key; it does not write the default.** With `Option` fields and
`deny_unknown_fields` throughout wlRIX, an absent key *is* the default. Writing today's default literally would pin it,
so a later change to what the default means would never reach anyone who had pressed Reset.

**`origin` is how you ignore your own echo.** It carries the unique bus name of whoever called
`Set`, or the literal `external` for a hand-edited file. Compare it against your own unique name and drop the match, or
your panel will fight its own debounce timer.

**One setting, four files.** Almost every key has one file and one owner, which is right: `[keyboard] layout` is the
compositor's and nobody else's. A color scheme is not like that — it has to reach the compositor's window chrome, the
desktop's icons, the tray and the screenshot overlay at once, and each of those reads it out of its own file. Declaring
it once per component would offer four switches for one setting, and somebody who moved three of them would be left
with a desktop that half changed.

So `appearance.palette` is a **group**. `Set` on it expands to its members before anything else happens and then
travels the ordinary path: one write per file, one signal per owner, validated through each owner's own parser first.
`Reset` clears all four. `Get` answers the first member's value. `Describe` answers in the same shape as any other key,
with `members` listing what it expands to and `owner`/`file` empty because there are several of each; `reload` is the
**least live** member's, so a panel does not report `applied` while one component still needs restarting.

The members are still ordinary keys. Somebody who genuinely wants the screenshot overlay dark and nothing else dark
sets `screenshot.appearance.palette` on its own; they are not fighting the group, they simply do not use it. That also
means the four can drift apart after a hand-edit — a client that cares reads `members` from `Describe` and asks for
each, rather than the daemon growing a method for one panel's status line. `Changed` carries the group key alongside
whichever members moved, including for a hand-edit, so a client can watch just the one.

`appearance` is deliberately **not** in `Namespaces`. There is no `appearance.toml`, so `GetAll`, `Sources` and
`ResetNamespace` have nothing to answer for it; `Groups` is the property that lists fan-out keys.

The outcome map answers `applied` (the owner was signaled and re-read its config),
`not-running` (written; it will be read at the next start), `restart-required`, or `next-login`. That is what lets a
panel say *"the compositor isn't running; this applies at next login"*
instead of appearing to have done nothing.

Errors are named — `com.wlrix.Settings1.Error.{UnknownKey, Unsupported, WrongType, InvalidValue,
OutOfRange, FileInvalid, Refused, WriteFailed}` — so a client can tell them apart without parsing English. `Refused` is
the interesting one: the change was understood and the owning program rejected it, and nothing was written.

## What it is careful about

**Your file stays your file.** Comments, key order, spacing, and every section it was not asked to touch survive a write
intact, including the trailing comment on the line it *did* change:

```toml
# my own file, hands off
[keyboard]  # japanese
layout = "jp"                       # ← the only line a Set touched
model = "jp106"
options = "grp:alt_shift_toggle"    # switch with alt+shift
```

**A first write seeds from `/etc`.** wlRIX takes the first config file it finds and does *not*
merge, so creating a user file to hold one key would otherwise discard everything the administrator put in `/etc/wlrix`.
The first write to a namespace with no user file copies the system file verbatim first, comments and all, and applies
the edit to that.

**Hand-editing keeps working.** An inotify watch on both config directories notices, and a
`Changed` carrying **only the keys that actually moved** goes out — plus a `SIGHUP` to the owner, so editing
`compositor.toml` in `$EDITOR` now applies live without anyone having to remember the pidfile. Its own writes are
recognized by byte comparison and not re-announced.

**A broken file is reported, not acted on.** When someone is halfway through editing, the daemon keeps serving the
last-good values, emits `FileInvalid`, and deliberately does **not** signal the owner — the compositor's reaction to a
config it cannot parse is to fall back to built-in defaults, so poking it about a half-typed file would blank their
whole configuration until they finished.

**Writes are atomic and flushed.** Temporary file beside the target, `fsync`, rename, `fsync` the directory. Stricter
than `wlrix-compositor`'s `outputs.toml` write, which is right not to flush — that is disposable state rebuilt from the
hardware, and a hand-owned config file is not.

## What it deliberately does not do

Array-of-table sections and free-form maps are **not** offered, and asking for one gets an
`Unsupported` error that says what to do instead rather than a misleading "no such setting". They are keyed collections,
not scalar leaves, and want add/remove/reorder rather than get/set — a later surface that nothing in the current
signatures blocks.

- **`[[output]]`** (compositor) — four reasons, not just its shape. The machine-written
  `$XDG_STATE_HOME/wlrix/outputs.toml` is layered on top of it per field, so a value set here is overridden the moment
  the compositor next saves; and `reload_config` never re-runs
  `outputs::resolve`, so nothing would take effect before a restart anyway. The right channel already exists: the
  compositor implements `wlr-output-management`, which applies live and atomically and has a test-and-rollback flow. A
  Displays panel should speak that.
- **`[[output]]`** (background) — a different section with the same name and the same problem: a per-monitor wallpaper
  is a picture, a mode and a color that only mean anything together with the connector name they hang off. Unlike the
  compositor's, this one *would* apply live, so it is the first candidate for the collection surface below.
- **`[[timeout]]`** (idle) — a countdown is several fields that only mean anything together.
- **`[[app]]`, `[env]`** (session) — a list and a free-form map with no fixed keys to describe.
- **`[preview] tile`** (portal) — a fixed-length pair of numbers, which none of the value shapes here describes.

`wlrix-greeter` is absent entirely: its config is greetd's, root-owned under `/etc/greetd/`, and not a session setting.

## Trying it

```bash
busctl --user introspect com.wlrix.Settings /com/wlrix/Settings com.wlrix.Settings1
busctl --user call com.wlrix.Settings /com/wlrix/Settings com.wlrix.Settings1 \
    Get s compositor.focus.policy
busctl --user call com.wlrix.Settings /com/wlrix/Settings com.wlrix.Settings1 \
    Set sv compositor.focus.policy s pointer
busctl --user monitor --match "type='signal',interface='com.wlrix.Settings1'"
```

Or the probe, which types each value from the schema rather than making you spell out the signature — the workspace's
`test_*` convention, as in
`wlrix-compositor/examples/test_desks.rs`:

```bash
cargo run --example test_settings                          # every namespace and value
cargo run --example test_settings -- describe compositor   # the schema, as a panel sees it
cargo run --example test_settings -- set compositor.focus.policy pointer
cargo run --example test_settings -- set-many compositor.keyboard.layout=jp \
                                              compositor.keyboard.model=jp106
cargo run --example test_settings -- reset compositor.keyboard.layout
cargo run --example test_settings -- watch                 # signals, until Ctrl+C
```

To try it without touching your real config, point it at a throwaway config home — the same trick the compositor's
idle-timeout testing uses:

```bash
XDG_CONFIG_HOME=/tmp/wlrix-test RUST_LOG=debug wlrix-settings-daemon --replace
```

The daemon itself also runs without a bus:

```bash
wlrix-settings-daemon --check          # what is wrong with the config files, if anything
wlrix-settings-daemon --dump-schema    # every setting at its default, annotated
```

## Building and installing

```bash
just release
sudo just install
```

Bus-activated, so nothing starts it: the first method call does. Unlike
`xdg-desktop-portal-wlrix` it carries **no** `ConditionEnvironment=WAYLAND_DISPLAY` — it holds no Wayland connection, so
it works from a TTY and from a `busctl` probe too. It is deliberately not in `wlrix-session`'s `DEFAULT_APPS`: a session
where nobody opens a settings panel should not have it resident, and `stop_all` would tear it down exactly when a
still-open panel wanted it.

The binary goes in `$PREFIX/bin`, not `$PREFIX/lib` where the portal lives. The portal argues
`lib` because it is never run by hand; this one is (`--check`, `--dump-schema`).

## Schema drift

The daemon cannot link `wlrix-compositor`'s `Config` type — the repos build standalone, with no path dependencies
between them — so `src/schema/table.rs` is a hand-kept copy of what those serde structs accept, and a hand-kept copy
will drift. With `deny_unknown_fields`, a drifted table is not a wrong setting; it is a lost config file.

Three layers guard it, and the middle one is the one that matters:

1. **Integrity tests** in `src/schema/mod.rs`: no duplicate keys, every key spells out its own path, every default
   inside its own range, every enum default among its own choices, every live setting with a pidfile to signal.

2. **Every write is validated by the program that will read it.** Before the rename, the candidate file is handed to
   `<owner> --check-config <path>`, and a refusal aborts the write with the real file untouched. Each of
   `wlrix-compositor`, `wlrix-desktop`, `wlrix-idle` and
   `xdg-desktop-portal-wlrix` grew that flag alongside this daemon; each parses with its own serde types and prints its
   own message. That puts correctness back with the type that defines it, and demotes this table to what a UI actually
   needs — ranges, choices and prose.

   So a typo here is caught before it reaches anyone's disk:

   ```
   $ busctl --user call … Set sv compositor.focus.policy s pointer
   Call failed: wlrix-compositor would not accept the change: TOML parse error at line 2 …
   unknown field `layuot`, expected one of `rules`, `model`, `layout`, …
   ```

   It declines to answer in two cases, both of which log and write anyway rather than failing — a settings daemon that
   refused to work because the compositor was not on `PATH` would be worse than one that wrote without asking. The
   program may not be installed; or it may not exit, which is the hazard worth naming, because `wlrix-compositor`
   *ignored* unknown arguments until this flag existed. A new daemon against an old compositor would have started a
   compositor rather than checked a file, so the check is killed after two seconds. (The other three have always
   rejected unknown arguments.)

3. **An epoch gate**, `just check-schema`, beside the existing `check-palette`: dumps the schema and runs each
   component's `--check-config` over its own slice, so drift is a red CI run rather than a settings app that has quietly
   stopped working. It lives in `wlrix-epoch`
   because that is the one place with every repo checked out.

`--dump-schema` also emits every key at its declared default, if you would rather paste one component's block into that
component's own tests as a fixture.

## Follow-ups

- The collection surface (`ListItems`/`AddItem`/`RemoveItem`/`SetItemField`) for the sections listed above.
- `wlrix-desktop/src/session.rs` keeps its own copy of the pidfile-and-signal logic for Log Out. It should stay that way
  while the desktop must boot without this daemon.
- Nothing yet drives `just check-schema` in CI; it needs every component built, so it belongs in a `wlrix-epoch`
  workflow rather than in this repo's.
