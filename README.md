# LiteCap

Low-RAM, cross-platform (Windows & Linux) screen recorder that lives in your system tray.

> ## ⚠️ IMPORTANT NOTE!
> This entire project — code, CI/release pipeline, and yes, this README included 😃 — was built entirely with AI assistance. It works, but it hasn't had the kind of scrutiny a hand-crafted project would get, so bugs, rough edges, or plain wrong assumptions are expected. If something breaks or looks off, please open an issue or a PR — all suggestions and contributions are very welcome.

## Download

Prebuilt binaries for Windows and Linux are published on the [Releases](https://github.com/Foelance/LiteCap/releases) page for every tagged version (`vX.Y.Z`), built and signed off by CI from `.github/workflows/release.yml`. `litecap.exe` is Windows-only — it will **not** run on Linux, not even under Wine, since screen capture uses the Windows Graphics Capture API. Linux users should download `litecap-linux-x86_64` from Releases (see [Linux: install & run](#linux-install--run) below), or build from source.

## Features

- **System tray app** — no window, no clutter. Start/stop recording from the tray menu or a global hotkey.
- **Low memory footprint** — captures frames natively (Windows Graphics Capture on Windows, PipeWire/xdg-desktop-portal on Linux) and streams them straight into `ffmpeg`, instead of buffering video in RAM.
- **Monitor selection** — pick which display to record from the tray's Monitor submenu.
- **Audio capture** — optionally include system sound and/or microphone input, mixed into the recording.
- **1920x1080 @ 60 FPS preset** — force a fixed output resolution/frame rate regardless of the source monitor's native mode (letterboxed if the aspect ratio differs).
- **Self-contained ffmpeg** — downloads and caches a portable `ffmpeg` build on first run; no manual install required.
- **Global hotkey** — configurable shortcut to start/stop recording without touching the tray.
- **Desktop notifications** — get notified when recording starts/stops or if something goes wrong.

## Building

Requires a recent stable Rust toolchain (`rustup`).

```sh
cargo build --release
```

### Linux dependencies

```sh
sudo apt-get install -y \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  libpipewire-0.3-dev \
  libclang-dev \
  libxdo-dev
```

Windows builds only need the stable Rust toolchain — platform capture/audio backends (`windows-capture`, `cpal`) are pulled in automatically.

## Linux: install & run

1. **Download** the latest `litecap-linux-x86_64` from the [Releases](https://github.com/Foelance/LiteCap/releases) page.
2. **Make it executable and run it:**
   ```sh
   chmod +x litecap-linux-x86_64
   ./litecap-linux-x86_64
   ```
   No installer — it's a single self-contained binary. Move it anywhere on your `$PATH` (e.g. `~/.local/bin/`) if you want to launch it by name.
3. **Runtime libraries** (usually already present on a desktop install; install if the binary refuses to start):
   ```sh
   sudo apt-get install -y libgtk-3-0 libayatana-appindicator3-1 libpipewire-0.3-0 libxdo3
   ```
   (Package names above are for Debian/Ubuntu; use your distro's equivalents, e.g. `gtk3`, `libappindicator-gtk3`, `pipewire`, `xdotool`-provided `libxdo` on Fedora/Arch.)
4. **First launch:** LiteCap appears as a tray icon (needs an AppIndicator-capable tray — on stock GNOME install the [AppIndicator/KStatusNotifierItem](https://extensions.gnome.org/extension/615/appindicator-support/) extension; KDE Plasma, XFCE, and most other DEs support it out of the box). It also downloads and caches a portable `ffmpeg` build on first run — no manual `ffmpeg` install needed.
5. **Screen capture permission:**
   - **Wayland sessions** — LiteCap requests capture through the `org.freedesktop.portal.ScreenCast` XDG portal via PipeWire. Your compositor needs a portal backend installed: `xdg-desktop-portal-gnome` (GNOME), `xdg-desktop-portal-kde` (KDE Plasma), or `xdg-desktop-portal-wlr` (Sway/Hyprland/other wlroots compositors). A system dialog will ask you to pick a monitor/window the first time you start recording; grant it.
   - **X11 sessions** — capture goes through ffmpeg's `x11grab` directly, no portal dialog. Monitor detection uses `xrandr`, so make sure it's installed (`sudo apt-get install x11-xserver-utils`) for correct per-monitor recording.
6. **Recording:** click the tray icon (or press the configured global hotkey) to start/stop. Pick a monitor from the tray's Monitor submenu, toggle system audio/microphone and the 1920x1080@60 preset from Options. Finished recordings land in your Videos folder by default (configurable — see [Configuration](#configuration)).

If the tray icon never appears, run it from a terminal to see error output: `./litecap-linux-x86_64` (no `windows_subsystem` hiding on Linux, so logs print to stdout/stderr).

## Configuration

LiteCap stores its config as TOML under the platform's standard config directory (via [`directories`](https://docs.rs/directories)), e.g. `%APPDATA%\litecap\litecap\config.toml` on Windows or `~/.config/litecap/config.toml` on Linux. Recordings are saved to a configurable output directory (defaults to your Videos folder) and can be opened directly from the tray menu.

## CI

GitHub Actions runs `cargo check --release` on both Windows and Linux for every push/PR (see `.github/workflows/ci.yml`), and builds + publishes release binaries for both platforms whenever a `v*` tag is pushed (see `.github/workflows/release.yml`).

## License

No license specified yet.
