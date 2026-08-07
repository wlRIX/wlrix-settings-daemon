#!/usr/bin/env just --justfile
name := 'wlrix-settings-daemon'

rootdir := ''
prefix := '/usr'

usrdir := absolute_path(clean(rootdir / prefix))
# `bin`, not `lib` where the portal's backend goes. That one argues `lib` because it is
# bus-activated and never run by hand; this one is bus-activated *and* run by hand --
# `--check` and `--dump-schema` are useful on their own -- so it belongs on PATH with every
# other wlRIX component.
bindir := usrdir / 'bin'
systemddir := usrdir / 'lib' / 'systemd' / 'user'
dbusdir := usrdir / 'share' / 'dbus-1' / 'services'

bin-src := 'target' / 'release' / name
bin-dst := bindir / name

# Both activation files carry an absolute path to the binary, so they are templates with
# @BINDIR@ substituted at install time -- a staged or non-/usr install has to point at where
# the binary actually landed.
dbus-src := 'data' / 'com.wlrix.Settings.service.in'
dbus-dst := dbusdir / 'com.wlrix.Settings.service'

unit-src := 'data' / name + '.service.in'
unit-dst := systemddir / name + '.service'

# List available recipes.
default:
  @just --list

release:
  cargo build --release

lint:
  cargo clippy

test:
  cargo test

# Every setting at its declared default, as an annotated config file.
#
# The generated reference for the schema table. Copy the block for one component into that
# component's own tests and assert its `Config` accepts it -- that turns a drifted key into a
# failing test in the owner's repo rather than a config file its owner rejects at runtime. See
# the README's "Schema drift".
[doc("Regenerate data/full-config-example.toml from the schema")]
schema:
  cargo run --quiet --release -- --dump-schema > data/full-config-example.toml
  @echo "wrote data/full-config-example.toml"

# The interface's introspection XML, which is what a client generates a proxy from.
#
# Needs the daemon running: introspection is served, not compiled in. Start one against a
# throwaway config home first if you do not want it touching yours:
#
#     XDG_CONFIG_HOME=/tmp/wlrix-test cargo run -- --replace
[doc("Print the D-Bus introspection XML (needs a running daemon)")]
introspect:
  @busctl --user introspect --xml-interface com.wlrix.Settings /com/wlrix/Settings

# Install the settings daemon and the two files that make it activatable.
#
# Deliberately does not build: this is normally run as root, and building as root leaves a
# target directory nobody can write to afterwards.
#
#     just release && sudo just install
[doc("Install the settings daemon and its activation files (build first; run as root)")]
install:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -x '{{bin-src}}' ]; then
      echo "no release build -- run 'just release' first" >&2
      exit 1
  fi
  install -Dm0755 '{{bin-src}}' '{{bin-dst}}'
  # DESTDIR must not leak into the paths the files themselves carry: a staged install is
  # assembled under rootdir but runs from prefix, so substitute the runtime location.
  runtime_bindir='{{ clean(prefix / "bin") }}'
  for pair in '{{dbus-src}}:{{dbus-dst}}' '{{unit-src}}:{{unit-dst}}'; do
      src="${pair%%:*}"; dst="${pair#*:}"
      install -d "$(dirname "$dst")"
      sed "s|@BINDIR@|${runtime_bindir}|g" "$src" > "$dst"
      chmod 0644 "$dst"
  done
  for f in '{{bin-dst}}' '{{dbus-dst}}' '{{unit-dst}}'; do
      echo "installed $f"
  done
  echo
  echo "Nothing starts it: it is bus-activated, so the first method call does."
  echo "Nothing depends on it either -- the desktop is complete without it."

[doc("Remove what install put down")]
uninstall:
  #!/usr/bin/env bash
  set -euo pipefail
  rm -f '{{bin-dst}}' '{{dbus-dst}}' '{{unit-dst}}'
  echo "removed the settings daemon and its activation files"

clean:
  cargo clean
