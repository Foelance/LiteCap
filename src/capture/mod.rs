#[cfg(windows)]
pub mod win;

#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(target_os = "linux")]
pub mod x11;

use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;

/// A single tightly-packed BGRA frame (stride removed at copy time).
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// At most 2 raw frame buffers alive at once: a "latest frame" slot plus one
/// in-flight write, reused via a small pool. No unbounded queues.
pub struct FrameSlot {
    latest: Mutex<Option<Frame>>,
    pool_tx: Sender<Vec<u8>>,
    pool_rx: Receiver<Vec<u8>>,
}

impl FrameSlot {
    pub fn new() -> Arc<Self> {
        let (pool_tx, pool_rx) = crossbeam_channel::bounded(2);
        Arc::new(Self {
            latest: Mutex::new(None),
            pool_tx,
            pool_rx,
        })
    }

    /// Producer side (capture callback). Copies `src` (already tightly
    /// packed BGRA) into a pooled buffer and publishes it as the latest
    /// frame. Never allocates after warmup: reuses a pooled buffer, or the
    /// buffer being replaced, resizing only if dimensions changed.
    pub fn publish(&self, width: u32, height: u32, src: &[u8]) {
        let mut buf = self.pool_rx.try_recv().unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(src);
        let mut guard = self.latest.lock();
        if let Some(old) = guard.replace(Frame {
            width,
            height,
            data: buf,
        }) {
            // Return the previous buffer to the pool for reuse.
            let _ = self.pool_tx.try_send(old.data);
        }
    }

    /// Consumer side (pacer thread). Takes the latest frame, leaving the
    /// slot empty; caller must return the buffer via `recycle` after use.
    pub fn take(&self) -> Option<Frame> {
        self.latest.lock().take()
    }

    pub fn recycle(&self, buf: Vec<u8>) {
        let _ = self.pool_tx.try_send(buf);
    }
}

/// Frame-send / stale-repeat counters, updated from the pacer thread.
#[derive(Default)]
pub struct Stats {
    pub frames_sent: AtomicU64,
    pub stale_repeats: AtomicU64,
}

/// Row-copy helper shared by capture backends that must convert a
/// possibly-strided, possibly-BGRx source buffer into tightly packed BGRA.
/// `force_opaque` sets alpha=0xFF during the copy (for BGRx sources).
pub fn copy_rows(src: &[u8], stride: usize, width: u32, height: u32, force_opaque: bool, dst: &mut Vec<u8>) {
    let width = width as usize;
    let height = height as usize;
    dst.clear();
    dst.reserve(width * height * 4);
    for row in 0..height {
        let start = row * stride;
        let row_bytes = &src[start..start + width * 4];
        if force_opaque {
            for px in row_bytes.chunks_exact(4) {
                dst.extend_from_slice(&[px[0], px[1], px[2], 0xFF]);
            }
        } else {
            dst.extend_from_slice(row_bytes);
        }
    }
}

/// Sample-and-hold pacer: ticks at `fps`, writes the latest frame (or
/// rewrites the previous one if nothing new arrived) to ffmpeg's stdin.
/// Produces exact CFR output; never lets encoder lag grow a queue.
pub fn run_pacer(
    slot: Arc<FrameSlot>,
    fps: u32,
    mut sink: ChildStdin,
    stop: Arc<AtomicBool>,
    stats: Arc<Stats>,
) -> anyhow::Result<()> {
    let period = Duration::from_nanos(1_000_000_000 / fps as u64);
    let mut next = Instant::now() + period;
    let mut held: Option<Frame> = None;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += period;

        let fresh = slot.take();
        let is_fresh = fresh.is_some();
        let frame = match fresh {
            Some(f) => {
                if let Some(prev) = held.take() {
                    slot.recycle(prev.data);
                }
                f
            }
            None => match held.take() {
                Some(f) => f,
                None => continue, // no frame captured yet
            },
        };

        if let Err(e) = sink.write_all(&frame.data) {
            stop.store(true, Ordering::Relaxed);
            return Err(anyhow::anyhow!("ffmpeg stdin write failed: {e}"));
        }

        stats.frames_sent.fetch_add(1, Ordering::Relaxed);
        if !is_fresh {
            stats.stale_repeats.fetch_add(1, Ordering::Relaxed);
        }
        held = Some(frame);
    }
    Ok(())
}

/// Dispatches to the platform-specific video source. X11 has no in-process
/// variant (ffmpeg's x11grab reads the screen itself).
pub enum VideoSource {
    #[cfg(windows)]
    Windows(win::WinCapture),
    #[cfg(target_os = "linux")]
    Wayland(wayland::WaylandCapture),
    /// No in-process capture; ffmpeg reads the screen directly.
    External,
}
