use tokio::sync::{oneshot, Mutex, RwLock};

/// Whether this instance is acting as host (controlled) or client (controller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRole {
    Idle,
    Host,
    Client,
}

/// Global Tauri app state shared across commands.
pub struct AppState {
    pub role: RwLock<AppRole>,
    /// Confirmed device ID (same as what is stored on disk).
    pub room_id: RwLock<Option<String>>,
    /// gRPC server URL (e.g. `"http://localhost:50051"`).
    pub server_url: RwLock<String>,
    /// Sender that, when triggered, gracefully stops the running session task.
    pub stop_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Sender to forward input events from the client frontend to the relay.
    /// Set while a client session is running.
    pub input_tx:
        Mutex<Option<tokio::sync::mpsc::Sender<proto::remote_desktop::InputEvent>>>,
}

impl AppState {
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            role: RwLock::new(AppRole::Idle),
            room_id: RwLock::new(None),
            server_url: RwLock::new(server_url.into()),
            stop_tx: Mutex::new(None),
            input_tx: Mutex::new(None),
        }
    }

    /// Reset state after a session ends.
    pub async fn reset(&self) {
        *self.role.write().await = AppRole::Idle;
        *self.stop_tx.lock().await = None;
        *self.input_tx.lock().await = None;
        // NOTE: room_id is intentionally NOT cleared — it is the persistent
        // device identity and should survive session restarts.
    }
}

