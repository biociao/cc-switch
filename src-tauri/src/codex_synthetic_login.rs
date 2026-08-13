//! Codex 免登录：本地模拟 ChatGPT 会话。
//!
//! 新版 Codex 桌面 App 启动时强制要求 `~/.codex/auth.json` 中存在 ChatGPT
//! 会话（`auth_mode: "chatgpt"` + `tokens`），仅有 `OPENAI_API_KEY` 会被卡在
//! 登录页；而官方登录需要海外手机号验证，部分用户无法完成。
//!
//! 本模块生成一份**纯本地**的合成会话：自签 HS256 JWT、`exp` 设为 10 年后
//! （Codex 只本地解码 payload 判断过期，不验签；未过期即不会触发 OAuth 刷新），
//! 不与 OpenAI 服务器发生任何交互。实际模型请求照旧走第三方供应商或本地路由
//! （provider 级 `experimental_bearer_token` 优先于 auth.json 参与请求鉴权）。
//!
//! 合成会话通过 id_token 中的标记邮箱识别，任何解析失败都判为"非合成"，
//! 绝不误伤用户的真实登录。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::config::read_json_file;
use crate::error::AppError;

/// 合成会话的标记邮箱，写入 id_token claims 用于识别。
const SYNTHETIC_EMAIL: &str = "cc-switch-synthetic@localhost";
/// JWT 有效期：10 年。足够远，Codex 永远不会因过期而触发 OAuth 刷新流程。
const SYNTHETIC_TOKEN_TTL_SECS: i64 = 10 * 365 * 24 * 3600;

type HmacSha256 = Hmac<Sha256>;

/// 用随机密钥 HS256 签名组装一个结构合法的 JWT。Codex 只做本地 payload
/// 解码，不验签，因此随机密钥足够；保持真实 JWT 的三段结构即可。
fn build_hs256_jwt(claims: &Value) -> Result<String, AppError> {
    let header = json!({ "alg": "HS256", "typ": "JWT" });

    let encode = |value: &Value| -> Result<String, AppError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| AppError::Message(format!("序列化 JWT claims 失败: {e}")))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    };

    let header_b64 = encode(&header)?;
    let payload_b64 = encode(claims)?;

    let mut key = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut key);
    let mut mac = HmacSha256::new_from_slice(&key)
        .map_err(|e| AppError::Message(format!("初始化 HMAC 失败: {e}")))?;
    mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
    let signature = mac.finalize().into_bytes();

    Ok(format!(
        "{header_b64}.{payload_b64}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// 生成一份完整的合成 ChatGPT 会话（auth.json 内容）。
pub fn generate_synthetic_codex_auth() -> Result<Value, AppError> {
    let now = chrono::Utc::now();
    let iat = now.timestamp();
    let exp = iat + SYNTHETIC_TOKEN_TTL_SECS;

    let user_id = uuid::Uuid::new_v4().to_string();
    let account_id = uuid::Uuid::new_v4().to_string();

    let auth_claim = json!({
        "chatgpt_account_id": account_id,
        "user_id": user_id,
    });

    let id_token = build_hs256_jwt(&json!({
        "sub": user_id,
        "email": SYNTHETIC_EMAIL,
        "email_verified": true,
        "name": "CC Switch Local",
        "iat": iat,
        "exp": exp,
        "auth_time": iat,
        "https://api.openai.com/auth": auth_claim,
    }))?;

    let access_token = build_hs256_jwt(&json!({
        "sub": user_id,
        "iat": iat,
        "exp": exp,
        "scope": "openid profile email offline_access",
        "https://api.openai.com/auth": auth_claim,
    }))?;

    Ok(json!({
        "OPENAI_API_KEY": null,
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": id_token,
            "access_token": access_token,
            "refresh_token": format!("ccs_{}", uuid::Uuid::new_v4()),
            "account_id": account_id,
        },
        "last_refresh": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }))
}

/// 解码 JWT payload（不验签），解析失败返回 None。
fn decode_jwt_payload(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// 判断一份 auth.json 内容是否是本模块生成的合成会话。
/// 任何字段缺失或解析失败都返回 false——绝不误伤真实登录。
pub fn is_synthetic_codex_auth(auth: &Value) -> bool {
    auth.get("tokens")
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(|token| token.as_str())
        .and_then(decode_jwt_payload)
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|email| email.as_str().map(str::to_string))
        .is_some_and(|email| email == SYNTHETIC_EMAIL)
}

/// 当前 live `auth.json` 是否是合成会话。文件缺失或不可读时返回 false。
pub fn live_synthetic_login_active() -> bool {
    let auth_path = crate::codex_config::get_codex_auth_path();
    if !auth_path.exists() {
        return false;
    }
    read_json_file(&auth_path)
        .map(|auth| is_synthetic_codex_auth(&auth))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_auth_is_decodable_and_detected() {
        let auth = generate_synthetic_codex_auth().expect("generate synthetic auth");

        assert_eq!(auth["auth_mode"], "chatgpt");
        assert!(auth["OPENAI_API_KEY"].is_null());
        assert!(auth["tokens"]["account_id"].as_str().is_some());
        assert!(auth["tokens"]["refresh_token"]
            .as_str()
            .is_some_and(|t| t.starts_with("ccs_")));
        assert!(auth["last_refresh"].as_str().is_some());

        for token in ["id_token", "access_token"] {
            let jwt = auth["tokens"][token].as_str().expect("token string");
            let payload = decode_jwt_payload(jwt).expect("decodable jwt payload");
            let exp = payload["exp"].as_i64().expect("exp claim");
            let iat = payload["iat"].as_i64().expect("iat claim");
            assert_eq!(exp - iat, SYNTHETIC_TOKEN_TTL_SECS);
        }

        let id_payload = decode_jwt_payload(auth["tokens"]["id_token"].as_str().unwrap()).unwrap();
        assert_eq!(id_payload["email"], SYNTHETIC_EMAIL);
        assert!(
            id_payload["https://api.openai.com/auth"]["chatgpt_account_id"]
                .as_str()
                .is_some()
        );

        assert!(is_synthetic_codex_auth(&auth));
    }

    #[test]
    fn synthetic_detection_rejects_real_and_broken_shapes() {
        // 真实登录形态：email 不是标记
        let real = json!({
            "auth_mode": "chatgpt",
            "tokens": { "id_token": "aaa.bbb.ccc" }
        });
        assert!(!is_synthetic_codex_auth(&real));

        // API key 形态
        assert!(!is_synthetic_codex_auth(
            &json!({ "OPENAI_API_KEY": "sk-x" })
        ));
        // 空 / 缺字段 / 非对象
        assert!(!is_synthetic_codex_auth(&json!({})));
        assert!(!is_synthetic_codex_auth(&json!({ "tokens": {} })));
        assert!(!is_synthetic_codex_auth(&Value::Null));
    }
}
