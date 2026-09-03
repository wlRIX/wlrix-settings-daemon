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
            // Its own tests assert that "tiled", "center", "zoom" and every capitalized form are
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
        key: "compositor.focus.raise_on_click",
        file: File::Compositor,
        path: &["focus", "raise_on_click"],
        // The compositor writes `FocusConfig::default` out by hand precisely so that this stays
        // true for a config with no `[focus]` section at all; a default of `false` here would
        // describe a compositor that does not exist.
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Clicking a window raises it",
        description: "On, clicking anywhere in a window brings it to the front. Off separates \
                      focusing from restacking: the click still gives the window the keyboard, \
                      but the stacking order only changes deliberately -- through the window's \
                      4Dwm frame, or Raise and Lower in its window menu. Frame clicks raise \
                      either way.",
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
    Setting {
        key: "compositor.appearance.palette",
        file: File::Compositor,
        path: &["appearance", "palette"],
        kind: Kind::Str { default: None },
        owner: Owner::Compositor,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Color scheme",
        description: "A scheme id from wlrix-ui. Empty or unrecognized means the default, with \
                      a line in the log for the latter -- a mistyped scheme name must not leave \
                      somebody with no session at all. Normally written through the \
                      `appearance.palette` group rather than on its own, so the chrome and the \
                      rest of the desktop move together.",
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
    // `appearance.palette` is declared here *and* in three other files, and is normally written
    // through the `appearance.palette` group in `schema::group`, which fans one write out to all
    // four and signals all four owners. This entry is what that expands to; it is still settable
    // on its own for a session that genuinely wants one component different.
    //
    // `appearance.icon_theme` has no group and wants none. It is genuinely per component: the
    // desktop draws 64-pixel launcher symbols and the tray draws 22-pixel cells, and the theme
    // that suits one need not suit the other.
    Setting {
        key: "desktop.appearance.palette",
        file: File::Desktop,
        path: &["appearance", "palette"],
        kind: Kind::Str { default: None },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Color scheme",
        description: "A scheme id from wlrix-ui. Empty or unrecognized means the default, with \
                      a line in the log for the latter -- a mistyped scheme name must not leave \
                      the desktop unpainted.",
    },
    Setting {
        key: "desktop.appearance.icon_theme",
        file: File::Desktop,
        path: &["appearance", "icon_theme"],
        kind: Kind::Str {
            default: Some("Adwaita"),
        },
        owner: Owner::Desktop,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Icon theme an Icon= name is looked up in",
        description: "Searched before hicolor and /usr/share/pixmaps, which between them have \
                      almost nothing a .desktop file names -- a launcher whose icon is not found \
                      is drawn as a bare magic carpet with no symbol on it. Empty means no named \
                      theme.",
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
    // screenshot.toml -- wlrix-screenshot/src/config.rs
    //
    // `Reload::None` throughout, and for a different reason from the portal's: there is nothing
    // running to tell. `wlrix-screenshot` is spawned per screenshot and reads its config each
    // time, so a change is in effect for the next one -- which is as immediate as it gets.
    //
    // `[appearance] dim` is here as a float; `[save] filename` is a strftime template and is a
    // plain string, since its validity is the C library's opinion rather than a pattern this
    // table could state.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "screenshot.save.dir",
        file: File::Screenshot,
        path: &["save", "dir"],
        kind: Kind::Str { default: None },
        owner: Owner::Screenshot,
        reload: Reload::None,
        unit: Unit::None,
        summary: "Where screenshots are saved",
        description: "Empty means the XDG pictures directory plus Screenshots. A leading ~/ is \
                      expanded; the directory is created when the first shot is saved.",
    },
    Setting {
        key: "screenshot.save.filename",
        file: File::Screenshot,
        path: &["save", "filename"],
        kind: Kind::Str {
            default: Some("Screenshot_%Y-%m-%d_%H-%M-%S"),
        },
        owner: Owner::Screenshot,
        reload: Reload::None,
        unit: Unit::None,
        summary: "Filename template",
        description: "A strftime template, without the extension. Every conversion strftime(3) \
                      documents works. A second shot in the same second gets -2 appended \
                      rather than overwriting the first.",
    },
    Setting {
        key: "screenshot.capture.cursor",
        file: File::Screenshot,
        path: &["capture", "cursor"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Screenshot,
        reload: Reload::None,
        unit: Unit::None,
        summary: "Draw the pointer into the shot",
        description: "Off by default: the pointer is usually somewhere incidental when the key \
                      is pressed, and a shot of a menu is spoiled rather than explained by an \
                      arrow in the corner of it.",
    },
    Setting {
        key: "screenshot.appearance.palette",
        file: File::Screenshot,
        path: &["appearance", "palette"],
        kind: Kind::Str { default: None },
        owner: Owner::Screenshot,
        reload: Reload::None,
        unit: Unit::None,
        summary: "Color scheme",
        description: "A scheme id from wlrix-ui. Empty or unrecognized means the default, with \
                      a line on stderr for the latter.",
    },
    Setting {
        key: "screenshot.appearance.dim",
        file: File::Screenshot,
        path: &["appearance", "dim"],
        kind: Kind::Float {
            default: Some(0.55),
            min: 0.0,
            max: 1.0,
        },
        owner: Owner::Screenshot,
        reload: Reload::None,
        unit: Unit::None,
        summary: "How far the unselected area is darkened",
        description: "0.0 leaves it alone, 1.0 blacks it out. The default is dark enough that \
                      the selection reads as the subject and light enough that what is outside \
                      it is still recognizable -- which matters, because the point of adjusting \
                      a selection is seeing what you are about to leave out.",
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
    // ---------------------------------------------------------------------------------------
    // tray.toml -- wlrix-tray/src/config.rs
    //
    // `[[item]]` is absent for the usual reason: it is an array of tables keyed by an item's
    // D-Bus `Id`, and a keyed collection needs add/remove/reorder rather than get/set.
    // ---------------------------------------------------------------------------------------
    Setting {
        key: "tray.output",
        file: File::Tray,
        path: &["output"],
        kind: Kind::Str { default: None },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Which monitor the tray is on",
        description: "A connector name (DP-1, HDMI-A-1). Empty means the leftmost. A name that \
                      is not plugged in falls back to the leftmost with a line on stderr.",
    },
    Setting {
        key: "tray.anchor",
        file: File::Tray,
        path: &["anchor"],
        kind: Kind::Enum {
            default: Some("bottom-left"),
            // Exactly what `#[serde(rename_all = "kebab-case")]` on wlrix-tray's `Anchor`
            // accepts. Its own tests assert that "bottom left" is rejected, so offering a
            // spaced or capitalized form here would cost the user their whole file.
            choices: &[
                Choice {
                    value: "bottom-left",
                    label: "Bottom left",
                },
                Choice {
                    value: "bottom-right",
                    label: "Bottom right",
                },
                Choice {
                    value: "top-left",
                    label: "Top left",
                },
                Choice {
                    value: "top-right",
                    label: "Top right",
                },
            ],
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Which corner the tray docks in",
        description: "IRIX put it bottom left, which is the default. The strip fills away from \
                      the corner and wraps inward, so it grows onto the desktop rather than \
                      off the screen.",
    },
    Setting {
        key: "tray.orientation",
        file: File::Tray,
        path: &["orientation"],
        kind: Kind::Enum {
            default: Some("horizontal"),
            choices: &[
                Choice {
                    value: "horizontal",
                    label: "Along the screen edge",
                },
                Choice {
                    value: "vertical",
                    label: "Inward from the edge",
                },
            ],
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Which way the strip runs",
        description: "Horizontal runs along the screen edge and wraps to a second row inward; \
                      vertical runs inward and wraps to a second column sideways.",
    },
    Setting {
        key: "tray.show_passive",
        file: File::Tray,
        path: &["show_passive"],
        kind: Kind::Bool {
            default: Some(false),
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Show items that are not asking for attention",
        description: "The specification lets a tray hide an item whose Status is Passive, and \
                      applications rely on it -- several park an item there permanently. On, \
                      the strip becomes a list of everything that has ever started.",
    },
    Setting {
        key: "tray.hide_when_empty",
        file: File::Tray,
        path: &["hide_when_empty"],
        kind: Kind::Bool {
            default: Some(true),
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Disappear when there is nothing to show",
        description: "Off leaves an empty beveled well on the desktop, which makes the tray \
                      discoverable -- there is somewhere for an icon to appear.",
    },
    Setting {
        key: "tray.appearance.palette",
        file: File::Tray,
        path: &["appearance", "palette"],
        kind: Kind::Str { default: None },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Color scheme",
        description: "A scheme id from wlrix-ui. Empty or unrecognized means the default, with \
                      a line on stderr for the latter.",
    },
    Setting {
        key: "tray.appearance.icon_theme",
        file: File::Tray,
        path: &["appearance", "icon_theme"],
        kind: Kind::Str {
            default: Some("Adwaita"),
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Icon theme an item's IconName is looked up in",
        description: "Searched before hicolor and /usr/share/pixmaps, which between them have \
                      almost nothing a tray wants -- fcitx5's input-keyboard-symbolic is in \
                      every other theme and neither of those. Empty means no named theme.",
    },
    Setting {
        key: "tray.metrics.icon",
        file: File::Tray,
        path: &["metrics", "icon"],
        kind: Kind::Int {
            default: Some(22),
            min: 8,
            max: 128,
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Icon size",
        description: "22 is what almost every IconPixmap on the bus arrives at, so it is the \
                      one size that needs no scaling.",
    },
    Setting {
        key: "tray.metrics.cell",
        file: File::Tray,
        path: &["metrics", "cell"],
        kind: Kind::Int {
            default: Some(28),
            min: 8,
            max: 160,
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Cell size",
        description: "One cell: the icon plus the room its bevel and highlight need. Floored at \
                      the icon size rather than refused.",
    },
    Setting {
        key: "tray.metrics.gap",
        file: File::Tray,
        path: &["metrics", "gap"],
        kind: Kind::Int {
            default: Some(2),
            min: 0,
            max: 32,
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Gap between cells",
        description: "Zero puts the cells edge to edge, which is how a row of them reads as one \
                      strip rather than several buttons.",
    },
    Setting {
        key: "tray.metrics.margin",
        file: File::Tray,
        path: &["metrics", "margin"],
        kind: Kind::Int {
            default: Some(8),
            min: 0,
            max: 200,
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::Pixels,
        summary: "Distance from the screen edges",
        description: "How far the strip sits from the two edges it is anchored to.",
    },
    Setting {
        key: "tray.metrics.wrap_at",
        file: File::Tray,
        path: &["metrics", "wrap_at"],
        kind: Kind::Int {
            default: Some(8),
            min: 1,
            max: 64,
        },
        owner: Owner::Tray,
        reload: Reload::Live,
        unit: Unit::None,
        summary: "Cells per run before the strip wraps",
        description: "Named for what it does rather than max_columns: in vertical orientation \
                      the run is a column and the wrap makes a new one sideways.",
    },
];
