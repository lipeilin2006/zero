pub mod session;

use tauri::Manager;

use session::SessionManager;

pub use zero_store::SavedServer;

/// 服务器列表现在和 CLI 共用一份（`$DATA/zero-client/servers.json`）。
#[tauri::command]
fn list_servers() -> Result<Vec<SavedServer>, String> {
    zero_store::load().map_err(|error| error.to_string())
}

#[tauri::command]
fn save_servers(servers: Vec<SavedServer>) -> Result<(), String> {
    zero_store::save(&servers).map_err(|error| error.to_string())
}

/// 当前客户端的 endpoint id（给服务端做 `zero-server trust` 用）。
#[tauri::command]
fn client_id() -> Result<String, String> {
    session::client_id().map_err(|error| error.to_string())
}

/// 调试用：把前端日志追加到 ZERO_UI_LOG 指向的文件（没设就什么都不做）。
#[tauri::command]
fn ui_log(line: String) {
    let Ok(path) = std::env::var("ZERO_UI_LOG") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// 调试用：自动打开一个终端（ZERO_AUTO_ENDPOINT）。
#[tauri::command]
fn auto_endpoint() -> String {
    std::env::var("ZERO_AUTO_ENDPOINT").unwrap_or_default()
}

/// 打开一个内嵌终端会话，返回会话 id。
#[tauri::command]
async fn session_open(
    endpoint: String,
    cols: Option<u16>,
    rows: Option<u16>,
    sessions: tauri::State<'_, SessionManager>,
) -> Result<u32, String> {
    sessions
        .open(
            &endpoint,
            cols.unwrap_or(120).max(1),
            rows.unwrap_or(30).max(1),
        )
        .await
        .map_err(|error| error.to_string())
}

/// 往会话里发输入（用户敲的一行或控制字符）。
#[tauri::command]
async fn session_input(
    id: u32,
    data: String,
    sessions: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    sessions
        .input(id, data.into_bytes())
        .await
        .map_err(|error| error.to_string())
}

/// 终端尺寸变化。
#[tauri::command]
async fn session_resize(
    id: u32,
    cols: u16,
    rows: u16,
    sessions: tauri::State<'_, SessionManager>,
) -> Result<(), String> {
    sessions
        .resize(id, cols, rows)
        .await
        .map_err(|error| error.to_string())
}

/// 关闭会话（发 Disconnect 并收掉后台任务）。
#[tauri::command]
async fn session_close(id: u32, sessions: tauri::State<'_, SessionManager>) -> Result<(), String> {
    sessions.close(id).await;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(SessionManager::new(app.handle().clone()));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    app.state::<SessionManager>().close_all().await;
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            ui_log,
            client_id,
            auto_endpoint,
            list_servers,
            save_servers,
            session_open,
            session_input,
            session_resize,
            session_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running zero-client");
}
