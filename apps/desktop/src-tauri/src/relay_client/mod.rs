use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use proto::remote_desktop::{
    remote_desktop_client::RemoteDesktopClient, ClientJoin, ClientMessage, FrameData, HostAnnounce,
    HostMessage, InputEvent, RegisterRequest, TileUpdate,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Channel, ClientTlsConfig};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Tile size in pixels.  128×128 is a good balance: small enough to detect
/// localised changes, large enough for JPEG DCT to work efficiently.
const TILE_SIZE: u32 = 128;

/// Send a full keyframe every N delta frames to let new clients sync, and to
/// recover from any accumulated error.  At 20 fps this is every ~6 seconds.
const KEYFRAME_INTERVAL: u32 = 120;

/// JPEG quality for keyframes.
const QUALITY_KEY: u8 = 75;

/// JPEG quality for dirty tiles.
const QUALITY_TILE: u8 = 65;

/// Capture interval (target ~20 fps).
const CAPTURE_MS: u64 = 50;

/// After this many consecutive idle captures (no dirty tiles), back off to a
/// slower poll rate to avoid burning CPU on static screens.
const IDLE_BACKOFF_AFTER: u32 = 6;   // 300 ms at 20 fps
const CAPTURE_MS_IDLE: u64 = 150;    // ~6 fps idle poll

// ─────────────────────────────────────────────────────────────────────────────
// gRPC channel factory
// ─────────────────────────────────────────────────────────────────────────────

pub async fn connect(server_url: &str) -> Result<RemoteDesktopClient<Channel>> {
    let mut builder = tonic::transport::Channel::from_shared(server_url.to_owned())?
        .connect_timeout(Duration::from_secs(10))
        // Send HTTP/2 PING every 20s to keep the connection alive through
        // Traefik / load-balancer idle timeouts (typically 60s).
        .http2_keep_alive_interval(Duration::from_secs(20))
        // If the peer doesn't respond to PING within 10s, close the connection.
        .keep_alive_timeout(Duration::from_secs(10))
        // Send keepalive even when there are no active streams (host idle mode).
        .keep_alive_while_idle(true);
    // For HTTPS endpoints, configure TLS with native system roots
    if server_url.starts_with("https://") {
        let tls = ClientTlsConfig::new().with_native_roots();
        builder = builder.tls_config(tls)?;
    }
    let channel = builder.connect().await?;
    Ok(RemoteDesktopClient::new(channel)
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024))
}

// ─────────────────────────────────────────────────────────────────────────────
// Device registration
// ─────────────────────────────────────────────────────────────────────────────

pub async fn register_device(server_url: &str, device_id: &str) -> Result<String> {
    let mut client = connect(server_url).await?;
    let resp = client
        .register_device(RegisterRequest {
            device_id: device_id.to_owned(),
        })
        .await?;
    Ok(resp.into_inner().device_id)
}

// ─────────────────────────────────────────────────────────────────────────────
// JPEG encoding helpers (libjpeg-turbo via turbojpeg crate)
// ─────────────────────────────────────────────────────────────────────────────

/// Encode a full RGBA image as JPEG using the provided TurboJPEG compressor.
///
/// `Subsamp::None` (4:4:4) is used so that colored text and sharp UI edges
/// are rendered faithfully — important for remote desktop content.
fn encode_jpeg(
    comp: &mut turbojpeg::Compressor,
    img: &image::RgbaImage,
    quality: u8,
) -> Result<Vec<u8>> {
    comp.set_quality(quality as i32);
    comp.set_subsamp(turbojpeg::Subsamp::Sub2x2); // 4:2:0 — standard JPEG chroma subsampling
    let image = turbojpeg::Image {
        pixels: img.as_raw().as_slice(),
        width:  img.width()  as usize,
        pitch:  img.width()  as usize * 4,
        height: img.height() as usize,
        format: turbojpeg::PixelFormat::RGBA,
    };
    Ok(comp.compress_to_vec(image)?)
}

/// Encode a rectangular sub-region of an RGBA image as JPEG.
///
/// Uses turbojpeg's `pitch` parameter to address the sub-region directly in
/// the source buffer — **no pixel copy required**.
fn encode_tile(
    comp: &mut turbojpeg::Compressor,
    img: &image::RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    quality: u8,
) -> Result<Vec<u8>> {
    debug_assert!(x + w <= img.width(),  "tile x+w out of bounds");
    debug_assert!(y + h <= img.height(), "tile y+h out of bounds");

    comp.set_quality(quality as i32);
    comp.set_subsamp(turbojpeg::Subsamp::Sub2x2); // 4:2:0 — 30-50% smaller than 4:4:4

    let pitch  = img.width() as usize * 4;
    let offset = y as usize * pitch + x as usize * 4; // byte offset of tile's top-left pixel
    let image = turbojpeg::Image {
        pixels: &img.as_raw()[offset..],
        width:  w as usize,
        pitch,           // turbojpeg uses this to skip to the next row — no copy needed
        height: h as usize,
        format: turbojpeg::PixelFormat::RGBA,
    };
    Ok(comp.compress_to_vec(image)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Dirty-rectangle state
// ─────────────────────────────────────────────────────────────────────────────

/// Per-tile FNV-1a hash of the last sent frame, used to detect changes.
struct CaptureState {
    /// FNV-1a hashes for each tile in row-major order.
    tile_hashes: Vec<u64>,
    /// Current screen dimensions (used to detect resolution changes).
    screen_w: u32,
    screen_h: u32,
    /// Running counter; triggers a periodic keyframe.
    delta_count: u32,
    /// Consecutive captures that produced no dirty tiles (idle detection).
    idle_count: u32,
    /// When set, the next frame is forced to be a full keyframe regardless of
    /// tile hashes.  Used to sync a newly-joined client immediately.
    force_keyframe: Arc<std::sync::atomic::AtomicBool>,
    /// Reused libjpeg-turbo compressor — avoids per-frame allocation.
    compressor: turbojpeg::Compressor,
}

impl CaptureState {
    fn new(force_keyframe: Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            tile_hashes: Vec::new(),
            screen_w: 0,
            screen_h: 0,
            delta_count: 0,
            idle_count: 0,
            force_keyframe,
            compressor: turbojpeg::Compressor::new()
                .expect("failed to initialise TurboJPEG compressor"),
        }
    }

    /// True when a full keyframe must be sent regardless of tile hashes.
    fn needs_keyframe(&self, img: &image::RgbaImage) -> bool {
        self.tile_hashes.is_empty()
            || img.width() != self.screen_w
            || img.height() != self.screen_h
            || self.delta_count % KEYFRAME_INTERVAL == 0
            // Consume the flag atomically; true means a client just joined.
            || self.force_keyframe.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Sleep duration for this capture cycle: fast when active, slow when idle.
    fn sleep_interval(&self) -> Duration {
        if self.idle_count >= IDLE_BACKOFF_AFTER {
            Duration::from_millis(CAPTURE_MS_IDLE)
        } else {
            Duration::from_millis(CAPTURE_MS)
        }
    }
}

/// Fast FNV-1a hash of a single tile's raw bytes.
fn hash_tile(raw: &[u8], img_width: u32, x: u32, y: u32, w: u32, h: u32) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for dy in 0..h {
        let row_start = ((y + dy) * img_width + x) as usize * 4;
        for &byte in &raw[row_start..row_start + w as usize * 4] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

/// Compute tiles_x, tiles_y for a given screen resolution.
fn tile_grid(w: u32, h: u32) -> (u32, u32) {
    (
        w.div_ceil(TILE_SIZE),
        h.div_ceil(TILE_SIZE),
    )
}

/// Main delta encoding function.
///
/// Returns `None` when the screen has not changed (no frame to send).
/// Returns `Some(FrameData)` with either a full JPEG (keyframe) or a list of
/// dirty tiles (delta frame).
fn compute_frame(state: &mut CaptureState, img: &image::RgbaImage) -> Option<FrameData> {
    let w = img.width();
    let h = img.height();
    let raw = img.as_raw();
    let (tiles_x, tiles_y) = tile_grid(w, h);
    let total = (tiles_x * tiles_y) as usize;

    if state.needs_keyframe(img) {
        // Full keyframe
        state.screen_w = w;
        state.screen_h = h;
        state.delta_count = 1;
        state.tile_hashes = (0..tiles_y)
            .flat_map(|ty| {
                (0..tiles_x).map(move |tx| {
                    let x = tx * TILE_SIZE;
                    let y = ty * TILE_SIZE;
                    let tw = TILE_SIZE.min(w - x);
                    let th = TILE_SIZE.min(h - y);
                    hash_tile(raw, w, x, y, tw, th)
                })
            })
            .collect();

        let jpeg = encode_jpeg(&mut state.compressor, img, QUALITY_KEY).unwrap_or_default();
        return Some(FrameData {
            width: w,
            height: h,
            is_keyframe: true,
            jpeg_data: jpeg,
            tiles: vec![],
        });
    }

    // Delta frame: compare each tile
    state.delta_count += 1;

    if state.tile_hashes.len() != total {
        state.tile_hashes.resize(total, 0);
    }

    let mut dirty: Vec<TileUpdate> = Vec::new();
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let x = tx * TILE_SIZE;
            let y = ty * TILE_SIZE;
            let tw = TILE_SIZE.min(w - x);
            let th = TILE_SIZE.min(h - y);
            let idx = (ty * tiles_x + tx) as usize;
            let new_hash = hash_tile(raw, w, x, y, tw, th);
            if new_hash != state.tile_hashes[idx] {
                state.tile_hashes[idx] = new_hash;
                if let Ok(jpeg) = encode_tile(&mut state.compressor, img, x, y, tw, th, QUALITY_TILE) {
                    dirty.push(TileUpdate {
                        x,
                        y,
                        tile_width: tw,
                        tile_height: th,
                        jpeg_data: jpeg,
                    });
                }
            }
        }
    }

    if dirty.is_empty() {
        state.idle_count = state.idle_count.saturating_add(1);
        return None; // screen unchanged — skip this frame entirely
    }

    state.idle_count = 0; // reset idle counter on activity

    Some(FrameData {
        width: w,
        height: h,
        is_keyframe: false,
        jpeg_data: vec![],
        tiles: dirty,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// HOST session
// ─────────────────────────────────────────────────────────────────────────────

/// Run the host gRPC session.
///
/// Screen capture + delta encoding run on a dedicated OS thread so they never
/// block the async runtime.  Frames are handed off through a small bounded
/// channel; if the gRPC sink is slow the capture side drops stale frames
/// instead of accumulating a queue.
pub async fn run_host_session(
    server_url: String,
    device_id: String,
    password: String,
    app_handle: AppHandle,
    input_tx: mpsc::Sender<InputEvent>,
    stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let mut client = connect(&server_url).await?;

    // Outbound channel: host → server (announce + frames)
    let (out_tx, out_rx) = mpsc::channel::<HostMessage>(4);

    // First message MUST be announce.
    out_tx
        .send(HostMessage {
            payload: Some(proto::remote_desktop::host_message::Payload::Announce(
                HostAnnounce {
                    device_id: device_id.clone(),
                    password: password.clone(),
                },
            )),
        })
        .await?;

    let response = client.host_session(ReceiverStream::new(out_rx)).await?;
    let mut inbound = response.into_inner();

    // ── Capture thread ────────────────────────────────────────────────────────
    // Shared flag: set to true when a new client joins so the capture thread
    // sends a full keyframe immediately instead of waiting for the interval.
    let force_keyframe = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let force_keyframe_capture = Arc::clone(&force_keyframe);

    // A small channel (capacity 2) acts as a one-frame lookahead buffer.
    // If it is full the capture thread drops the new frame and moves on —
    // the network is the bottleneck, not the encoder.
    let (frame_tx, mut frame_rx) = mpsc::channel::<FrameData>(2);

    std::thread::spawn(move || {
        let mut state = CaptureState::new(force_keyframe_capture);
        loop {
            let t0 = Instant::now();
            match crate::capture::capture_primary() {
                Ok(img) => {
                    if let Some(frame) = compute_frame(&mut state, &img) {
                        match frame_tx.try_send(frame) {
                            Ok(()) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                tracing::debug!("frame dropped (backpressure)");
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                }
                Err(e) => tracing::warn!("screen capture error: {e}"),
            }
            let interval = state.sleep_interval();
            let elapsed = t0.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
    });

    // ── Async pump: forward frames from capture thread to gRPC ────────────────
    let out_tx2 = out_tx.clone();
    tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            let msg = HostMessage {
                payload: Some(proto::remote_desktop::host_message::Payload::Frame(frame)),
            };
            if out_tx2.send(msg).await.is_err() {
                break;
            }
        }
    });

    // ── Inbound relay loop ─────────────────────────────────────────────────────
    let mut stop_rx = stop_rx;
    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                tracing::info!("host session stopped by command");
                break;
            }
            msg = inbound.message() => {
                match msg {
                    Ok(Some(srv_msg)) => {
                        use proto::remote_desktop::server_to_host::Payload;
                        match srv_msg.payload {
                            Some(Payload::ClientJoined(_)) => {
                                tracing::info!("client joined host session — forcing keyframe");
                                // Force the capture thread to send a full keyframe
                                // immediately so the new client gets a complete picture.
                                force_keyframe.store(true, std::sync::atomic::Ordering::Relaxed);
                                let _ = app_handle.emit("peer-connected", ());
                            }
                            Some(Payload::ClientLeft(_)) => {
                                tracing::info!("client left host session");
                                let _ = app_handle.emit("peer-disconnected", ());
                            }
                            Some(Payload::Input(evt)) => {
                                let _ = input_tx.send(evt).await;
                            }
                            Some(Payload::Clipboard(clip)) => {
                                let _ = app_handle.emit("clipboard-sync", clip.text);
                            }
                            Some(Payload::Error(e)) => {
                                tracing::error!("server error: {}", e.message);
                                let _ = app_handle.emit("connection-error", e.message);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => {
                        tracing::info!("host session stream ended by server");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("host session recv error: {e}");
                        let _ = app_handle.emit("connection-error", e.to_string());
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// CLIENT session
// ─────────────────────────────────────────────────────────────────────────────

/// Run the client gRPC session.  Receives FrameData (keyframe or delta tiles),
/// emits Tauri events for the canvas renderer.
///
/// `join_tx` is signalled as soon as the JoinResult is known so that the
/// caller can unblock and navigate to the Session page before the frame
/// loop starts.  The channel is dropped (not signalled again) after that.
pub async fn run_client_session(
    server_url: String,
    target_id: String,
    password: String,
    app_handle: AppHandle,
    input_rx: mpsc::Receiver<InputEvent>,
    stop_rx: oneshot::Receiver<()>,
    join_tx: oneshot::Sender<Result<()>>,
) -> Result<()> {
    tracing::info!("[client] connecting to server: {server_url}");
    let mut client = match connect(&server_url).await {
        Ok(c) => { tracing::info!("[client] gRPC channel established"); c }
        Err(e) => {
            tracing::error!("[client] failed to connect to server: {e}");
            let _ = join_tx.send(Err(anyhow::anyhow!("{e}")));
            return Err(e);
        }
    };

    let (out_tx, out_rx) = mpsc::channel::<ClientMessage>(64);

    tracing::info!("[client] sending Join for target={target_id}");
    out_tx
        .send(ClientMessage {
            payload: Some(proto::remote_desktop::client_message::Payload::Join(
                ClientJoin {
                    device_id: target_id.clone(),
                    password: password.clone(),
                },
            )),
        })
        .await?;

    // Forward input events from the frontend to the server.
    let out_tx2 = out_tx.clone();
    tokio::spawn(async move {
        let mut input_rx = input_rx;
        while let Some(evt) = input_rx.recv().await {
            let msg = ClientMessage {
                payload: Some(proto::remote_desktop::client_message::Payload::Input(evt)),
            };
            if out_tx2.send(msg).await.is_err() {
                break;
            }
        }
    });

    tracing::info!("[client] opening ClientSession stream");
    let response = match client.client_session(ReceiverStream::new(out_rx)).await {
        Ok(r) => { tracing::info!("[client] stream opened, waiting for JoinResult"); r }
        Err(e) => {
            tracing::error!("[client] client_session RPC failed: {e}");
            let _ = join_tx.send(Err(anyhow::anyhow!("{e}")));
            return Err(e.into());
        }
    };
    let mut inbound = response.into_inner();

    // Wait for JoinResult — signal join_tx so join_room can return to the frontend.
    match inbound.message().await {
        Ok(Some(msg)) => {
            use proto::remote_desktop::server_to_client::Payload;
            match msg.payload {
                Some(Payload::JoinResult(jr)) => {
                    if !jr.ok {
                        tracing::error!("[client] join rejected: {}", jr.error);
                        let err = anyhow::anyhow!("join rejected: {}", jr.error);
                        let _ = join_tx.send(Err(anyhow::anyhow!("join rejected: {}", jr.error)));
                        return Err(err);
                    }
                    tracing::info!("[client] JoinResult OK (host addr: {})", jr.host_public_addr);
                    let _ = join_tx.send(Ok(()));
                }
                other => {
                    let err = anyhow::anyhow!("unexpected first message: {:?}", other.map(|_| "non-join-result"));
                    tracing::error!("[client] {err}");
                    let _ = join_tx.send(Err(anyhow::anyhow!("{err}")));
                    return Err(err);
                }
            }
        }
        Ok(None) => {
            let err = anyhow::anyhow!("server closed stream before join result");
            tracing::error!("[client] {err}");
            let _ = join_tx.send(Err(anyhow::anyhow!("{err}")));
            return Err(err);
        }
        Err(e) => {
            tracing::error!("[client] error waiting for JoinResult: {e}");
            let _ = join_tx.send(Err(anyhow::anyhow!("{e}")));
            return Err(e.into());
        }
    }

    // Inbound frame loop
    tracing::info!("[client] entering frame receive loop");
    let mut frame_count = 0u64;
    let mut stop_rx = stop_rx;
    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                tracing::info!("[client] session stopped by command");
                break;
            }
            msg = inbound.message() => {
                match msg {
                    Ok(Some(srv_msg)) => {
                        use proto::remote_desktop::server_to_client::Payload;
                        match srv_msg.payload {
                            Some(Payload::Frame(frame)) => {
                                frame_count += 1;
                                if frame_count == 1 {
                                    tracing::info!("[client] first frame received (keyframe={})", frame.is_keyframe);
                                } else if frame_count % 100 == 0 {
                                    tracing::info!("[client] {} frames received", frame_count);
                                }
                                emit_frame(&app_handle, frame);
                            }
                            Some(Payload::HostLeft(_)) => {
                                tracing::info!("[client] host left");
                                let _ = app_handle.emit("peer-disconnected", ());
                                break;
                            }
                            Some(Payload::Clipboard(clip)) => {
                                let _ = app_handle.emit("clipboard-sync", clip.text);
                            }
                            Some(Payload::Error(e)) => {
                                tracing::error!("[client] server error: {}", e.message);
                                let _ = app_handle.emit("connection-error", e.message);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => {
                        tracing::info!("[client] stream ended by server");
                        let _ = app_handle.emit("peer-disconnected", ());
                        break;
                    }
                    Err(e) => {
                        tracing::error!("[client] recv error: {e}");
                        let _ = app_handle.emit("connection-error", e.to_string());
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Emit a FrameData to the frontend.
///
/// Keyframes are sent as a single `"frame-keyframe"` event with the full JPEG.
/// Delta frames are sent as a `"frame-delta"` event with an array of tiles
/// (each tile carries its position, size, and base64 JPEG).
fn emit_frame(app: &AppHandle, frame: FrameData) {
    if frame.is_keyframe {
        let _ = app.emit(
            "frame-keyframe",
            serde_json::json!({
                "width":  frame.width,
                "height": frame.height,
                "data":   B64.encode(&frame.jpeg_data),
            }),
        );
    } else {
        let tiles: Vec<_> = frame
            .tiles
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "x":    t.x,
                    "y":    t.y,
                    "w":    t.tile_width,
                    "h":    t.tile_height,
                    "data": B64.encode(&t.jpeg_data),
                })
            })
            .collect();
        if !tiles.is_empty() {
            let _ = app.emit(
                "frame-delta",
                serde_json::json!({
                    "width":  frame.width,
                    "height": frame.height,
                    "tiles":  tiles,
                }),
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::sync::atomic::AtomicBool;

    fn solid(r: u8, g: u8, b: u8, w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |_, _| Rgba([r, g, b, 255]))
    }

    fn new_state() -> CaptureState {
        CaptureState::new(Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn keyframe_on_first_call() {
        let mut state = new_state();
        let img = solid(128, 64, 32, 256, 256);
        let frame = compute_frame(&mut state, &img).expect("first call must return keyframe");
        assert!(frame.is_keyframe);
        assert!(!frame.jpeg_data.is_empty());
        assert!(frame.tiles.is_empty());
        // JPEG magic bytes 0xFF 0xD8
        assert_eq!(&frame.jpeg_data[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn identical_frames_produce_no_output() {
        let mut state = new_state();
        let img = solid(10, 20, 30, 256, 256);
        let _ = compute_frame(&mut state, &img); // keyframe
        // Same image again — nothing changed
        let delta = compute_frame(&mut state, &img);
        assert!(delta.is_none(), "identical frame should produce None");
    }

    #[test]
    fn single_pixel_change_produces_one_tile() {
        let mut state = new_state();
        let img1 = solid(100, 100, 100, 256, 256);
        let _ = compute_frame(&mut state, &img1); // keyframe

        // Change one pixel in the top-left tile
        let mut img2 = img1.clone();
        img2.put_pixel(5, 5, Rgba([200, 200, 200, 255]));
        let delta = compute_frame(&mut state, &img2).expect("should produce delta");
        assert!(!delta.is_keyframe);
        assert_eq!(delta.tiles.len(), 1, "only one tile should be dirty");
        let tile = &delta.tiles[0];
        assert_eq!(tile.x, 0);
        assert_eq!(tile.y, 0);
        assert!(!tile.jpeg_data.is_empty());
    }

    #[test]
    fn resolution_change_triggers_keyframe() {
        let mut state = new_state();
        let img1 = solid(50, 50, 50, 256, 256);
        let _ = compute_frame(&mut state, &img1);

        let img2 = solid(50, 50, 50, 512, 384); // different resolution
        let frame = compute_frame(&mut state, &img2).expect("resolution change must keyframe");
        assert!(frame.is_keyframe);
    }

    #[test]
    fn hash_tile_different_content() {
        let img1 = solid(0, 0, 0, 128, 128);
        let img2 = solid(255, 255, 255, 128, 128);
        let h1 = hash_tile(img1.as_raw(), 128, 0, 0, 128, 128);
        let h2 = hash_tile(img2.as_raw(), 128, 0, 0, 128, 128);
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_tile_same_content() {
        let img = solid(77, 88, 99, 128, 128);
        let h1 = hash_tile(img.as_raw(), 128, 0, 0, 128, 128);
        let h2 = hash_tile(img.as_raw(), 128, 0, 0, 128, 128);
        assert_eq!(h1, h2);
    }
}
