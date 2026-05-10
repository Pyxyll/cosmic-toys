# cosmic-toys

A PowerToys-style toolbox for the COSMIC desktop. One app, one settings page, one set of hotkeys — and a shared daemon underneath so each tool's hotkey works whether the GUI is open or not.

The project started as a one-tool fix: `hyprpicker` doesn't run on COSMIC (`cosmic-comp` doesn't expose `zwlr_screencopy_v1`) and `xdg-desktop-portal-cosmic`'s `PickColor` is a `// XXX implement` stub. Color picker came first; the toolbox grew around it.

![demo](demo.gif)

## Tools

| Tool | What it does |
|---|---|
| **Color Picker** | Magnifier-on-cursor overlay, pixel-precision sampling, clipboard delivery, HEX/RGB/HSL/HSV/OKLCH readouts, recents history. |

More tools land in upcoming releases (Mouse Find in `0.3.0`, Screen Ruler in `0.3.1`).

> The Color Picker tool is a stopgap until `xdg-desktop-portal-cosmic`'s `PickColor` ships natively. Other tools are not.

## What's in the box

| Binary | Job |
|---|---|
| `cosmic-toysd` | Headless daemon. Owns the IPC socket, runs each tool's background work on demand, persists state. Auto-starts at login via the systemd user unit. |
| `cosmic-toys` | GUI. Sidebar nav with one entry per tool, plus Settings (hotkey binder + autostart toggle) and About. |
| `cosmic-toys-applet` | Panel applet. Quick access to the most-used tool actions. |

Tools, hotkeys, and the GUI all funnel through the same daemon, so any state (e.g., picked colors) is visible to every entry point.

## Install

Pick whichever matches your distro. After install, enable the daemon once:

```sh
systemctl --user enable --now cosmic-toysd
```

Add the panel applet via **COSMIC Settings → Panel → Configure panel applets → cosmic-toys Applet**. The GUI shows up in the launcher as "cosmic-toys."

> **Upgrading from `cosmic-color-picker` v0.2.x?** The first launch of `cosmic-toys` migrates your history from `~/.config/cosmic/com.pyxyll.CosmicColorPicker/v1/` to the new namespace automatically. The old autostart entry is removed; re-toggle it from Settings if you want autostart. The old binaries (`cosmic-color-picker`, `cosmic-color-pickerd`, `cosmic-applet-color-picker`) can be uninstalled.

### Pop!_OS / Ubuntu / Debian

Download the `.deb` from the [latest release](https://github.com/Pyxyll/cosmic-toys/releases/latest):

```sh
sudo apt install ./cosmic-toys_*.deb
```

### Fedora / openSUSE

Download the `.rpm` from the [latest release](https://github.com/Pyxyll/cosmic-toys/releases/latest):

```sh
sudo rpm -i cosmic-toys-*.rpm
```

### Arch / CachyOS / EndeavourOS / Manjaro

```sh
yay -S cosmic-toys          # or paru / your AUR helper of choice
```

Or build the in-tree PKGBUILD directly:

```sh
git clone https://github.com/Pyxyll/cosmic-toys.git
cd cosmic-toys/dist/aur
makepkg -si
```

### Anything else (static tarball)

Grab the `cosmic-toys-*-x86_64-linux.tar.gz` from the [latest release](https://github.com/Pyxyll/cosmic-toys/releases/latest), extract, and copy the contents into your prefix of choice (`/usr/local/`, `~/.local/`, etc.).

### Build from source

Requires `rust >= 1.95`, `just`, plus runtime tools: `grim`, `wl-clipboard`, `libnotify`.

```sh
git clone https://github.com/Pyxyll/cosmic-toys.git
cd cosmic-toys
sudo just install
systemctl --user enable --now cosmic-toysd
```

`sudo just uninstall` removes everything.

## Usage

After install:

1. Open **cosmic-toys** from your launcher.
2. Pick a tool from the sidebar (Color Picker for now).
3. **Settings → Keyboard shortcut**: click the button, press your desired combo. COSMIC picks it up immediately. Esc cancels.
4. Hit your hotkey anywhere → the tool's overlay (magnifier for Color Picker) → result lands in clipboard + notification + the GUI's history.

## Caveats

- **Capture is a frozen screenshot via `grim`**, not live frames. Animations stop while you're picking. Same as basically every other color picker.
- **Magnifier doesn't appear until you move the mouse** after triggering. COSMIC doesn't fire `Pointer.Enter` for a fresh layer-shell surface, and seeding a default cursor position made it look broken on multi-monitor setups.
- **GUI X button kills the daemon if it was launched together.** libcosmic forces `iced::exit()` when the main window is closed (`core.exit_on_main_window_closed = true`, no public setter). The systemd user unit keeps the daemon up independently of the GUI's lifecycle, so this only matters if you started both manually.
- **Applet doesn't auto-reopen after pick.** Layer-shell popups need a recent-input-event grab serial; opening from a delayed task fails silently. Clippy-land hit the same wall.

## Architecture

```
[Hotkey]  ──spawn──>  cosmic-toys --pick ─┐
                                          │
                                          ├─IPC─> cosmic-toysd (daemon)
                                          │         ├── runs the tool's overlay
[Applet]  ──spawn──>  cosmic-toys --pick ─┤         ├── persists state per-tool
                                          │         └── responds with the result
[GUI Pick]──IPC─────> cosmic-toysd ───────┘

  ~/.config/cosmic/com.pyxyll.CosmicToys/v1/<state> (RON, watch_config)
        ▲                ▲                    ▲
        │ writes         │ watch_config       │ watch_config
      daemon            GUI                  applet
```

The daemon owns each tool's persistent state. GUI and applet are pure clients; both subscribe to the cosmic-config files via `watch_config`, so updates land in their UIs without explicit messaging.

## Development

```sh
cargo build --release --workspace          # all three binaries
cargo build --release -p cosmic-toys  # GUI only
cargo build --release -p cosmic-toysd # daemon only
cargo build --release -p cosmic-toys-applet # applet only
```

Source structure:

```
cosmic-toys/
├── daemon/    cosmic-toysd  (no libcosmic; pure tokio + sctk)
├── gui/       cosmic-toys   (libcosmic Application)
├── applet/    cosmic-toys-applet  (libcosmic applet)
└── dist/      systemd unit + AUR PKGBUILD
```

## License

MIT.
