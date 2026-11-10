pub mod session;

use std::path::PathBuf;

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

/// 仍然保留：用系统终端模拟器打开 CLI（旧行为，作为兜底）。
#[tauri::command]
fn launch_zero(endpoint: String) -> Result<(), String> {
    let executable = if let Ok(path) = std::env::var("ZERO_BIN") {
        PathBuf::from(path)
    } else {
        let filename = format!("zero{}", std::env::consts::EXE_SUFFIX);
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join(&filename)))
            .filter(|path| path.exists())
            .unwrap_or_else(|| PathBuf::from(filename))
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(executable)
            .arg(endpoint)
            .creation_flags(0x00000010)
            .spawn()
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[cfg(unix)]
    {
        for (terminal, option) in [
            ("x-terminal-emulator", "-e"),
            ("gnome-terminal", "--"),
            ("konsole", "-e"),
            ("xfce4-terminal", "-x"),
            ("alacritty", "-e"),
            ("kitty", ""),
            ("xterm", "-e"),
        ] {
            let mut command = std::process::Command::new(terminal);
            if !option.is_empty() {
                command.arg(option);
            }
            if command.arg(&executable).arg(&endpoint).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("未找到可用的终端模拟器".into())
    }

    #[cfg(not(any(windows, unix)))]
    {
        Err("不支持当前操作系统".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            list_servers,
            save_servers,
            launch_zero,
            session_open,
            session_input,
            session_resize,
            session_close
        ])
        .run(tauri::generate_context!())
        .expect("error while running zero-client");
}
