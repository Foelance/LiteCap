use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use parking_lot::Mutex;

/// Actual negotiated sample rate/channels for a probed device, so the
/// caller can tell ffmpeg's `-ar`/`-ac` the truth before ever spawning it
/// (we never resample in-process).
#[derive(Debug, Clone, Copy)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A device plus its negotiated config, probed before ffmpeg is spawned so
/// the command line can carry the real `-ar`/`-ac`.
pub struct ProbedSource {
    device: Device,
    config: StreamConfig,
    pub info: StreamInfo,
}

/// Holds the cpal streams alive; dropping stops them.
pub struct AudioStreams {
    pub sys: Option<cpal::Stream>,
    pub mic: Option<cpal::Stream>,
}

/// A socket slot the audio callback writes into once available. The cpal
/// stream starts capturing (and keeps the WASAPI engine warmed up)
/// immediately, before ffmpeg has even connected; bytes captured before the
/// socket is attached are simply dropped rather than delaying capture
/// startup until after the TCP handshake completes.
pub type SinkSlot = Arc<Mutex<Option<TcpStream>>>;

fn f32_stream(device: &Device, config: &StreamConfig, sink_slot: SinkSlot, label: &'static str) -> Result<cpal::Stream> {
    let err_label = label;
    let stream = device.build_input_stream(
        config.clone(),
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            let mut guard = sink_slot.lock();
            let Some(sink) = guard.as_mut() else {
                return; // socket not attached yet; drop this packet
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), std::mem::size_of_val(data))
            };
            // Never block the audio callback: drop the packet on backpressure.
            if let Err(e) = sink.write_all(bytes) {
                if e.kind() != std::io::ErrorKind::WouldBlock {
                    log::debug!("{label} audio socket write failed: {e}");
                }
            }
        },
        move |e| log::warn!("{err_label} audio stream error: {e}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

/// Probes the WASAPI loopback source (default *output* device; cpal
/// transparently enables loopback for an input stream on an output device
/// on the WASAPI backend) without opening the stream yet.
pub fn probe_system_audio() -> Result<ProbedSource> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default output device for loopback capture")?;
    let supported = device
        .default_output_config()
        .context("no default output config")?;
    anyhow::ensure!(
        supported.sample_format() == SampleFormat::F32,
        "unsupported loopback sample format {:?}",
        supported.sample_format()
    );
    Ok(ProbedSource {
        info: StreamInfo {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        },
        config: supported.config(),
        device,
    })
}

/// Probes the default microphone input device without opening the stream.
pub fn probe_microphone() -> Result<ProbedSource> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default input (microphone) device")?;
    let supported = device
        .default_input_config()
        .context("no default input config")?;
    anyhow::ensure!(
        supported.sample_format() == SampleFormat::F32,
        "unsupported mic sample format {:?}",
        supported.sample_format()
    );
    Ok(ProbedSource {
        info: StreamInfo {
            sample_rate: supported.sample_rate(),
            channels: supported.channels(),
        },
        config: supported.config(),
        device,
    })
}

impl ProbedSource {
    /// Starts capturing immediately (before ffmpeg has even connected),
    /// writing into a shared slot. Returns the running stream plus the slot;
    /// call `attach` on the slot once the TCP socket is accepted. Starting
    /// capture immediately avoids a startup race where the WASAPI/loopback
    /// engine is opened fresh mid-playback after an unpredictable delay
    /// (accepting the socket can take up to several seconds), which was
    /// observed to sometimes yield an audio-less recording.
    pub fn start(&self, label: &'static str) -> Result<(cpal::Stream, SinkSlot)> {
        let slot: SinkSlot = Arc::new(Mutex::new(None));
        let stream = f32_stream(&self.device, &self.config, slot.clone(), label)?;
        Ok((stream, slot))
    }
}

/// Attaches the accepted TCP socket to a running capture stream's sink slot.
pub fn attach(slot: &SinkSlot, sink: TcpStream) -> Result<()> {
    sink.set_nonblocking(true)?;
    *slot.lock() = Some(sink);
    Ok(())
}
