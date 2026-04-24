pub mod capture;
pub mod clipboard;
pub mod input;
pub mod relay_client;
pub mod state;

use std::sync::Arc;
use std::fs;

use input::InputController;
use proto::remote_desktop::{input_event, InputEvent, MouseButton, MouseMove, MouseScroll, KeyEvent};
use state::{AppRole, AppState};
use tauri::{Emitter, Manager, State};
use tokio::sync::{mpsc, oneshot};

fn get_primary_screen_size() -> (u32, u32) {
    xcap::Monitor::all()
        .ok()
        .and_then(|monitors| {
            monitors
                .into_iter()
                .find(|m| m.is_primary())
                .map(|m| (m.width(), m.height()))
        })
        .unwrap_or((1920, 1080))
}

pub(crate) mod commands {
    use super::*;

    #[tauri::command]
    pub async fn set_server_url(
        url: String,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        *app_state.server_url.write().await = url;
        Ok(())
    }

    #[tauri::command]
    pub async fn get_or_create_device_id(app: tauri::AppHandle) -> Result<String, String> {
        let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let id_file = data_dir.join("device_id");

        if let Ok(id) = fs::read_to_string(&id_file) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return Ok(id);
            }
        }

        let new_id: String = uuid::Uuid::new_v4()
            .as_bytes()
            .iter()
            .take(4)
            .map(|b| format!("{b:02X}"))
            .collect();

        fs::write(&id_file, &new_id).map_err(|e| e.to_string())?;
        Ok(new_id)
    }

    #[tauri::command]
    pub async fn create_room(
        password: String,
        device_id: Option<String>,
        app: tauri::AppHandle,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<String, String> {
        // Return existing room if already hosting.
        {
            let role = app_state.role.read().await;
            let room = app_state.room_id.read().await;
            if *role == AppRole::Host {
                if let Some(id) = room.clone() {
                    return Ok(id);
                }
            }
        }

        let server_url = app_state.server_url.read().await.clone();

        // Determine device ID to use.
        let local_id = device_id.unwrap_or_else(|| {
            app.path()
                .app_data_dir()
                .ok()
                .and_then(|d| fs::read_to_string(d.join("device_id")).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        });

        // Register with the server (confirm / claim device ID).
        let confirmed_id = relay_client::register_device(&server_url, &local_id)
            .await
            .map_err(|e| e.to_string())?;

        // Persist confirmed ID.
        if let Ok(data_dir) = app.path().app_data_dir() {
            let _ = fs::create_dir_all(&data_dir);
            let _ = fs::write(data_dir.join("device_id"), &confirmed_id);
        }

        *app_state.role.write().await = AppRole::Host;
        *app_state.room_id.write().await = Some(confirmed_id.clone());

        // Input event channel: relay_client → InputController.
        let (input_tx, mut input_rx) = mpsc::channel::<InputEvent>(64);
        *app_state.input_tx.lock().await = Some(input_tx.clone());

        // Stop channel.
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        *app_state.stop_tx.lock().await = Some(stop_tx);

        let app_handle = app.clone();
        let state_clone = Arc::clone(&app_state);
        let device_id_clone = confirmed_id.clone();

        tokio::spawn(async move {
            // Input injection task.
            // Spawn a dedicated OS thread because Enigo on macOS wraps CGEventSource
            // (a raw CoreFoundation pointer) which is not `Send`, preventing use of
            // tokio::spawn. A regular thread + blocking_recv is the correct pattern.
            let (sw, sh) = get_primary_screen_size();
            std::thread::spawn(move || {
                let mut injector = match InputController::new(sw, sh) {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::error!("InputController unavailable: {e}");
                        return;
                    }
                };
                while let Some(evt) = input_rx.blocking_recv() {
                    if let Err(e) = injector.handle(&evt) {
                        tracing::warn!("input inject error: {e}");
                    }
                }
            });
            let app_clone = app_handle.clone();

            match relay_client::run_host_session(
                server_url,
                device_id_clone,
                password,
                app_clone,
                input_tx,
                stop_rx,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("host session error: {e}");
                    let _ = app_handle.emit("connection-error", e.to_string());
                }
            }
            state_clone.reset().await;
        });

        Ok(confirmed_id)
    }

    #[tauri::command]
    pub async fn join_room(
        room_id: String,
        password: String,
        app: tauri::AppHandle,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        let server_url = app_state.server_url.read().await.clone();
        tracing::info!("[join_room] target={room_id} server={server_url}");

        let (input_tx, input_rx) = mpsc::channel::<InputEvent>(64);
        *app_state.input_tx.lock().await = Some(input_tx);

        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        *app_state.stop_tx.lock().await = Some(stop_tx);
        *app_state.role.write().await = AppRole::Client;

        // join_tx is passed into run_client_session and signalled as soon as
        // JoinResult is received (success or failure).  We await join_rx here so
        // join_room only returns to the frontend after the connection is confirmed,
        // eliminating the race where the frontend navigates before listeners fire.
        let (join_tx, join_rx) = oneshot::channel::<anyhow::Result<()>>();

        let app_handle = app.clone();
        let state_clone = Arc::clone(&app_state);

        tokio::spawn(async move {
            let result = relay_client::run_client_session(
                server_url,
                room_id,
                password,
                app_handle.clone(),
                input_rx,
                stop_rx,
                join_tx,
            )
            .await;

            if let Err(e) = result {
                tracing::error!("[join_room] session ended with error: {e}");
                let _ = app_handle.emit("connection-error", e.to_string());
            }
            state_clone.reset().await;
        });

        // Block until JoinResult is known.
        match join_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("connection task dropped before join result".to_string()),
        }
    }

    /// Send a mouse-move input event (client mode).
    #[tauri::command]
    pub async fn send_mouse_move(
        x: f32,
        y: f32,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        let evt = InputEvent {
            event: Some(input_event::Event::MouseMove(MouseMove { x, y })),
        };
        send_input_event(evt, &app_state).await
    }

    /// Send a mouse-button input event (client mode).
    #[tauri::command]
    pub async fn send_mouse_button(
        button: String,
        pressed: bool,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        let evt = InputEvent {
            event: Some(input_event::Event::MouseButton(MouseButton { button, pressed })),
        };
        send_input_event(evt, &app_state).await
    }

    /// Send a mouse-scroll input event (client mode).
    #[tauri::command]
    pub async fn send_mouse_scroll(
        dx: i32,
        dy: i32,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        let evt = InputEvent {
            event: Some(input_event::Event::MouseScroll(MouseScroll { dx, dy })),
        };
        send_input_event(evt, &app_state).await
    }

    /// Send a key input event (client mode).
    #[tauri::command]
    pub async fn send_key(
        key: String,
        pressed: bool,
        app_state: State<'_, Arc<AppState>>,
    ) -> Result<(), String> {
        let evt = InputEvent {
            event: Some(input_event::Event::Key(KeyEvent { key, pressed })),
        };
        send_input_event(evt, &app_state).await
    }

    async fn send_input_event(
        evt: InputEvent,
        app_state: &Arc<AppState>,
    ) -> Result<(), String> {
        if let Some(tx) = app_state.input_tx.lock().await.as_ref() {
            tx.send(evt).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[tauri::command]
    pub async fn disconnect(app_state: State<'_, Arc<AppState>>) -> Result<(), String> {
        if let Some(tx) = app_state.stop_tx.lock().await.take() {
            let _ = tx.send(());
        }
        app_state.reset().await;
        Ok(())
    }

    #[tauri::command]
    pub async fn get_monitors() -> Result<Vec<serde_json::Value>, String> {
        use xcap::Monitor;
        let monitors = Monitor::all().map_err(|e| e.to_string())?;
        Ok(monitors
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id(),
                    "name": m.name(),
                    "width": m.width(),
                    "height": m.height(),
                    "is_primary": m.is_primary(),
                })
            })
            .collect())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing so [client]/[join_room] logs appear in the terminal.
    use tracing_subscriber::{fmt, EnvFilter};
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("desktop_lib=debug,info")),
        )
        .init();

    let app_state = Arc::new(AppState::new("http://127.0.0.1:50055"));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::set_server_url,
            commands::get_or_create_device_id,
            commands::create_room,
            commands::join_room,
            commands::send_mouse_move,
            commands::send_mouse_button,
            commands::send_mouse_scroll,
            commands::send_key,
            commands::disconnect,
            commands::get_monitors,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
