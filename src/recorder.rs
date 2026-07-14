use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::capture::{FrameSlot, Stats, VideoSource};
use crate::config::Config;
use crate::ffmpeg::{FfmpegJob, VideoEncoder};

#[cfg(windows)]
use crate::audio_net;
#[cfg(windows)]
use crate::audio_win::{self, AudioStreams};
#[cfg(windows)]
use crate::capture::win;

#[cfg(target_os = "linux")]
use crate::capture::wayland;
#[cfg(target_os = "linux")]
use crate::capture::x11;

/// A live recording session: owns the ffmpeg subprocess, the video source
/// (if any runs in-process), audio streams, and the pacer thread.
pub struct Recording {
    ffmpeg: FfmpegJob,
    video: VideoSource,
    #[cfg(windows)]
    audio: Option<AudioStreams>,
    pacer: Option<PacerHandle>,
    stats: Arc<Stats>,
    pub out_path: PathBuf,
    pub started_at: Instant,
}

struct PacerHandle {
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<Result<()>>,
}

fn output_path(cfg: &Config) -> Result<PathBuf> {
    std::fs::create_dir_all(&cfg.output_dir)?;
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    const FORMAT: &[time::format_description::FormatItem<'_>] =
        time::macros::format_description!("[year][month][day]-[hour][minute][second]");
    let stamp = now.format(FORMAT).unwrap_or_else(|_| "recording".to_string());
    Ok(cfg.output_dir.join(format!("litecap-{stamp}.mp4")))
}

/// Builds the video filter chain prefix (scale, then any encoder-specific
/// pixel-format conversion) as a single `-vf`/`-filter:v` value, or `None`
/// if no filter is needed.
fn video_filter(cfg: &Config, enc: VideoEncoder) -> Option<String> {
    let scale = if cfg.preset_1080p60 {
        // Fit within 1920x1080 preserving aspect, then pad to exact size
        // so the output is always precisely 1920x1080 regardless of the
        // source monitor's native resolution/aspect.
        Some(
            "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2"
                .to_string(),
        )
    } else {
        cfg.max_height.map(|h| format!("scale=-2:{h}"))
    };
    match enc {
        VideoEncoder::Vaapi => {
            let mut parts = Vec::new();
            if let Some(s) = scale {
                parts.push(s);
            }
            parts.push("format=nv12,hwupload".to_string());
            Some(parts.join(","))
        }
        _ => scale,
    }
}

#[cfg(windows)]
pub fn start(cfg: &Config, ffmpeg: &Path, enc: VideoEncoder) -> Result<Recording> {
    let out_path = output_path(cfg)?;

    // 1. Start video capture and wait for the first frame so we know W/H.
    let slot = FrameSlot::new();
    let video = VideoSource::Windows(
        win::start(cfg.monitor_index, slot.clone()).context("starting screen capture")?,
    );
    let (width, height) = wait_first_frame(&slot, Duration::from_secs(3))?;

    // 2. Probe audio sources (get real device rate/channels) and bind
    //    listeners, before spawning ffmpeg, so the command line can carry
    //    the true `-ar`/`-ac` (ffmpeg resamples; we never do in-process).
    let sys_probe = if cfg.system_audio {
        match audio_win::probe_system_audio() {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("system audio unavailable: {e}");
                notify("System audio unavailable, recording without it");
                None
            }
        }
    } else {
        None
    };
    let mic_probe = if cfg.microphone {
        match audio_win::probe_microphone() {
            Ok(p) => Some(p),
            Err(e) => {
                log::warn!("microphone unavailable: {e}");
                notify("Microphone unavailable, recording without it");
                None
            }
        }
    } else {
        None
    };
    let sys_listener = sys_probe
        .as_ref()
        .map(|_| audio_net::listener().context("binding system-audio socket"))
        .transpose()?;
    let mic_listener = mic_probe
        .as_ref()
        .map(|_| audio_net::listener().context("binding microphone socket"))
        .transpose()?;

    // 3. Build and spawn ffmpeg.
    let args = build_ffmpeg_args_windows(
        cfg,
        enc,
        width,
        height,
        sys_listener.as_ref().map(|(_, p)| (*p, sys_probe.as_ref().unwrap().info)),
        mic_listener.as_ref().map(|(_, p)| (*p, mic_probe.as_ref().unwrap().info)),
        &out_path,
    );
    let mut job = FfmpegJob::spawn(ffmpeg, &args, true).context("spawning ffmpeg")?;
    let stdin = job.stdin().context("ffmpeg stdin was not piped")?;

    // 4. Start the pacer thread feeding ffmpeg's stdin immediately. ffmpeg
    //    opens its inputs in command-line order and won't reach the TCP
    //    audio inputs until the rawvideo stdin input has data flowing, so
    //    the pacer must run before we try to accept the audio sockets.
    let stats = Arc::new(Stats::default());
    let stop = Arc::new(AtomicBool::new(false));
    let pacer = {
        let slot = slot.clone();
        let stop = stop.clone();
        let stats = stats.clone();
        let fps = cfg.effective_fps();
        std::thread::Builder::new()
            .name("litecap-pacer".into())
            .spawn(move || crate::capture::run_pacer(slot, fps, stdin, stop, stats))
            .context("spawning pacer thread")?
    };

    // 5. Accept audio sockets and start cpal streams. A single source
    //    failing to open must not abort the recording.
    let mut audio = AudioStreams { sys: None, mic: None };
    if let (Some((listener, _)), Some(probe)) = (sys_listener, &sys_probe) {
        match audio_net::accept_with_timeout(&listener, Duration::from_secs(5))
            .and_then(|s| probe.open(s, "system"))
        {
            Ok(stream) => audio.sys = Some(stream),
            Err(e) => {
                log::warn!("system audio unavailable: {e}");
                notify("System audio unavailable, recording without it");
            }
        }
    }
    if let (Some((listener, _)), Some(probe)) = (mic_listener, &mic_probe) {
        match audio_net::accept_with_timeout(&listener, Duration::from_secs(5))
            .and_then(|s| probe.open(s, "mic"))
        {
            Ok(stream) => audio.mic = Some(stream),
            Err(e) => {
                log::warn!("microphone unavailable: {e}");
                notify("Microphone unavailable, recording without it");
            }
        }
    }

    Ok(Recording {
        ffmpeg: job,
        video,
        audio: Some(audio),
        pacer: Some(PacerHandle { stop, join: pacer }),
        stats,
        out_path,
        started_at: Instant::now(),
    })
}

#[cfg(windows)]
fn wait_first_frame(slot: &Arc<FrameSlot>, timeout: Duration) -> Result<(u32, u32)> {
    let start = Instant::now();
    loop {
        if let Some(frame) = slot.take() {
            let dims = (frame.width, frame.height);
            slot.recycle(frame.data);
            return Ok(dims);
        }
        if start.elapsed() >= timeout {
            bail!("timed out waiting for the first captured frame");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn build_ffmpeg_args_windows(
    cfg: &Config,
    enc: VideoEncoder,
    width: u32,
    height: u32,
    sys: Option<(u16, audio_win::StreamInfo)>,
    mic: Option<(u16, audio_win::StreamInfo)>,
    out_path: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "warning".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "bgra".into(),
        "-video_size".into(),
        format!("{width}x{height}"),
        "-framerate".into(),
        cfg.effective_fps().to_string(),
        "-i".into(),
        "pipe:0".into(),
    ];

    let mut audio_maps: Vec<String> = Vec::new();
    let mut input_index = 1;
    let mut sys_idx = None;
    let mut mic_idx = None;
    if let Some((port, info)) = sys {
        args.extend([
            "-f".into(),
            "f32le".into(),
            "-ar".into(),
            info.sample_rate.to_string(),
            "-ac".into(),
            info.channels.to_string(),
            "-i".into(),
            format!("tcp://127.0.0.1:{port}"),
        ]);
        sys_idx = Some(input_index);
        input_index += 1;
    }
    if let Some((port, info)) = mic {
        args.extend([
            "-f".into(),
            "f32le".into(),
            "-ar".into(),
            info.sample_rate.to_string(),
            "-ac".into(),
            info.channels.to_string(),
            "-i".into(),
            format!("tcp://127.0.0.1:{port}"),
        ]);
        mic_idx = Some(input_index);
    }

    match (sys_idx, mic_idx) {
        (Some(a), Some(b)) => {
            args.extend([
                "-filter_complex".into(),
                format!(
                    "[{a}:a]aresample=async=1[a1];[{b}:a]aresample=async=1[a2];[a1][a2]amix=inputs=2:duration=longest[aout]"
                ),
            ]);
            audio_maps.push("[aout]".into());
        }
        (Some(a), None) => {
            args.extend([
                "-filter_complex".into(),
                format!("[{a}:a]aresample=async=1[aout]"),
            ]);
            audio_maps.push("[aout]".into());
        }
        (None, Some(b)) => {
            args.extend([
                "-filter_complex".into(),
                format!("[{b}:a]aresample=async=1[aout]"),
            ]);
            audio_maps.push("[aout]".into());
        }
        (None, None) => {}
    }

    args.extend(["-map".into(), "0:v".into()]);
    for m in &audio_maps {
        args.extend(["-map".into(), m.clone()]);
    }

    if let Some(vf) = video_filter(cfg, enc) {
        args.extend(["-vf".into(), vf]);
    }
    args.extend(enc.probe_args().iter().map(|s| s.to_string()));
    args.extend(enc.quality_args(cfg.quality));

    if !audio_maps.is_empty() {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    }

    args.extend([
        "-g".into(),
        (2 * cfg.effective_fps()).to_string(),
        "-bf".into(),
        "0".into(),
        "-movflags".into(),
        "+frag_keyframe+empty_moov".into(),
        "-f".into(),
        "mp4".into(),
        out_path.to_string_lossy().into_owned(),
    ]);

    args
}

#[cfg(target_os = "linux")]
pub fn start(cfg: &Config, ffmpeg: &Path, enc: VideoEncoder) -> Result<Recording> {
    let out_path = output_path(cfg)?;
    let stats = Arc::new(Stats::default());

    if x11::is_wayland_session() {
        start_wayland(cfg, ffmpeg, enc, out_path, stats)
    } else {
        start_x11(cfg, ffmpeg, enc, out_path, stats)
    }
}

#[cfg(target_os = "linux")]
fn pulse_probe_ok(ffmpeg: &Path) -> bool {
    std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-f", "pulse", "-i", "default", "-t", "0.1", "-f", "null", "-"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn default_sink() -> Option<String> {
    let out = std::process::Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(target_os = "linux")]
fn linux_audio_inputs(cfg: &Config, ffmpeg: &Path, args: &mut Vec<String>) -> (bool, bool) {
    if !cfg.system_audio && !cfg.microphone {
        return (false, false);
    }
    if !pulse_probe_ok(ffmpeg) {
        log::warn!("ffmpeg pulse input unusable in this build; disabling audio");
        notify("Audio unavailable in this FFmpeg build (needs a pulse-enabled build)");
        return (false, false);
    }
    let mut sys_ok = false;
    let mut mic_ok = false;
    if cfg.system_audio {
        if let Some(sink) = default_sink() {
            args.extend(["-f".into(), "pulse".into(), "-i".into(), format!("{sink}.monitor")]);
            sys_ok = true;
        } else {
            log::warn!("no default pulse sink found; disabling system audio");
            notify("System audio unavailable, recording without it");
        }
    }
    if cfg.microphone {
        args.extend(["-f".into(), "pulse".into(), "-i".into(), "default".into()]);
        mic_ok = true;
    }
    (sys_ok, mic_ok)
}

#[cfg(target_os = "linux")]
fn append_audio_filter(args: &mut Vec<String>, video_input_count: usize, sys_ok: bool, mic_ok: bool) -> bool {
    let a = video_input_count; // first audio input index
    let b = a + 1;
    match (sys_ok, mic_ok) {
        (true, true) => {
            args.extend([
                "-filter_complex".into(),
                format!(
                    "[{a}:a]aresample=async=1[a1];[{b}:a]aresample=async=1[a2];[a1][a2]amix=inputs=2:duration=longest[aout]"
                ),
            ]);
            true
        }
        (true, false) | (false, true) => {
            args.extend(["-filter_complex".into(), format!("[{a}:a]aresample=async=1[aout]")]);
            true
        }
        (false, false) => false,
    }
}

#[cfg(target_os = "linux")]
fn start_x11(cfg: &Config, ffmpeg: &Path, enc: VideoEncoder, out_path: PathBuf, stats: Arc<Stats>) -> Result<Recording> {
    let monitors = x11::monitors();
    let mon = monitors.get(cfg.monitor_index);

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-v".into(), "warning".into()];
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0.0".into());
    args.extend(["-f".into(), "x11grab".into(), "-framerate".into(), cfg.effective_fps().to_string()]);
    if let Some(m) = mon {
        args.extend([
            "-video_size".into(),
            format!("{}x{}", m.w, m.h),
            "-i".into(),
            format!("{display}+{},{}", m.x, m.y),
        ]);
    } else {
        args.extend(["-i".into(), display]);
    }

    let (sys_ok, mic_ok) = linux_audio_inputs(cfg, ffmpeg, &mut args);
    let has_audio = append_audio_filter(&mut args, 1, sys_ok, mic_ok);

    args.extend(["-map".into(), "0:v".into()]);
    if has_audio {
        args.extend(["-map".into(), "[aout]".into()]);
    }
    if let Some(vf) = video_filter(cfg, enc) {
        args.extend(["-vf".into(), vf]);
    }
    args.extend(enc.probe_args().iter().map(|s| s.to_string()));
    args.extend(enc.quality_args(cfg.quality));
    if has_audio {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    }
    args.extend([
        "-g".into(),
        (2 * cfg.effective_fps()).to_string(),
        "-bf".into(),
        "0".into(),
        "-movflags".into(),
        "+frag_keyframe+empty_moov".into(),
        "-f".into(),
        "mp4".into(),
        out_path.to_string_lossy().into_owned(),
    ]);

    let job = FfmpegJob::spawn(ffmpeg, &args, false).context("spawning ffmpeg (x11grab)")?;

    Ok(Recording {
        ffmpeg: job,
        video: VideoSource::External,
        pacer: None,
        stats,
        out_path,
        started_at: Instant::now(),
    })
}

#[cfg(target_os = "linux")]
fn start_wayland(cfg: &Config, ffmpeg: &Path, enc: VideoEncoder, out_path: PathBuf, stats: Arc<Stats>) -> Result<Recording> {
    let slot = FrameSlot::new();
    let capture = wayland::start(slot.clone()).map_err(|e| match e {
        wayland::WaylandCaptureError::Cancelled => anyhow::anyhow!("capture cancelled by user"),
        wayland::WaylandCaptureError::Other(e) => e,
    })?;
    let (width, height) = wait_first_frame_linux(&slot, Duration::from_secs(10))?;

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "warning".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "bgra".into(),
        "-video_size".into(),
        format!("{width}x{height}"),
        "-framerate".into(),
        cfg.effective_fps().to_string(),
        "-i".into(),
        "pipe:0".into(),
    ];

    let (sys_ok, mic_ok) = linux_audio_inputs(cfg, ffmpeg, &mut args);
    let has_audio = append_audio_filter(&mut args, 1, sys_ok, mic_ok);

    args.extend(["-map".into(), "0:v".into()]);
    if has_audio {
        args.extend(["-map".into(), "[aout]".into()]);
    }
    if let Some(vf) = video_filter(cfg, enc) {
        args.extend(["-vf".into(), vf]);
    }
    args.extend(enc.probe_args().iter().map(|s| s.to_string()));
    args.extend(enc.quality_args(cfg.quality));
    if has_audio {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    }
    args.extend([
        "-g".into(),
        (2 * cfg.effective_fps()).to_string(),
        "-bf".into(),
        "0".into(),
        "-movflags".into(),
        "+frag_keyframe+empty_moov".into(),
        "-f".into(),
        "mp4".into(),
        out_path.to_string_lossy().into_owned(),
    ]);

    let mut job = FfmpegJob::spawn(ffmpeg, &args, true).context("spawning ffmpeg (wayland)")?;
    let stdin = job.stdin().context("ffmpeg stdin was not piped")?;

    let stop = Arc::new(AtomicBool::new(false));
    let pacer = {
        let slot = slot.clone();
        let stop = stop.clone();
        let stats = stats.clone();
        let fps = cfg.effective_fps();
        std::thread::Builder::new()
            .name("litecap-pacer".into())
            .spawn(move || crate::capture::run_pacer(slot, fps, stdin, stop, stats))
            .context("spawning pacer thread")?
    };

    Ok(Recording {
        ffmpeg: job,
        video: VideoSource::Wayland(capture),
        pacer: Some(PacerHandle { stop, join: pacer }),
        stats,
        out_path,
        started_at: Instant::now(),
    })
}

#[cfg(target_os = "linux")]
fn wait_first_frame_linux(slot: &Arc<FrameSlot>, timeout: Duration) -> Result<(u32, u32)> {
    let start = Instant::now();
    loop {
        if let Some(frame) = slot.take() {
            let dims = (frame.width, frame.height);
            slot.recycle(frame.data);
            return Ok(dims);
        }
        if start.elapsed() >= timeout {
            bail!("timed out waiting for the first captured frame");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Recording {
    pub fn stats(&self) -> (u64, u64) {
        (
            self.stats.frames_sent.load(Ordering::Relaxed),
            self.stats.stale_repeats.load(Ordering::Relaxed),
        )
    }

    pub fn stop(mut self) -> Result<PathBuf> {
        let (frames_sent, stale_repeats) = self.stats();
        log::info!("pacer stats: {frames_sent} frames sent, {stale_repeats} stale repeats");
        if let Some(p) = self.pacer.take() {
            p.stop.store(true, Ordering::Relaxed);
            if let Ok(Err(e)) = p.join.join() {
                log::warn!("pacer thread reported: {e}");
            }
        }

        match self.video {
            #[cfg(windows)]
            VideoSource::Windows(w) => {
                if let Err(e) = w.stop() {
                    log::warn!("failed to stop windows capture cleanly: {e}");
                }
            }
            #[cfg(target_os = "linux")]
            VideoSource::Wayland(w) => w.stop(),
            VideoSource::External => {}
        }

        #[cfg(windows)]
        drop(self.audio.take());

        let ok = self.ffmpeg.stop(Duration::from_secs(10))?;
        if !ok {
            log::warn!("ffmpeg exited non-zero or was killed; see litecap.log");
        }

        Ok(self.out_path)
    }
}

fn notify(body: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .summary("LiteCap")
        .body(body)
        .show()
    {
        log::warn!("failed to show notification: {e}");
    }
}
