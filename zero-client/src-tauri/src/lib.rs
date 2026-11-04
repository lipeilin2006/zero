use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedServer {
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub last_opened: Option<u64>,
}

fn store_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zero-client")
        .join("servers.json")
}

fn load_servers() -> Result<Vec<SavedServer>, String> {
    let path = store_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn write_servers(servers: &[SavedServer]) -> Result<(), String> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(servers).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

#[tauri::command]
fn list_servers() -> Result<Vec<SavedServer>, String> {
    load_servers()
}

#[tauri::command]
fn save_servers(servers: Vec<SavedServer>) -> Result<(), String> {
    write_servers(&servers)
}

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
        .invoke_handler(tauri::generate_handler![list_servers, save_servers, launch_zero])
        .run(tauri::generate_context!())
        .expect("error while running zero-client");
}
