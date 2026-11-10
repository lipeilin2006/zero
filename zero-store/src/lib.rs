//! 服务器列表：CLI 和 Tauri 客户端共用一个 JSON 文件。
//!
//! 路径：`$DATA/zero-client/servers.json`（沿用客户端原来的位置，
//! 老用户的数据不用搬）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedServer {
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub last_opened: Option<u64>,
}

pub fn store_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("zero-client")
        .join("servers.json")
}

/// 读列表；文件不存在就当空列表。
pub fn load() -> Result<Vec<SavedServer>> {
    let path = store_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).with_context(|| format!("解析 {} 失败", path.display()))
}

/// 原子写：先写临时文件再 rename，写到一半崩不会丢整个列表。
pub fn save(servers: &[SavedServer]) -> Result<()> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 {} 失败", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(servers)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("替换 {} 失败", path.display()))?;
    Ok(())
}

/// 按名称或 endpoint 前缀找一条。
pub fn find(target: &str) -> Result<Option<SavedServer>> {
    let target = target.trim();
    if target.is_empty() {
        return Ok(None);
    }
    let servers = load()?;
    Ok(servers.into_iter().find(|server| matches(server, target)))
}

fn matches(server: &SavedServer, target: &str) -> bool {
    server.name.eq_ignore_ascii_case(target)
        || server.endpoint == target
        || (target.len() >= 8 && server.endpoint.starts_with(target))
}

/// 添加一条；名称或 endpoint 重复会报错。
pub fn add(name: &str, endpoint: &str, tags: Vec<String>) -> Result<SavedServer> {
    let name = name.trim();
    let endpoint = endpoint.trim();
    if name.is_empty() {
        bail!("名称不能为空");
    }
    if endpoint.is_empty() {
        bail!("endpoint 不能为空");
    }
    let mut servers = load()?;
    if servers.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
        bail!("已经有一个叫「{name}」的服务器了");
    }
    if servers.iter().any(|s| s.endpoint == endpoint) {
        bail!("这个 endpoint 已经保存过了");
    }
    let server = SavedServer {
        name: name.to_string(),
        endpoint: endpoint.to_string(),
        tags,
        last_opened: None,
    };
    servers.push(server.clone());
    save(&servers)?;
    Ok(server)
}

/// 删除一条，返回被删掉的记录。
pub fn remove(target: &str) -> Result<SavedServer> {
    let mut servers = load()?;
    let index = servers
        .iter()
        .position(|server| matches(server, target.trim()))
        .with_context(|| format!("找不到「{target}」，用 zero list 看看有哪些"))?;
    let removed = servers.remove(index);
    save(&servers)?;
    Ok(removed)
}

/// 记录一次打开时间。
pub fn touch(target: &str) -> Result<()> {
    let mut servers = load()?;
    let Some(server) = servers
        .iter_mut()
        .find(|server| matches(server, target.trim()))
    else {
        return Ok(());
    };
    server.last_opened = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    save(&servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_store<T>(f: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!("zero-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 用 XDG_DATA_HOME 指向临时目录，避免碰真实数据
        std::env::set_var("XDG_DATA_HOME", &dir);
        let out = f();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn add_find_remove_roundtrip() {
        with_temp_store(|| {
            add("dev", "abc123", vec!["linux".into()]).unwrap();
            assert!(add("DEV", "other", vec![]).is_err(), "名字重复要拒绝");
            assert!(
                add("dev2", "abc123", vec![]).is_err(),
                "endpoint 重复要拒绝"
            );

            let found = find("dev").unwrap().unwrap();
            assert_eq!(found.endpoint, "abc123");
            assert_eq!(found.tags, vec!["linux".to_string()]);

            let removed = remove("abc123").unwrap();
            assert_eq!(removed.name, "dev");
            assert!(find("dev").unwrap().is_none());
        });
    }
}
