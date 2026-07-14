use std::io::Write;
use std::net::TcpStream;

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};

/// Actual negotiated sample rate/channels for a probed device, so the
/// caller can tell ffmpeg's `-ar`/`-ac` the truth before ever spawning it
/// (we never resample in-process).
#[derive(Debug, Clone, Copy)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A device plus its negotiated config, probed before ffmpeg is spawned so
/// the command line can carry the real `-ar`/`-ac`. `open` is deferred until
/// after ffmpeg has connected back to our TCP listener.
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

fn f32_stream(
    device: &Device,
    config: &StreamConfig,
    mut sink: TcpStream,
    label: &'static str,
) -> Result<cpal::Stream> {
    sink.set_nonblocking(true)?;
    let err_label = label;
    let stream = device.build_input_stream(
        config.clone(),
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
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
    /// Opens the stream against an already-connected ffmpeg socket, using
    /// the exact config that was probed (and therefore already told to
    /// ffmpeg via `-ar`/`-ac`).
    pub fn open(&self, sink: TcpStream, label: &'static str) -> Result<cpal::Stream> {
        f32_stream(&self.device, &self.config, sink, label)
    }
}
