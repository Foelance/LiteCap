use std::sync::Arc;

use anyhow::{Context as _, Result};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as WcFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::FrameSlot;

type Flags = Arc<FrameSlot>;
type CaptureError = Box<dyn std::error::Error + Send + Sync>;

/// Handles WGC frame-arrived callbacks: converts each frame to tightly
/// packed BGRA and publishes it into the shared `FrameSlot`.
struct Handler {
    slot: Arc<FrameSlot>,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = CaptureError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            slot: ctx.flags,
            scratch: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut WcFrame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let width = frame.width();
        let height = frame.height();
        let buffer = frame.buffer()?;
        let bytes = buffer.as_nopadding_buffer(&mut self.scratch);
        self.slot.publish(width, height, bytes);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        log::info!("windows capture session closed");
        Ok(())
    }
}

pub struct WinCapture {
    control: CaptureControl<Handler, CaptureError>,
}

impl WinCapture {
    pub fn stop(self) -> Result<()> {
        self.control
            .stop()
            .map_err(|e| anyhow::anyhow!("failed to stop capture: {e}"))
    }
}

/// Picks the monitor by 0-based `monitor_index`, falling back to primary if
/// out of range, and starts free-threaded WGC capture publishing BGRA
/// frames into `slot`.
pub fn start(monitor_index: usize, slot: Arc<FrameSlot>) -> Result<WinCapture> {
    let monitor = Monitor::from_index(monitor_index + 1).or_else(|e| {
        log::warn!("monitor index {monitor_index} unavailable ({e}), falling back to primary");
        Monitor::primary()
    })
    .context("no capturable monitor found")?;

    let settings: Settings<Flags, Monitor> = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        slot,
    );

    let control = Handler::start_free_threaded(settings)
        .map_err(|e| anyhow::anyhow!("failed to start capture: {e}"))?;
    Ok(WinCapture { control })
}

pub fn monitor_names() -> Vec<String> {
    Monitor::enumerate()
        .map(|monitors| {
            monitors
                .iter()
                .enumerate()
                .map(|(i, m)| m.name().unwrap_or_else(|_| format!("Monitor {}", i + 1)))
                .collect()
        })
        .unwrap_or_default()
}
