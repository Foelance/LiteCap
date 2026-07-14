# LiteCap

Low-RAM, cross-platform (Windows & Linux) screen recorder that lives in your system tray.

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

## Configuration

LiteCap stores its config as TOML under the platform's standard config directory (via [`directories`](https://docs.rs/directories)), e.g. `%APPDATA%\litecap\litecap\config.toml` on Windows or `~/.config/litecap/config.toml` on Linux. Recordings are saved to a configurable output directory (defaults to your Videos folder) and can be opened directly from the tray menu.

## CI

GitHub Actions runs `cargo check --release` on both Windows and Linux for every push/PR (see `.github/workflows/ci.yml`).

## License

No license specified yet.
