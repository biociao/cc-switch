//! Codex 免登录（本地模拟会话）相关命令。
//!
//! 生成/移除/查询 `~/.codex/auth.json` 中的合成 ChatGPT 会话，让 Codex
//! 桌面端在使用第三方供应商或本地路由时不再强制官方登录。详见
//! `codex_synthetic_login` 模块文档。

use serde::Serialize;

use crate::codex_config;
use crate::codex_synthetic_login as synthetic;
use crate::error::AppError;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSyntheticLoginStatus {
    /// live auth.json 当前是合成会话
    pub active: bool,
    /// live auth.json 存在真实的 ChatGPT 登录材料（非合成）
    pub real_login: bool,
    /// macOS Keychain 存在 "Codex Auth" 项，可能遮蔽 auth.json
    pub keychain_conflict: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSyntheticLoginResult {
    /// 本次是否写入了新的合成会话
    pub wrote: bool,
    /// 已存在真实登录，拒绝覆盖
    pub already_real_login: bool,
    pub keychain_conflict: bool,
}

/// 读取 live auth.json；文件缺失或不可读时视为 None。
fn read_live_auth() -> Option<serde_json::Value> {
    let auth_path = codex_config::get_codex_auth_path();
    if !auth_path.exists() {
        return None;
    }
    crate::config::read_json_file(&auth_path).ok()
}

fn live_has_real_login(auth: Option<&serde_json::Value>) -> bool {
    auth.is_some_and(|auth| {
        codex_config::codex_auth_has_credential_login_material(auth)
            && !synthetic::is_synthetic_codex_auth(auth)
    })
}

/// macOS 上检测 Keychain 中是否存在 "Codex Auth" 项（不读取内容）。
/// 旧版 Codex 会把登录态写进钥匙串并优先于 auth.json，合成会话可能被遮蔽。
#[cfg(target_os = "macos")]
fn codex_keychain_auth_exists() -> bool {
    std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Codex Auth"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn codex_keychain_auth_exists() -> bool {
    false
}

/// 确保 config.toml 顶层 `cli_auth_credentials_store = "file"`，让 Codex
/// 从 auth.json 读取凭据。仅在键缺失或取值非 "file" 时改动，其余内容不动。
fn ensure_codex_credentials_store_file() -> Result<(), AppError> {
    let config_text = codex_config::read_codex_config_text()?;
    let current = config_text
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("cli_auth_credentials_store")
                .and_then(|item| item.as_str())
                .map(str::to_string)
        });
    if current.as_deref() == Some("file") {
        return Ok(());
    }

    let mut doc = if config_text.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        config_text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?
    };
    doc["cli_auth_credentials_store"] = toml_edit::value("file");
    codex_config::write_codex_live_config_atomic(Some(&doc.to_string()))
}

/// 查询免登录会话状态。
#[tauri::command]
pub async fn get_codex_synthetic_login_status() -> Result<CodexSyntheticLoginStatus, String> {
    let live_auth = read_live_auth();
    Ok(CodexSyntheticLoginStatus {
        active: live_auth
            .as_ref()
            .is_some_and(synthetic::is_synthetic_codex_auth),
        real_login: live_has_real_login(live_auth.as_ref()),
        keychain_conflict: codex_keychain_auth_exists(),
    })
}

/// 生成并写入合成登录会话。
///
/// 已存在真实 ChatGPT 登录时拒绝覆盖；已存在合成会话时直接复用。
#[tauri::command]
pub async fn generate_codex_synthetic_login() -> Result<CodexSyntheticLoginResult, String> {
    let live_auth = read_live_auth();
    if live_has_real_login(live_auth.as_ref()) {
        return Ok(CodexSyntheticLoginResult {
            wrote: false,
            already_real_login: true,
            keychain_conflict: codex_keychain_auth_exists(),
        });
    }
    let already_synthetic = live_auth
        .as_ref()
        .is_some_and(synthetic::is_synthetic_codex_auth);

    let mut wrote = false;
    if !already_synthetic {
        let auth = synthetic::generate_synthetic_codex_auth().map_err(|e| e.to_string())?;
        let auth_path = codex_config::get_codex_auth_path();
        if let Some(parent) = auth_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e).to_string())?;
        }
        crate::config::write_json_file(&auth_path, &auth).map_err(|e| e.to_string())?;
        wrote = true;
    }

    ensure_codex_credentials_store_file().map_err(|e| e.to_string())?;

    Ok(CodexSyntheticLoginResult {
        wrote,
        already_real_login: false,
        keychain_conflict: codex_keychain_auth_exists(),
    })
}

/// 移除合成登录会话。仅当 live auth.json 判定为合成时才删除，
/// 绝不触碰真实登录。返回是否实际删除。
#[tauri::command]
pub async fn remove_codex_synthetic_login() -> Result<bool, String> {
    let Some(live_auth) = read_live_auth() else {
        return Ok(false);
    };
    if !synthetic::is_synthetic_codex_auth(&live_auth) {
        return Ok(false);
    }
    crate::config::delete_file(&codex_config::get_codex_auth_path()).map_err(|e| e.to_string())?;
    Ok(true)
}
