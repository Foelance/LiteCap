use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::config::data_dir;

#[cfg(windows)]
const FFMPEG_BIN: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const FFMPEG_BIN: &str = "ffmpeg";

#[cfg(windows)]
const DOWNLOAD_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";
#[cfg(not(windows))]
const DOWNLOAD_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-linux64-gpl.tar.xz";

/// `<data_dir>/ffmpeg/ffmpeg(.exe)`
pub fn bundled_path() -> PathBuf {
    data_dir().join("ffmpeg").join(FFMPEG_BIN)
}

/// Check, in order: bundled path, then `ffmpeg` on PATH.
pub fn locate() -> Option<PathBuf> {
    let bundled = bundled_path();
    if bundled.is_file() {
        return Some(bundled);
    }
    let on_path = PathBuf::from(FFMPEG_BIN);
    if Command::new(&on_path)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Some(on_path);
    }
    None
}

/// Download ffmpeg into `<data_dir>/ffmpeg/`, streaming to a temp file (never
/// buffering the whole archive in RAM) and extracting only the ffmpeg binary.
pub fn download() -> Result<PathBuf> {
    let dest_dir = data_dir().join("ffmpeg");
    std::fs::create_dir_all(&dest_dir)?;

    let tmp_archive = dest_dir.join("download.tmp");
    stream_download(DOWNLOAD_URL, &tmp_archive)
        .with_context(|| format!("downloading ffmpeg from {DOWNLOAD_URL}"))?;

    let final_path = dest_dir.join(FFMPEG_BIN);
    extract_ffmpeg(&tmp_archive, &final_path)?;
    let _ = std::fs::remove_file(&tmp_archive);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&final_path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&final_path, perm)?;
    }

    Ok(final_path)
}

fn stream_download(url: &str, dest: &Path) -> Result<()> {
    let mut resp = ureq::get(url).call().context("http request failed")?;
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(())
}

#[cfg(windows)]
fn extract_ffmpeg(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        if name.ends_with("bin/ffmpeg.exe") {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    bail!("ffmpeg.exe not found inside downloaded archive")
}

#[cfg(not(windows))]
fn extract_ffmpeg(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let decompressed = xz2::read::XzDecoder::new(file);
    let mut tar = tar::Archive::new(decompressed);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path
            .to_str()
            .map(|s| s.ends_with("bin/ffmpeg"))
            .unwrap_or(false)
        {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    bail!("ffmpeg binary not found inside downloaded archive")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    Nvenc,
    Qsv,
    Amf,
    Vaapi,
    X264,
}

impl VideoEncoder {
    /// Encoder args (excluding quality, which callers append via `quality_args`).
    pub fn probe_args(self) -> &'static [&'static str] {
        match self {
            VideoEncoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4"],
            VideoEncoder::Qsv => &["-c:v", "h264_qsv"],
            VideoEncoder::Amf => &["-c:v", "h264_amf", "-rc", "cqp"],
            VideoEncoder::Vaapi => &[
                "-vaapi_device",
                "/dev/dri/renderD128",
                "-vf",
                "format=nv12,hwupload",
                "-c:v",
                "h264_vaapi",
            ],
            VideoEncoder::X264 => &["-c:v", "libx264", "-preset", "veryfast"],
        }
    }

    pub fn quality_args(self, q: u8) -> Vec<String> {
        let q = q.to_string();
        match self {
            VideoEncoder::Nvenc => vec!["-cq".into(), q],
            VideoEncoder::Qsv => vec!["-global_quality".into(), q],
            VideoEncoder::Amf => vec!["-qp_i".into(), q.clone(), "-qp_p".into(), q],
            VideoEncoder::Vaapi => vec!["-qp".into(), q],
            VideoEncoder::X264 => vec!["-crf".into(), q],
        }
    }

    #[cfg(windows)]
    fn candidates() -> &'static [VideoEncoder] {
        &[
            VideoEncoder::Nvenc,
            VideoEncoder::Qsv,
            VideoEncoder::Amf,
            VideoEncoder::X264,
        ]
    }

    #[cfg(not(windows))]
    fn candidates() -> &'static [VideoEncoder] {
        &[VideoEncoder::Vaapi, VideoEncoder::X264]
    }
}

/// Probe encoder support by running a real tiny encode. First exit-0 wins.
/// Not cached to disk: caller keeps the result in memory for the process
/// lifetime; re-probe each launch since hardware can change.
pub fn probe_encoder(ffmpeg: &Path) -> VideoEncoder {
    for &cand in VideoEncoder::candidates() {
        let mut args: Vec<&str> = vec![
            "-hide_banner",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=256x256:r=30:d=0.2",
            "-frames:v",
            "3",
        ];
        args.extend_from_slice(cand.probe_args());
        args.extend_from_slice(&["-f", "null", "-"]);
        let ok = Command::new(ffmpeg)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        log::debug!("encoder probe {cand:?}: {}", if ok { "ok" } else { "failed" });
        if ok {
            return cand;
        }
    }
    log::warn!("no hardware encoder available, falling back to libx264");
    VideoEncoder::X264
}

/// A running ffmpeg subprocess with piped stdin (rawvideo path) or no stdin
/// (x11grab path, where ffmpeg reads the screen itself).
pub struct FfmpegJob {
    child: Child,
}

impl FfmpegJob {
    pub fn spawn(ffmpeg: &Path, args: &[String], want_stdin: bool) -> Result<Self> {
        let log_path = data_dir().join("litecap.log");
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .context("opening litecap.log")?;

        let mut cmd = Command::new(ffmpeg);
        cmd.args(args);
        if want_stdin {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::from(log_file));
        let child = cmd.spawn().context("spawning ffmpeg")?;
        Ok(Self { child })
    }

    pub fn stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }

    /// Wait for graceful exit up to `timeout`; kill if it doesn't.
    pub fn stop(mut self, timeout: Duration) -> Result<bool> {
        let start = Instant::now();
        loop {
            match self.child.try_wait()? {
                Some(status) => return Ok(status.success()),
                None => {
                    if start.elapsed() >= timeout {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return Ok(false);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

