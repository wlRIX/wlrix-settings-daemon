// SPDX-License-Identifier: GPL-3.0-or-later
//! Every setting wlRIX has, in one table.
//!
//! One `const` slice rather than a declarative macro. The workspace hand-rolls its argument
//! parsing rather than taking `clap`, for the same reason: a table you can read in a diff beats
//! a macro whose expansion nobody reviews. It is also `const`-evaluable, so the integrity tests
//! next door cost nothing.
//!
//! Order is the order of the file. That is what a generated example wants to print, and what a
//! panel wants to lay out; alphabetical would interleave `[focus]` and `[keyboard]`.
//!
//! **Each block below is a hand-kept copy of one component's serde structs.** The comment above
//! each names the file it was copied from, so the next person changing that file knows what
//! else to change. See the module docs in `super` for why a copy is acceptable here and what
//! actually stops it from costing anyone their config.

use super::{Choice, Kind, Owner, Reload, Setting, Unit};
use crate::paths::File;

/// `wlrix-idle`'s longest countdown, borrowed as the bound on the compositor's blank timer too.
const A_DAY: i64 = 86_400;

pub const SETTINGS: &[Setting] = &[
    // ---------------------------------------------------------------------------------------
    // background.toml -- wlrix-bg/src/config.rs
    //
    // All `Reload::Live`: `wlrix-bg` re-reads the file on SIGHUP, drops its decode cache and
    // repaints every output. Changing a wallpaper is a buffer swap, so there is nothing here
    // that needs a restart.
    //
    // `[[output]]` -- the per-monitor overrides -- is absent for the same reason as the
    // compositor's and `wlrix-idle`'s `[[timeout]]`: it is a keyed collection, not a scalar
    // leaf. An override is three fields that only mean anything together with the connector
    // name they hang off.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "background.image",
        file: File::Background,
        path: &["image"],
        // No default, and absent is not the same as "": absent means the plain color, which is
        // what a fresh install shows, and a UI must be able to offer "no picture" as a choice
        // rather than as an empty text field.
        kind: Kind::Str { default: None },
        owner: Owner::Background,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Wallpaper",
        description: "The picture to show on the desktop, as an absolute path. PNG, JPEG, WebP \
                      and the rest, plus the formats IRIX shipped its own backgrounds in -- SGI \
                      (.rgb, .bw) and XPM. Empty shows the color alone.",
    },
    Setting {
        key: "background.mode",
        file: File::Background,
        path: &["mode"],
        kind: Kind::Enum {
            // Exactly what `#[serde(rename_all = "lowercase")]` on wlrix-bg's `Mode` accepts.
            // Its own tests assert that "tiled", "centre", "zoom" and every capitalized form are
            // rejected, so offering one here would cost the user their whole file.
            default: Some("fill"),
            choices: &[
                Choice {
                    value: "fill",
                    label: "Fill the screen",
                },
                Choice {
                    value: "fit",
                    label: "Fit inside the screen",
                },
                Choice {
                    value: "stretch",
                    label: "Stretch to the screen",
                },
                Choice {
                    value: "center",
                    label: "Center at full size",
                },
                Choice {
                    value: "tile",
                    label: "Tile",
                },
                Choice {
                    value: "solid",
                    label: "Color only",
                },
            ],
        },
        owner: Owner::Background,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "How the wallpaper is fitted",
        description: "Fill covers the screen and crops what does not fit; fit shows the whole \
                      picture with the color in the bars; stretch distorts it to fit exactly; \
                      center and tile draw it at its own size. Solid ignores the picture.",
    },
    Setting {
        key: "background.color",
        file: File::Background,
        path: &["color"],
        // `#555555` is the palette's DESKTOP role -- the gray IRIX's desktop is under everything
        // -- and wlrix-bg's own `DEFAULT_COLOR`.
        kind: Kind::Str {
            default: Some("#555555"),
        },
        owner: Owner::Background,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Desktop color",
        description: "The color behind the picture, as \"#rrggbb\". Seen in the letterbox bars \
                      under fit and center, through anything transparent, and over the whole \
                      screen when there is no picture or the mode is solid.",
    },
    // ---------------------------------------------------------------------------------------
    // compositor.toml -- wlrix-compositor/src/config.rs
    //
    // Everything here is `Reload::Live`: `State::reload_config` re-applies `[keyboard]` through
    // `set_xkb_config`/`change_repeat_info`, reloads the cursor theme when `[cursor]` changed and
    // re-arms the blank timer, and `[focus]`/`[windows]` are read at the point of use, so a
    // reloaded config is in force for the next click. `[[output]]` is the exception and is absent
    // from this table -- see below.
    //
    // `[cursor]` is live for the *compositor's* pointer only. Clients read XCURSOR_THEME from
    // their own environment, fixed when they were started, so a running app keeps the theme it
    // launched with however many reloads happen; the change is whole at the next login.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "compositor.keyboard.rules",
        file: File::Compositor,
        path: &["keyboard", "rules"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "xkb rules file",
        description: "Which xkb rules file to compile the keymap from. Empty picks the system \
                      default, which is what almost every installation wants.",
    },
    Setting {
        key: "compositor.keyboard.model",
        file: File::Compositor,
        path: &["keyboard", "model"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Keyboard model",
        description: "The physical keyboard, e.g. pc105, or jp106 for a Japanese 106-key board. \
                      Empty lets libxkbcommon decide.",
    },
    Setting {
        key: "compositor.keyboard.layout",
        file: File::Compositor,
        path: &["keyboard", "layout"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Keyboard layout",
        description: "A comma-separated list of layouts, e.g. \"jp\" or \"jp,us\". More than \
                      one enables the layout-cycle key. Empty uses the system default, usually \
                      US.",
    },
    Setting {
        key: "compositor.keyboard.variant",
        file: File::Compositor,
        path: &["keyboard", "variant"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Layout variant",
        description: "A comma-separated list of variants, one per layout, e.g. \"dvorak\". \
                      Empty means each layout's own default.",
    },
    Setting {
        key: "compositor.keyboard.options",
        file: File::Compositor,
        path: &["keyboard", "options"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "xkb options",
        description: "Comma-separated xkb options, e.g. \"grp:alt_shift_toggle\" to switch \
                      layouts with Alt+Shift, or \"ctrl:nocaps\" for Caps Lock as Control.",
    },
    Setting {
        key: "compositor.keyboard.repeat_delay",
        file: File::Compositor,
        path: &["keyboard", "repeat_delay"],
        kind: Kind::Int {
            default: Some(200),
            min: 0,
            max: 10_000,
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::Milliseconds,
        summary: "Repeat delay",
        description: "How long a key is held before it starts repeating.",
    },
    Setting {
        key: "compositor.keyboard.repeat_rate",
        file: File::Compositor,
        path: &["keyboard", "repeat_rate"],
        kind: Kind::Int {
            default: Some(25),
            min: 0,
            max: 255,
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::Hertz,
        summary: "Repeat rate",
        description: "Repeats per second once repetition starts. Zero switches key repeat off \
                      altogether, which is what the Wayland protocol reads a rate of 0 as.",
    },
    Setting {
        key: "compositor.focus.policy",
        file: File::Compositor,
        path: &["focus", "policy"],
        kind: Kind::Enum {
            default: Some("click"),
            // Exactly what `#[serde(rename_all = "lowercase")]` on the compositor's
            // `FocusPolicy` accepts. Its own tests assert that "explicit" -- Motif's name for
            // the same thing -- and "Pointer" are rejected, so offering either here would be
            // offering a value that costs the user their file.
            choices: &[
                Choice {
                    value: "click",
                    label: "Click to focus",
                },
                Choice {
                    value: "pointer",
                    label: "Focus follows pointer",
                },
            ],
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Focus policy",
        description: "How a window comes to have the keyboard. Click focuses and raises at the \
                      same time; pointer gives the keyboard to whatever is under the pointer, \
                      decorations included, while a click still raises.",
    },
    Setting {
        key: "compositor.windows.opaque_move",
        file: File::Compositor,
        path: &["windows", "opaque_move"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Show window contents while moving",
        description: "On, a window is dragged as itself. Off gives IRIX's other mode: the \
                      window stays put and a red wireframe follows the pointer, with the move \
                      applied on release.",
    },
    Setting {
        key: "compositor.windows.opaque_resize",
        file: File::Compositor,
        path: &["windows", "opaque_resize"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Show window contents while resizing",
        description: "Off rubber-bands the frame and configures the client once, on release, \
                      rather than on every motion event -- which is the cheaper of the two by \
                      a wide margin on a large window.",
    },
    Setting {
        key: "compositor.cursor.theme",
        file: File::Compositor,
        path: &["cursor", "theme"],
        // No default, and for the same reason as `compositor.keyboard.layout`: absent is not a
        // value here. It means "whatever XCURSOR_THEME says, or this machine's default theme",
        // which a UI has to be able to offer as its own choice rather than as an empty field.
        // What a wlRIX install actually gets is `sgi`, from the system default config the
        // compositor installs -- and a user file a panel writes is seeded from that.
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Cursor theme",
        description: "An XCursor theme name -- a directory under share/icons on an XDG data \
                      directory, or under ~/.icons. wlRIX ships sgi, the IRIX pointer set. A \
                      theme that is not installed leaves a plain built-in arrow.",
    },
    Setting {
        key: "compositor.cursor.size",
        file: File::Compositor,
        path: &["cursor", "size"],
        // Also no default: absent falls through to XCURSOR_SIZE before it reaches the built-in
        // 24. The range starts at 8 rather than 1 because a pointer smaller than that is not
        // findable on screen, and a settings panel offering it is offering a way to lose the
        // cursor with no obvious way back.
        kind: Kind::Int {
            default: None,
            min: 8,
            max: 512,
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Cursor size",
        description: "Nominal, not literal: a theme carries whichever sizes its author drew and \
                      the nearest is used. sgi has 32 and nothing else, which is what the system \
                      default config asks for -- anything else would get the same images and \
                      tell every client to resample them.",
    },
    Setting {
        key: "compositor.idle.blank_after_secs",
        file: File::Compositor,
        path: &["idle", "blank_after_secs"],
        // No default: absent and 0 both mean "never", and the compositor's own docs ask for
        // the section to be left out entirely on an ordinary install.
        kind: Kind::Int {
            default: None,
            min: 0,
            max: A_DAY,
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::Seconds,
        summary: "Blank the screen after (deprecated)",
        description: "Deprecated: wlrix-idle owns idle policy for a wlRIX session and is \
                      started as part of it, so this should normally be left unset. Two idle \
                      timers on one screen fail in a way that is hard to diagnose -- a blank a \
                      client asked for is left alone by input, so once this one has fired \
                      against wlrix-idle's back, nothing switches the screens on again.",
    },
    // `[[output]]` is deliberately not declared. Four reasons, and the README carries them in
    // full: it is a keyed collection rather than a leaf; `outputs::resolve` overlays the
    // machine-written `outputs.toml` on top of it per field, so a hand-set mode is silently
    // overridden the moment the compositor next saves; `reload_config` never re-runs `resolve`,
    // so nothing would take effect before a restart anyway; and `wlr-output-management` already
    // exists in the compositor, is live, is atomic and has a test-and-rollback flow. A Displays
    // panel should speak that, not this.

    // ---------------------------------------------------------------------------------------
    // desktop.toml -- wlrix-desktop/src/config.rs
    //
    // `Reload::Live`, since wlrix-desktop gained a pidfile and a SIGHUP handler alongside this
    // daemon: its `reload_config` re-reads the file, drops the icon cache and lays the desktop
    // out again.
    //
    // `output` is the exception and stays `Restart`: moving the icons to another monitor means
    // tearing down the layer surface and building a new one against a different output, which
    // is a restart's worth of work for a setting nobody changes twice.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "desktop.snap_to_grid",
        file: File::Desktop,
        path: &["snap_to_grid"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Snap icons to a grid",
        description: "The starting value only. The user can flip this at runtime and the state \
                      file remembers what they chose, which then wins over this.",
    },
    Setting {
        key: "desktop.output",
        file: File::Desktop,
        path: &["output"],
        kind: Kind::Str { default: None },
        owner: Owner::Desktop,
        // The one desktop setting a reload cannot apply: the layer surface is bound to an
        // output, and moving it means building a new one. wlrix-desktop says so in its own log
        // when it sees this change.
        reload: Reload::Restart,
        unit: Unit::None,
        summary: "Monitor for desktop icons",
        description: "Which monitor the icons appear on, by connector name (DP-1, HDMI-A-1, \
                      Virtual-1). Empty lets wlrix-desktop work it out, which means the \
                      leftmost.",
    },
    Setting {
        key: "desktop.open",
        file: File::Desktop,
        path: &["open"],
        kind: Kind::StrList { default: &[] },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Command that opens a file",
        description: "The command and its leading arguments; the file or URL is appended. \
                      Empty means xdg-open, which consults the user's MIME associations.",
    },
    Setting {
        key: "desktop.terminal",
        file: File::Desktop,
        path: &["terminal"],
        kind: Kind::StrList { default: &[] },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Terminal for Terminal=true launchers",
        description: "The command and its leading arguments that wrap a console program; the \
                      program is appended. Empty lets wlrix-desktop work one out.",
    },
    Setting {
        key: "desktop.metrics.icon",
        file: File::Desktop,
        path: &["metrics", "icon"],
        kind: Kind::Int {
            default: Some(64),
            min: 8,
            max: 512,
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Icon size",
        description: "The icon artwork, square. A cell is never smaller than its icon, so \
                      raising this raises the cell with it.",
    },
    Setting {
        key: "desktop.metrics.cell_width",
        file: File::Desktop,
        path: &["metrics", "cell_width"],
        kind: Kind::Int {
            default: Some(96),
            min: 8,
            max: 1024,
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Cell width",
        description: "One cell holds the icon and its label. Too narrow and long filenames are \
                      cut.",
    },
    Setting {
        key: "desktop.metrics.cell_height",
        file: File::Desktop,
        path: &["metrics", "cell_height"],
        kind: Kind::Int {
            default: Some(104),
            min: 8,
            max: 1024,
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Cell height",
        description: "The default is the icon, a small gap, and two lines of label with a \
                      little slack. Shave a few pixels off and the second label line silently \
                      stops fitting, which shows up as every long filename being cut after one \
                      line.",
    },
    Setting {
        key: "desktop.metrics.gap",
        file: File::Desktop,
        path: &["metrics", "gap"],
        kind: Kind::Int {
            default: Some(8),
            min: 0,
            max: 256,
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Gap between cells",
        description: "Space between one cell and the next, in both directions.",
    },
    Setting {
        key: "desktop.metrics.margin",
        file: File::Desktop,
        path: &["metrics", "margin"],
        kind: Kind::Int {
            default: Some(12),
            min: 0,
            max: 512,
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Margin at the screen edge",
        description: "Space between the outermost cells and the edge of the monitor.",
    },
    // ---------------------------------------------------------------------------------------
    // idle.toml -- wlrix-idle/src/config.rs
    //
    // `[[timeout]]` is absent for the same reason as `[[output]]`: it is a collection, and one
    // countdown is four fields that only mean anything together. It is also the section most
    // worth a bespoke UI rather than a generic one.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "idle.lock.command",
        file: File::Idle,
        path: &["lock", "command"],
        kind: Kind::Str { default: None },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Screen locker",
        description: "The locker to run, e.g. \"swaylock -f -c 000000\". Nothing by default: \
                      which locker a session uses is a choice, and guessing at one that is not \
                      installed would turn every lock into a silent no-op.",
    },
    Setting {
        key: "idle.before_sleep.lock",
        file: File::Idle,
        path: &["before_sleep", "lock"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Lock before suspending",
        description: "Run the locker while logind holds the machine back from suspending, so \
                      the screen is already locked when it wakes.",
    },
    Setting {
        key: "idle.before_sleep.blank",
        file: File::Idle,
        path: &["before_sleep", "blank"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Blank before suspending",
        description: "Switch the monitors off before the machine suspends.",
    },
    Setting {
        key: "idle.before_sleep.command",
        file: File::Idle,
        path: &["before_sleep", "command"],
        kind: Kind::Str { default: None },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Command to run before suspending",
        description: "Run through sh -c, so quoting and pipes work. It has to finish inside \
                      the delay below.",
    },
    Setting {
        key: "idle.before_sleep.timeout_secs",
        file: File::Idle,
        path: &["before_sleep", "timeout_secs"],
        kind: Kind::Int {
            default: Some(4),
            min: 1,
            max: 20,
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::Seconds,
        summary: "How long to hold suspend back",
        description: "logind's InhibitDelayMaxSec is five seconds by default; past that it \
                      suspends regardless, so a longer wait here only means losing the race \
                      with the lock half-run.",
    },
    Setting {
        key: "idle.gamepad.enable",
        file: File::Idle,
        path: &["gamepad", "enable"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Count controller input as activity",
        description: "libinput classifies a gamepad as a joystick and drops it, so the \
                      compositor never sees a stick move and its idle timer never resets. \
                      wlrix-idle reads controllers straight from evdev to fix exactly that.",
    },
    Setting {
        key: "idle.gamepad.deadzone",
        file: File::Idle,
        path: &["gamepad", "deadzone"],
        kind: Kind::Float {
            default: Some(0.25),
            min: 0.02,
            max: 0.90,
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Stick deadzone",
        description: "How far a stick has to move before it counts, as a fraction of its \
                      travel from rest to the end of the axis. Triggers and d-pads rest at one \
                      end, so every press clears it.",
    },
    Setting {
        key: "idle.gamepad.min_interval_ms",
        file: File::Idle,
        path: &["gamepad", "min_interval_ms"],
        kind: Kind::Int {
            default: Some(1000),
            min: 0,
            max: 60_000,
        },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::Milliseconds,
        summary: "Least time between reports",
        description: "One device cannot report activity more often than this. A stick held \
                      over would otherwise wake the idle timer thousands of times a second.",
    },
    Setting {
        key: "idle.gamepad.allow",
        file: File::Idle,
        path: &["gamepad", "allow"],
        kind: Kind::StrList { default: &[] },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Only these controllers",
        description: "Case-insensitive substrings of the device name. Empty means every \
                      controller.",
    },
    Setting {
        key: "idle.gamepad.deny",
        file: File::Idle,
        path: &["gamepad", "deny"],
        kind: Kind::StrList { default: &[] },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Never these controllers",
        description: "Case-insensitive substrings of the device name, for a device that \
                      reports movement on its own.",
    },
    Setting {
        key: "idle.gamepad.devices",
        file: File::Idle,
        path: &["gamepad", "devices"],
        kind: Kind::StrList { default: &[] },
        owner: Owner::Idle,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Controllers to use regardless",
        description: "Paths under /dev/input to treat as controllers whatever the detection \
                      thinks, for the one nobody's heuristic gets right.",
    },
    Setting {
        key: "idle.dbus.screensaver",
        file: File::Idle,
        path: &["dbus", "screensaver"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Idle,
        // Not Live. wlrix-idle's own reload refuses this section by name: dropping and
        // retaking a bus name would silently void every inhibit applications are holding right
        // now, and open a gap for another desktop's daemon to take the name.
        reload: Reload::Restart,
        unit: Unit::None,
        summary: "Serve org.freedesktop.ScreenSaver",
        description: "How Firefox, mpv and Steam say \"not now\" while something is playing. \
                      Turning it off means the screen blanks during a film.",
    },
    Setting {
        key: "idle.dbus.power_management",
        file: File::Idle,
        path: &["dbus", "power_management"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Idle,
        reload: Reload::Restart,
        unit: Unit::None,
        summary: "Serve org.freedesktop.PowerManagement.Inhibit",
        description: "The older inhibit interface, which some applications still use instead \
                      of the ScreenSaver one.",
    },
    Setting {
        key: "idle.dbus.logind",
        file: File::Idle,
        path: &["dbus", "logind"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Idle,
        reload: Reload::Restart,
        unit: Unit::None,
        summary: "Take logind's delay inhibitor",
        description: "What makes the before-suspend actions above possible at all. With this \
                      off, the machine suspends without waiting for them.",
    },
    Setting {
        key: "idle.dbus.replace",
        file: File::Idle,
        path: &["dbus", "replace"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Idle,
        reload: Reload::Restart,
        unit: Unit::None,
        summary: "Take the bus names from whoever holds them",
        description: "For running wlrix-idle inside another desktop, which already owns them. \
                      Taking a live KDE session's inhibit handling away breaks it, so this is \
                      off unless asked for.",
    },
    // ---------------------------------------------------------------------------------------
    // portal.toml -- xdg-desktop-portal-wlrix/src/config.rs
    //
    // `Reload::None`, and deliberately so: the portal installs no reload handler, and its own
    // signals.rs says why -- a screen share is not something to reconfigure underneath. The
    // right action is `systemctl --user try-restart`, but only when no cast is live, which the
    // daemon has no way to know. So it reports and leaves it to the user.
    //
    // `[preview] tile` is absent: it is a fixed-length pair of numbers, which none of the
    // scalar kinds describe and which a wrong-length list would turn into a parse failure.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "portal.preview.tick_ms",
        file: File::Portal,
        path: &["preview", "tick_ms"],
        kind: Kind::Int {
            default: Some(100),
            min: 1,
            max: 60_000,
        },
        owner: Owner::Portal,
        reload: Reload::None,
        unit: Unit::Milliseconds,
        summary: "Preview refresh interval",
        description: "How often one source is captured for the screen-share picker. Not the \
                      refresh rate of a tile: sources take turns, so with ten of them the \
                      default refreshes each about once a second. Lower it to make the grid \
                      livelier and the readback more expensive.",
    },
    // ---------------------------------------------------------------------------------------
    // session.toml -- wlrix-session/src/config.rs
    //
    // Read once, before there is a compositor. `[[app]]` and `[env]` are a collection and a
    // free-form map; both wait for the collection surface.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "session.compositor",
        file: File::Session,
        path: &["compositor"],
        kind: Kind::Str { default: None },
        owner: Owner::None,
        reload: Reload::NextLogin,
        unit: Unit::None,
        summary: "Compositor to run",
        description: "Empty leaves the built-in default, wlrix-compositor. Changing this is \
                      how you run a patched build for one session without touching the \
                      installed one.",
    },
];
