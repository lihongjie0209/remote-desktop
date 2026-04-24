use std::net::SocketAddr;
use std::sync::Arc;

use dashmap::DashMap;
use proto::remote_desktop::{
    remote_desktop_server::{RemoteDesktop, RemoteDesktopServer},
    ClientJoined, ClientLeft, HostLeft, JoinResult, PeerEndpoint, RegisterRequest,
    RegisterResponse, ServerError, ServerToClient, ServerToHost,
};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server as TonicServer, Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::{auth, Args};

// ── Session state ─────────────────────────────────────────────────────────────

struct HostSession {
    password_hash: String,
    /// Channel to forward messages from client → host.
    tx: mpsc::Sender<Result<ServerToHost, Status>>,
    /// Host's observed public address (for P2P).
    public_addr: String,
}

struct ClientSession {
    #[allow(dead_code)]
    peer_id: String,
    tx: mpsc::Sender<Result<ServerToClient, Status>>,
    #[allow(dead_code)]
    public_addr: String,
}

struct RoomEntry {
    host: Option<HostSession>,
    client: Option<ClientSession>,
}

// ── Registry ─────────────────────────────────────────────────────────────────

pub struct Registry {
    rooms: DashMap<String, Arc<Mutex<RoomEntry>>>,
}

impl Registry {
    fn new() -> Self {
        Self {
            rooms: DashMap::new(),
        }
    }

    fn get_or_create_room(&self, device_id: &str) -> Arc<Mutex<RoomEntry>> {
        self.rooms
            .entry(device_id.to_owned())
            .or_insert_with(|| {
                Arc::new(Mutex::new(RoomEntry {
                    host: None,
                    client: None,
                }))
            })
            .clone()
    }

    fn get_room(&self, device_id: &str) -> Option<Arc<Mutex<RoomEntry>>> {
        self.rooms.get(device_id).map(|r| r.clone())
    }
}

// ── gRPC service ─────────────────────────────────────────────────────────────

pub struct RemoteDesktopService {
    registry: Arc<Registry>,
}

impl RemoteDesktopService {
    fn new() -> Self {
        Self {
            registry: Arc::new(Registry::new()),
        }
    }
}

fn server_error(msg: impl Into<String>) -> ServerToHost {
    ServerToHost {
        payload: Some(proto::remote_desktop::server_to_host::Payload::Error(
            ServerError { message: msg.into() },
        )),
    }
}

fn client_error(msg: impl Into<String>) -> ServerToClient {
    ServerToClient {
        payload: Some(proto::remote_desktop::server_to_client::Payload::Error(
            ServerError { message: msg.into() },
        )),
    }
}

#[tonic::async_trait]
impl RemoteDesktop for RemoteDesktopService {
    // ── RegisterDevice ────────────────────────────────────────────────────────
    async fn register_device(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = request.into_inner();
        let device_id = if req.device_id.is_empty() {
            // Generate a new 8-char uppercase hex ID.
            let bytes = Uuid::new_v4().as_bytes().to_owned();
            format!("{:02X}{:02X}{:02X}{:02X}", bytes[0], bytes[1], bytes[2], bytes[3])
        } else {
            req.device_id.clone()
        };
        tracing::info!("RegisterDevice: {device_id}");
        Ok(Response::new(RegisterResponse { device_id }))
    }

    // ── HostSession ───────────────────────────────────────────────────────────
    type HostSessionStream = ReceiverStream<Result<ServerToHost, Status>>;

    async fn host_session(
        &self,
        request: Request<Streaming<proto::remote_desktop::HostMessage>>,
    ) -> Result<Response<Self::HostSessionStream>, Status> {
        // Extract peer address before consuming the request.
        let public_addr = request
            .remote_addr()
            .map(|a: SocketAddr| a.to_string())
            .unwrap_or_default();

        let mut stream = request.into_inner();
        let registry = Arc::clone(&self.registry);

        // Outbound channel (server → host).
        let (tx, rx) = mpsc::channel::<Result<ServerToHost, Status>>(64);

        tokio::spawn(async move {
            use proto::remote_desktop::host_message::Payload;

            // ── Expect announce as first message ──────────────────────────────
            let first = match stream.message().await {
                Ok(Some(m)) => m,
                _ => return,
            };

            let (device_id, password) = match first.payload {
                Some(Payload::Announce(a)) => (a.device_id, a.password),
                _ => {
                    let _ = tx.send(Ok(server_error("first message must be announce"))).await;
                    return;
                }
            };

            if device_id.is_empty() {
                let _ = tx.send(Ok(server_error("device_id must not be empty"))).await;
                return;
            }

            let password_hash = auth::hash_password(&password);
            tracing::info!("HostSession: device={device_id} addr={public_addr}");

            let room = registry.get_or_create_room(&device_id);
            // Register host and evict any stale client from a previous session.
            let stale_client_tx = {
                let mut entry = room.lock().await;
                entry.host = Some(HostSession {
                    password_hash,
                    tx: tx.clone(),
                    public_addr: public_addr.clone(),
                });
                // Clear stale client — if host reconnects the old client entry
                // must not block new connection attempts.
                entry.client.take().map(|c| c.tx)
            };
            if let Some(ctx) = stale_client_tx {
                // Best-effort: notify the stale client that the host restarted.
                let _ = ctx
                    .send(Ok(ServerToClient {
                        payload: Some(
                            proto::remote_desktop::server_to_client::Payload::HostLeft(
                                HostLeft {},
                            ),
                        ),
                    }))
                    .await;
            }

            // ── Relay loop: receive host frames → forward to client ───────────
            while let Ok(Some(msg)) = stream.message().await {
                match msg.payload {
                    Some(Payload::Frame(frame)) => {
                        // Forward frame to the client if one is connected.
                        let client_tx = {
                            let entry = room.lock().await;
                            entry.client.as_ref().map(|c| c.tx.clone())
                        };
                        if let Some(ctx) = client_tx {
                            let _ = ctx
                                .send(Ok(ServerToClient {
                                    payload: Some(
                                        proto::remote_desktop::server_to_client::Payload::Frame(
                                            frame,
                                        ),
                                    ),
                                }))
                                .await;
                        }
                    }
                    Some(Payload::Clipboard(clip)) => {
                        let client_tx = {
                            let entry = room.lock().await;
                            entry.client.as_ref().map(|c| c.tx.clone())
                        };
                        if let Some(ctx) = client_tx {
                            let _ = ctx
                                .send(Ok(ServerToClient {
                                    payload: Some(
                                        proto::remote_desktop::server_to_client::Payload::Clipboard(
                                            clip,
                                        ),
                                    ),
                                }))
                                .await;
                        }
                    }
                    Some(Payload::Heartbeat(_)) => {}
                    _ => {}
                }
            }

            // ── Host disconnected — notify client ─────────────────────────────
            tracing::info!("HostSession ended: device={device_id}");
            let client_tx = {
                let mut entry = room.lock().await;
                entry.host = None;
                entry.client.as_ref().map(|c| c.tx.clone())
            };
            if let Some(ctx) = client_tx {
                let _ = ctx
                    .send(Ok(ServerToClient {
                        payload: Some(
                            proto::remote_desktop::server_to_client::Payload::HostLeft(
                                HostLeft {},
                            ),
                        ),
                    }))
                    .await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    // ── ClientSession ─────────────────────────────────────────────────────────
    type ClientSessionStream = ReceiverStream<Result<ServerToClient, Status>>;

    async fn client_session(
        &self,
        request: Request<Streaming<proto::remote_desktop::ClientMessage>>,
    ) -> Result<Response<Self::ClientSessionStream>, Status> {
        let public_addr = request
            .remote_addr()
            .map(|a: SocketAddr| a.to_string())
            .unwrap_or_default();

        let mut stream = request.into_inner();
        let registry = Arc::clone(&self.registry);

        let (tx, rx) = mpsc::channel::<Result<ServerToClient, Status>>(64);

        tokio::spawn(async move {
            use proto::remote_desktop::client_message::Payload;

            // ── Expect join as first message ──────────────────────────────────
            let first = match stream.message().await {
                Ok(Some(m)) => m,
                _ => return,
            };

            let (target_id, password) = match first.payload {
                Some(Payload::Join(j)) => (j.device_id, j.password),
                _ => {
                    let _ = tx.send(Ok(client_error("first message must be join"))).await;
                    return;
                }
            };

            tracing::info!("ClientSession: target={target_id} addr={public_addr}");

            // ── Look up host ──────────────────────────────────────────────────
            let room = match registry.get_room(&target_id) {
                Some(r) => r,
                None => {
                    let _ = tx
                        .send(Ok(ServerToClient {
                            payload: Some(
                                proto::remote_desktop::server_to_client::Payload::JoinResult(
                                    JoinResult {
                                        ok: false,
                                        error: "device not found".into(),
                                        host_public_addr: String::new(),
                                    },
                                ),
                            ),
                        }))
                        .await;
                    return;
                }
            };

            let (peer_id, host_public_addr, host_tx) = {
                let mut entry = room.lock().await;

                // Verify host exists and password is correct.
                let (pw_hash, h_addr, h_tx) = match entry.host.as_ref() {
                    Some(h) => (h.password_hash.clone(), h.public_addr.clone(), h.tx.clone()),
                    None => {
                        let _ = tx
                            .send(Ok(ServerToClient {
                                payload: Some(
                                    proto::remote_desktop::server_to_client::Payload::JoinResult(
                                        JoinResult {
                                            ok: false,
                                            error: "host not available".into(),
                                            host_public_addr: String::new(),
                                        },
                                    ),
                                ),
                            }))
                            .await;
                        return;
                    }
                };

                if !auth::verify_password(&password, &pw_hash) {
                    let _ = tx
                        .send(Ok(ServerToClient {
                            payload: Some(
                                proto::remote_desktop::server_to_client::Payload::JoinResult(
                                    JoinResult {
                                        ok: false,
                                        error: "wrong password".into(),
                                        host_public_addr: String::new(),
                                    },
                                ),
                            ),
                        }))
                        .await;
                    return;
                }

                // If a stale client entry remains (channel already closed due
                // to disconnect before the relay loop cleaned up), evict it so
                // new connections are not permanently blocked.
                let client_is_stale = entry.client.as_ref().map(|c| c.tx.is_closed()).unwrap_or(false);
                if client_is_stale {
                    tracing::info!("Evicting stale client for device={target_id}");
                    entry.client = None;
                }

                if entry.client.is_some() {
                    let _ = tx
                        .send(Ok(ServerToClient {
                            payload: Some(
                                proto::remote_desktop::server_to_client::Payload::JoinResult(
                                    JoinResult {
                                        ok: false,
                                        error: "room is full".into(),
                                        host_public_addr: String::new(),
                                    },
                                ),
                            ),
                        }))
                        .await;
                    return;
                }

                let peer_id = Uuid::new_v4().to_string();
                entry.client = Some(ClientSession {
                    peer_id: peer_id.clone(),
                    tx: tx.clone(),
                    public_addr: public_addr.clone(),
                });

                (peer_id, h_addr, h_tx)
            };

            // ── Notify client of successful join ──────────────────────────────
            let _ = tx
                .send(Ok(ServerToClient {
                    payload: Some(
                        proto::remote_desktop::server_to_client::Payload::JoinResult(
                            JoinResult {
                                ok: true,
                                error: String::new(),
                                host_public_addr: host_public_addr.clone(),
                            },
                        ),
                    ),
                }))
                .await;

            // ── Notify host of client join + client's public addr ─────────────
            let _ = host_tx
                .send(Ok(ServerToHost {
                    payload: Some(proto::remote_desktop::server_to_host::Payload::ClientJoined(
                        ClientJoined {
                            peer_id: peer_id.clone(),
                            public_addr: public_addr.clone(),
                        },
                    )),
                }))
                .await;
            // Also send client's endpoint so host can try P2P.
            let _ = host_tx
                .send(Ok(ServerToHost {
                    payload: Some(
                        proto::remote_desktop::server_to_host::Payload::PeerEndpoint(
                            PeerEndpoint {
                                addr: public_addr.clone(),
                            },
                        ),
                    ),
                }))
                .await;

            // ── Relay loop: receive client input → forward to host ────────────
            while let Ok(Some(msg)) = stream.message().await {
                match msg.payload {
                    Some(Payload::Input(input)) => {
                        let host_tx = {
                            let entry = room.lock().await;
                            entry.host.as_ref().map(|h| h.tx.clone())
                        };
                        if let Some(htx) = host_tx {
                            let _ = htx
                                .send(Ok(ServerToHost {
                                    payload: Some(
                                        proto::remote_desktop::server_to_host::Payload::Input(
                                            input,
                                        ),
                                    ),
                                }))
                                .await;
                        }
                    }
                    Some(Payload::Clipboard(clip)) => {
                        let host_tx = {
                            let entry = room.lock().await;
                            entry.host.as_ref().map(|h| h.tx.clone())
                        };
                        if let Some(htx) = host_tx {
                            let _ = htx
                                .send(Ok(ServerToHost {
                                    payload: Some(
                                        proto::remote_desktop::server_to_host::Payload::Clipboard(
                                            clip,
                                        ),
                                    ),
                                }))
                                .await;
                        }
                    }
                    Some(Payload::Heartbeat(_)) => {}
                    _ => {}
                }
            }

            // ── Client disconnected — notify host ─────────────────────────────
            tracing::info!("ClientSession ended: peer={peer_id}");
            let host_tx = {
                let mut entry = room.lock().await;
                entry.client = None;
                entry.host.as_ref().map(|h| h.tx.clone())
            };
            if let Some(htx) = host_tx {
                let _ = htx
                    .send(Ok(ServerToHost {
                        payload: Some(
                            proto::remote_desktop::server_to_host::Payload::ClientLeft(
                                ClientLeft { peer_id },
                            ),
                        ),
                    }))
                    .await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run(args: Args) -> anyhow::Result<()> {
    let addr = format!("{}:{}", args.host, args.port).parse()?;
    tracing::info!("gRPC server listening on {addr}");

    let svc = RemoteDesktopService::new();

    TonicServer::builder()
        .add_service(RemoteDesktopServer::new(svc))
        .serve(addr)
        .await?;

    Ok(())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_creates_room_on_demand() {
        let reg = Registry::new();
        let _ = reg.get_or_create_room("AABB1122");
        assert!(reg.get_room("AABB1122").is_some());
        assert!(reg.get_room("NONEXIST").is_none());
    }

    #[test]
    fn registry_room_count() {
        let reg = Registry::new();
        let _ = reg.get_or_create_room("AAA");
        let _ = reg.get_or_create_room("BBB");
        let _ = reg.get_or_create_room("AAA"); // idempotent
        assert_eq!(reg.rooms.len(), 2);
    }
}
