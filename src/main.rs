#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio_net;
mod capture;
mod config;
mod ffmpeg;
mod icon;
mod recorder;

#[cfg(windows)]
mod audio_win;

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use config::Config;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use recorder::Recording;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// Owns everything the tray/hotkey callbacks need to mutate; lives on the
/// event-loop thread only, per the plan's state-machine design.
enum AppState {
    PreparingFfmpeg,
    Idle {
        ffmpeg: std::path::PathBuf,
        encoder: ffmpeg::VideoEncoder,
    },
    Recording {
        ffmpeg: std::path::PathBuf,
        encoder: ffmpeg::VideoEncoder,
        recording: Recording,
    },
}

struct MenuItems {
    start_stop: MenuItem,
    monitor_items: Vec<CheckMenuItem>,
    system_audio: CheckMenuItem,
    microphone: CheckMenuItem,
    preset_1080p60: CheckMenuItem,
    open_folder: MenuItem,
    quit: MenuItem,
}

struct App {
    cfg: Config,
    state: AppState,
    tray: TrayIcon,
    items: MenuItems,
    _hotkey_manager: Option<GlobalHotKeyManager>,
    hotkey_id: Option<u32>,
    quit_requested: bool,
}

fn notify(summary: &str, body: &str) {
    if let Err(e) = notify_rust::Notification::new().summary(summary).body(body).show() {
        log::warn!("failed to show notification {summary:?}: {e}");
    }
}

#[cfg(windows)]
fn monitor_names() -> Vec<String> {
    capture::win::monitor_names()
}
#[cfg(target_os = "linux")]
fn monitor_names() -> Vec<String> {
    // On Wayland the portal's own picker replaces the tray submenu.
    if capture::x11::is_wayland_session() {
        Vec::new()
    } else {
        capture::x11::monitors().into_iter().map(|m| m.name).collect()
    }
}

fn build_menu(cfg: &Config) -> (Menu, MenuItems) {
    let menu = Menu::new();

    let start_stop = MenuItem::new("Start Recording", true, None);
    menu.append(&start_stop).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();

    let monitor_submenu = Submenu::new("Monitor", true);
    let names = monitor_names();
    let mut monitor_items = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let item = CheckMenuItem::new(name, true, i == cfg.monitor_index, None);
        monitor_submenu.append(&item).ok();
        monitor_items.push(item);
    }
    if !names.is_empty() {
        menu.append(&monitor_submenu).ok();
    }

    let options_submenu = Submenu::new("Options", true);
    let system_audio = CheckMenuItem::new("System Sounds", true, cfg.system_audio, None);
    let microphone = CheckMenuItem::new("Microphone", true, cfg.microphone, None);
    let preset_1080p60 = CheckMenuItem::new("1920x1080 @ 60 FPS", true, cfg.preset_1080p60, None);
    options_submenu.append(&microphone).ok();
    options_submenu.append(&system_audio).ok();
    options_submenu.append(&PredefinedMenuItem::separator()).ok();
    options_submenu.append(&preset_1080p60).ok();
    menu.append(&options_submenu).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();

    let open_folder = MenuItem::new("Open recordings folder", true, None);
    menu.append(&open_folder).ok();
    menu.append(&PredefinedMenuItem::separator()).ok();

    let quit = MenuItem::new("Quit", true, None);
    menu.append(&quit).ok();

    (
        menu,
        MenuItems {
            start_stop,
            monitor_items,
            system_audio,
            microphone,
            preset_1080p60,
            open_folder,
            quit,
        },
    )
}

impl App {
    fn toggle_recording(&mut self) {
        match std::mem::replace(&mut self.state, AppState::PreparingFfmpeg) {
            AppState::Idle { ffmpeg, encoder } => {
                match recorder::start(&self.cfg, &ffmpeg, encoder) {
                    Ok(recording) => {
                        notify("LiteCap", "Recording started");
                        self.items.start_stop.set_text("Stop Recording (00:00)");
                        self.tray.set_icon(Some(icon::recording())).ok();
                        self.state = AppState::Recording {
                            ffmpeg,
                            encoder,
                            recording,
                        };
                    }
                    Err(e) => {
                        log::error!("failed to start recording: {e}");
                        notify("LiteCap", &format!("Failed to start recording: {e}"));
                        self.state = AppState::Idle { ffmpeg, encoder };
                    }
                }
            }
            AppState::Recording {
                ffmpeg,
                encoder,
                recording,
            } => match recording.stop() {
                Ok(path) => {
                    let size_mb = std::fs::metadata(&path).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
                    notify(
                        "LiteCap",
                        &format!("Saved {} ({size_mb:.1} MB)", path.file_name().unwrap_or_default().to_string_lossy()),
                    );
                    self.items.start_stop.set_text("Start Recording");
                    self.tray.set_icon(Some(icon::idle())).ok();
                    self.state = AppState::Idle { ffmpeg, encoder };
                }
                Err(e) => {
                    log::error!("failed to stop recording cleanly: {e}");
                    notify("LiteCap", "Recording stopped with errors, see litecap.log");
                    self.items.start_stop.set_text("Start Recording");
                    self.tray.set_icon(Some(icon::idle())).ok();
                    self.state = AppState::Idle { ffmpeg, encoder };
                }
            },
            other => self.state = other,
        }
    }

    fn stop_if_recording(&mut self) {
        if matches!(self.state, AppState::Recording { .. }) {
            self.toggle_recording();
        }
    }

    fn handle_menu_event(&mut self, id: &tray_icon::menu::MenuId) {
        if id == self.items.start_stop.id() {
            self.toggle_recording();
        } else if id == self.items.open_folder.id() {
            open_folder(&self.cfg.output_dir);
        } else if id == self.items.system_audio.id() {
            self.cfg.system_audio = self.items.system_audio.is_checked();
            let _ = self.cfg.save();
        } else if id == self.items.microphone.id() {
            self.cfg.microphone = self.items.microphone.is_checked();
            let _ = self.cfg.save();
        } else if id == self.items.preset_1080p60.id() {
            self.cfg.preset_1080p60 = self.items.preset_1080p60.is_checked();
            let _ = self.cfg.save();
        } else if id == self.items.quit.id() {
            self.stop_if_recording();
            self.quit_requested = true;
        } else if let Some(idx) = self.items.monitor_items.iter().position(|m| m.id() == id) {
            self.cfg.monitor_index = idx;
            for (i, m) in self.items.monitor_items.iter().enumerate() {
                m.set_checked(i == idx);
            }
            let _ = self.cfg.save();
        }
    }

    fn update_timer_label(&mut self) {
        if let AppState::Recording { recording, .. } = &self.state {
            let secs = recording.started_at.elapsed().as_secs();
            self.items
                .start_stop
                .set_text(format!("Stop Recording ({:02}:{:02})", secs / 60, secs % 60));
        }
    }
}

fn open_folder(dir: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn ensure_ffmpeg_ready() -> anyhow::Result<(std::path::PathBuf, ffmpeg::VideoEncoder)> {
    let path = match ffmpeg::locate() {
        Some(p) => p,
        None => {
            notify("LiteCap", "Downloading FFmpeg…");
            let p = ffmpeg::download()?;
            notify("LiteCap", "FFmpeg ready");
            p
        }
    };
    let encoder = ffmpeg::probe_encoder(&path);
    log::info!("using encoder {encoder:?}");
    Ok((path, encoder))
}

fn register_hotkey(cfg: &Config) -> (Option<GlobalHotKeyManager>, Option<u32>) {
    match GlobalHotKeyManager::new() {
        Ok(mgr) => match HotKey::from_str(&cfg.hotkey) {
            Ok(hotkey) => match mgr.register(hotkey) {
                Ok(()) => (Some(mgr), Some(hotkey.id())),
                Err(e) => {
                    log::warn!("failed to register global hotkey {}: {e}", cfg.hotkey);
                    (Some(mgr), None)
                }
            },
            Err(e) => {
                log::warn!("invalid hotkey string {:?}: {e}", cfg.hotkey);
                (Some(mgr), None)
            }
        },
        Err(e) => {
            log::warn!("global hotkey manager unavailable: {e}");
            (None, None)
        }
    }
}

fn main() {
    env_logger::init();
    let cfg = Config::load();

    let (menu, items) = build_menu(&cfg);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("LiteCap")
        .with_icon(icon::idle())
        .build()
        .expect("failed to create tray icon");

    let (hotkey_manager, hotkey_id) = register_hotkey(&cfg);

    let app = App {
        cfg,
        state: AppState::PreparingFfmpeg,
        tray,
        items,
        _hotkey_manager: hotkey_manager,
        hotkey_id,
        quit_requested: false,
    };

    // Prepare ffmpeg on a worker thread; the tray is already up so the user
    // sees the app immediately. Start stays effectively unavailable (any
    // toggle attempt while PreparingFfmpeg is a no-op) until this completes.
    let ready = Arc::new(AtomicBool::new(false));
    let ready_result: Arc<std::sync::Mutex<Option<anyhow::Result<(std::path::PathBuf, ffmpeg::VideoEncoder)>>>> =
        Arc::new(std::sync::Mutex::new(None));
    {
        let ready = ready.clone();
        let ready_result = ready_result.clone();
        app.items.start_stop.set_text("Preparing FFmpeg…");
        std::thread::spawn(move || {
            let result = ensure_ffmpeg_ready();
            *ready_result.lock().unwrap() = Some(result);
            ready.store(true, Ordering::Release);
        });
    }

    run_event_loop(app, ready, ready_result);
}

fn poll_channels(app: &mut App) {
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        app.handle_menu_event(&event.id);
    }
    while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        if event.state == global_hotkey::HotKeyState::Pressed
            && app.hotkey_id.is_some()
            && !matches!(app.state, AppState::PreparingFfmpeg)
        {
            app.toggle_recording();
        }
    }
    app.update_timer_label();
}

fn poll_ffmpeg_ready(
    app: &mut App,
    ready: &AtomicBool,
    ready_result: &std::sync::Mutex<Option<anyhow::Result<(std::path::PathBuf, ffmpeg::VideoEncoder)>>>,
) {
    if matches!(app.state, AppState::PreparingFfmpeg) && ready.load(Ordering::Acquire) {
        if let Some(result) = ready_result.lock().unwrap().take() {
            match result {
                Ok((ffmpeg, encoder)) => {
                    app.items.start_stop.set_text("Start Recording");
                    app.state = AppState::Idle { ffmpeg, encoder };
                }
                Err(e) => {
                    log::error!("ffmpeg unavailable: {e}");
                    notify(
                        "LiteCap",
                        "FFmpeg unavailable. Install manually: winget install Gyan.FFmpeg / sudo apt install ffmpeg",
                    );
                    app.items.start_stop.set_text("FFmpeg unavailable");
                }
            }
        }
    }
}

#[cfg(windows)]
fn run_event_loop(
    mut app: App,
    ready: Arc<AtomicBool>,
    ready_result: Arc<std::sync::Mutex<Option<anyhow::Result<(std::path::PathBuf, ffmpeg::VideoEncoder)>>>>,
) {
    use tao::event_loop::{ControlFlow, EventLoop};

    let event_loop = EventLoop::new();
    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));
        poll_ffmpeg_ready(&mut app, &ready, &ready_result);
        poll_channels(&mut app);
        if app.quit_requested {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "linux")]
fn run_event_loop(
    app: App,
    ready: Arc<AtomicBool>,
    ready_result: Arc<std::sync::Mutex<Option<anyhow::Result<(std::path::PathBuf, ffmpeg::VideoEncoder)>>>>,
) {
    gtk::init().expect("failed to initialize gtk");

    let app = Rc::new(RefCell::new(app));

    {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let mut app = app.borrow_mut();
            poll_ffmpeg_ready(&mut app, &ready, &ready_result);
            poll_channels(&mut app);
            if app.quit_requested {
                gtk::main_quit();
            }
            glib::ControlFlow::Continue
        });
    }

    gtk::main();
}

#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::rc::Rc;
