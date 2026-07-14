use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context as _, Result};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;

use super::FrameSlot;

/// Distinct error so the recorder can show "capture cancelled" instead of a
/// hard failure when the user dismisses the portal's monitor-picker dialog.
#[derive(Debug, thiserror::Error)]
pub enum WaylandCaptureError {
    #[error("user cancelled the screen selection dialog")]
    Cancelled,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

enum LoopMsg {
    Stop,
}

pub struct WaylandCapture {
    thread: JoinHandle<()>,
    sender: pw::channel::Sender<LoopMsg>,
}

impl WaylandCapture {
    pub fn stop(self) {
        let _ = self.sender.send(LoopMsg::Stop);
        let _ = self.thread.join();
    }
}

/// Runs the XDG portal negotiation (async) to get a PipeWire node id + fd,
/// then drives the PipeWire loop (sync) on this same dedicated thread.
pub fn start(slot: Arc<FrameSlot>) -> Result<WaylandCapture, WaylandCaptureError> {
    let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<pw::channel::Sender<LoopMsg>, WaylandCaptureError>>();

    let thread = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = init_tx.send(Err(WaylandCaptureError::Other(e.into())));
                return;
            }
        };
        let negotiated = rt.block_on(negotiate_portal());
        let (node_id, fd) = match negotiated {
            Ok(v) => v,
            Err(e) => {
                let _ = init_tx.send(Err(e));
                return;
            }
        };
        // Drop the tokio runtime before blocking on the pipewire loop.
        drop(rt);

        if let Err(e) = run_pipewire_loop(node_id, fd, slot, init_tx) {
            log::error!("pipewire capture loop failed: {e}");
        }
    });

    match init_rx.recv() {
        Ok(Ok(sender)) => Ok(WaylandCapture { thread, sender }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(WaylandCaptureError::Other(anyhow::anyhow!(
                "wayland capture thread exited before initializing"
            )))
        }
    }
}

async fn negotiate_portal() -> Result<(u32, std::os::fd::OwnedFd), WaylandCaptureError> {
    let proxy = Screencast::new()
        .await
        .context("connecting to screencast portal")?;
    let session = proxy
        .create_session(Default::default())
        .await
        .context("creating screencast session")?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor)
                .set_multiple(false)
                .set_persist_mode(PersistMode::Application),
        )
        .await
        .context("selecting screencast sources")?;

    let response = proxy
        .start(&session, None, Default::default())
        .await
        .map_err(|e| {
            // `ashpd::desktop::request::ResponseError` is a private type (the
            // `request` module is `pub(crate)`), so it can't be named/matched
            // here. Distinguish "user cancelled" via ashpd's stable Display
            // text for `Error::Response(ResponseError::Cancelled)` instead.
            if matches!(e, ashpd::Error::Response(_)) && e.to_string() == "Portal request was cancelled" {
                WaylandCaptureError::Cancelled
            } else {
                WaylandCaptureError::Other(anyhow::anyhow!("starting screencast: {e}"))
            }
        })?
        .response()
        .context("reading screencast response")?;

    let stream = response
        .streams()
        .first()
        .cloned()
        .context("portal returned no streams")?;
    let node_id = stream.pipe_wire_node_id();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .context("opening pipewire remote")?;

    Ok((node_id, fd))
}

struct StreamData {
    slot: Arc<FrameSlot>,
    width: u32,
    height: u32,
    format: spa::param::video::VideoFormat,
    scratch: Vec<u8>,
}

fn run_pipewire_loop(
    node_id: u32,
    fd: std::os::fd::OwnedFd,
    slot: Arc<FrameSlot>,
    init_tx: std::sync::mpsc::Sender<Result<pw::channel::Sender<LoopMsg>, WaylandCaptureError>>,
) -> Result<()> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None).context("creating pipewire main loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("creating pipewire context")?;
    let core = context
        .connect_fd_rc(fd, None)
        .context("connecting pipewire to portal fd")?;

    let (sender, receiver) = pw::channel::channel::<LoopMsg>();
    let ml_for_recv = mainloop.clone();
    let _recv_guard = receiver.attach(mainloop.loop_(), move |msg| match msg {
        LoopMsg::Stop => ml_for_recv.quit(),
    });

    let data = StreamData {
        slot,
        width: 0,
        height: 0,
        format: spa::param::video::VideoFormat::BGRx,
        scratch: Vec::new(),
    };

    let stream = pw::stream::StreamRc::new(
        core,
        "litecap-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .context("creating pipewire stream")?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_, user_data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param) else {
                return;
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            let mut info = spa::param::video::VideoInfoRaw::default();
            if info.parse(param).is_err() {
                return;
            }
            user_data.width = info.size().width;
            user_data.height = info.size().height;
            user_data.format = info.format();
            log::info!(
                "wayland capture negotiated {}x{} format={:?}",
                user_data.width,
                user_data.height,
                user_data.format
            );
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() || user_data.width == 0 {
                return;
            }
            let stride = datas[0].chunk().stride() as usize;
            let Some(bytes) = datas[0].data() else { return };
            let force_opaque = matches!(user_data.format, spa::param::video::VideoFormat::BGRx);
            super::copy_rows(
                bytes,
                stride,
                user_data.width,
                user_data.height,
                force_opaque,
                &mut user_data.scratch,
            );
            user_data.slot.publish(user_data.width, user_data.height, &user_data.scratch);
        })
        .register()
        .context("registering pipewire stream listener")?;

    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA,
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 1920, height: 1080 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
    );
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .context("serializing pipewire format pod")?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).context("parsing serialized pod")?];

    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("connecting pipewire stream to node")?;

    // Signal success to the caller now that the loop is about to run.
    if init_tx.send(Ok(sender)).is_err() {
        // Caller gave up waiting; nothing to run for.
        return Ok(());
    }

    mainloop.run();
    Ok(())
}
