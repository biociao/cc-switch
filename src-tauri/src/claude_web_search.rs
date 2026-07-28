//! Claude 供应商 web_search 兼容层（与 codex_config.rs 的 web_search 机制对应）。
//!
//! 背景：用户通过第三方中转渠道（new-api / one-api 等）使用 Claude Code 时，
//! 渠道不支持或"假支持" WebSearch 工具——把联网搜索结果以
//! "Search results for query:" 纯文本注入对话，造成冗余。对这类渠道，cc-switch
//! 在写入 live 配置（~/.claude/settings.json）时注入
//! `permissions.deny: ["WebSearch"]`，让 Claude Code 在本地直接禁用 WebSearch；
//! 支持的渠道保持不变。

use serde_json::{json, Value};

use crate::provider::Provider;

/// 注入到 `permissions.deny` 的规则：禁用 Claude Code 内置 WebSearch 工具。
/// 同时充当 cc-switch 的归属哨兵——剥离逻辑只移除这条规则，且仅当供应商
/// 自己的存储配置里没有它时才移除（见 `strip_injected_web_search_deny`）。
pub(crate) const CLAUDE_WEB_SEARCH_DENY_RULE: &str = "WebSearch";

/// `meta.webSearchCompat` 的显式取值："enabled" 强制允许 / "disabled" 强制禁用；
/// 缺省或 "auto" 走下方黑名单判定。
pub(crate) const CLAUDE_WEB_SEARCH_COMPAT_ENABLED: &str = "enabled";
pub(crate) const CLAUDE_WEB_SEARCH_COMPAT_DISABLED: &str = "disabled";

/// 已知不支持 / 强制文本注入 web_search 的渠道 host 黑名单（按
/// `env.ANTHROPIC_BASE_URL` 子串匹配）。
///
/// 这是 BLACKLIST（默认放行）：未列出的渠道一律保持 Claude Code 默认行为。
/// fail-safe 方向是有意的——漏判的后果只是该渠道用户手动把 webSearchCompat
/// 设为 "disabled"（可自愈的小不便），误判的后果则是剥夺了本来可用的联网
/// 搜索（功能性损失），所以宁可漏判、不可误判。
///
/// 目前为空：**不要编造"已验证"的条目**。发现一例（用户反馈 + 复现确认）
/// 补一例，并在条目后注释渠道名与确认日期。
pub(crate) const CLAUDE_WEB_SEARCH_REJECT_HOSTS: &[&str] = &[];

/// 已知有问题的模型 id 前缀黑名单，匹配 `env.ANTHROPIC_MODEL` 的最后一个
/// `/` 分段（与 host 黑名单同理，目前为空，发现一例补一例）。
pub(crate) const CLAUDE_WEB_SEARCH_REJECT_MODEL_PREFIXES: &[&str] = &[];

/// 判定一个 Claude 供应商写入 live 时是否应注入 `permissions.deny: ["WebSearch"]`。
///
/// 优先级：
/// 1. `meta.webSearchCompat` 显式设置（"disabled" → true，"enabled" → false）；
/// 2. auto / 缺省：按 `env.ANTHROPIC_BASE_URL` 的 host 子串、或
///    `env.ANTHROPIC_MODEL` 最后一段（`/` 分隔）的前缀命中内置黑名单。
pub(crate) fn claude_provider_denies_web_search(provider: &Provider) -> bool {
    // 优先级 1：用户显式设置覆盖一切自动判定
    if let Some(compat) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.web_search_compat.as_deref())
    {
        match compat {
            CLAUDE_WEB_SEARCH_COMPAT_DISABLED => return true,
            CLAUDE_WEB_SEARCH_COMPAT_ENABLED => return false,
            _ => {} // "auto" 或未知值：回落黑名单判定
        }
    }

    // 优先级 2（auto）：host / 模型前缀黑名单
    let env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object);

    if let Some(base_url) = env
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(Value::as_str)
    {
        let base_url = base_url.to_ascii_lowercase();
        if CLAUDE_WEB_SEARCH_REJECT_HOSTS
            .iter()
            .any(|host| base_url.contains(host))
        {
            return true;
        }
    }

    if let Some(model) = env
        .and_then(|env| env.get("ANTHROPIC_MODEL"))
        .and_then(Value::as_str)
    {
        let model = model.to_ascii_lowercase();
        // 去掉聚合渠道的 "vendor/" 前缀，如 "openai/claude-sonnet-4"
        let model = model.rsplit('/').next().unwrap_or(model.as_str());
        if CLAUDE_WEB_SEARCH_REJECT_MODEL_PREFIXES
            .iter()
            .any(|prefix| model.starts_with(prefix))
        {
            return true;
        }
    }

    false
}

/// 写入 live 配置时按供应商策略注入 WebSearch deny 规则。
///
/// 仅在 `claude_provider_denies_web_search` 为 true 时往
/// `settings["permissions"]["deny"]`（不存在则创建）追加 "WebSearch"；
/// 已存在则不重复追加。`permissions` / `deny` 已存在但类型不符时放弃注入，
/// 不覆盖用户配置。
///
/// ownership 语义：注入只发生在写 live 这一刻，供应商存储配置与通用配置
/// 片段中都不含这条规则；从 live 读回（切换回填 / 提取共享片段）时由
/// `strip_injected_web_search_deny` 对称剥离，保证注入产物永不停留进存储。
pub(crate) fn apply_claude_web_search_policy(settings: &mut Value, provider: &Provider) {
    if !claude_provider_denies_web_search(provider) {
        return;
    }

    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let permissions = obj
        .entry("permissions")
        .or_insert_with(|| json!({}));
    let Some(permissions) = permissions.as_object_mut() else {
        return;
    };
    let deny = permissions.entry("deny").or_insert_with(|| json!([]));
    let Some(deny) = deny.as_array_mut() else {
        return;
    };

    if !deny
        .iter()
        .any(|rule| rule.as_str() == Some(CLAUDE_WEB_SEARCH_DENY_RULE))
    {
        deny.push(Value::String(CLAUDE_WEB_SEARCH_DENY_RULE.to_string()));
    }
}

/// 从一份 live settings 中剥离 cc-switch 注入的 WebSearch deny 规则。
///
/// 仅当两个条件同时满足才剥离：
/// 1. `claude_provider_denies_web_search(provider)` 为 true（即写 live 时确实会注入）；
/// 2. provider 自己存储的 `settings_config.permissions.deny` 里没有 "WebSearch"
///    （否则这条规则归用户所有，永不被误删）。
///
/// 剥离后做空对象清理：`deny` 数组空了则移除 `deny` 键，`permissions` 空了
/// 则移除 `permissions` 键，避免回填留下 `"permissions": {}` 残渣。
pub(crate) fn strip_injected_web_search_deny(settings: &mut Value, provider: &Provider) {
    if !claude_provider_denies_web_search(provider) {
        return;
    }

    // 用户手写的 deny 归用户所有：存储配置里已有 WebSearch 时，live 里这条
    // 视为用户配置而非注入产物，不剥。
    let user_owned = provider
        .settings_config
        .get("permissions")
        .and_then(Value::as_object)
        .and_then(|permissions| permissions.get("deny"))
        .and_then(Value::as_array)
        .is_some_and(|deny| {
            deny.iter()
                .any(|rule| rule.as_str() == Some(CLAUDE_WEB_SEARCH_DENY_RULE))
        });
    if user_owned {
        return;
    }

    let Some(permissions) = settings
        .get_mut("permissions")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    let mut deny_emptied = false;
    if let Some(deny) = permissions.get_mut("deny").and_then(Value::as_array_mut) {
        deny.retain(|rule| rule.as_str() != Some(CLAUDE_WEB_SEARCH_DENY_RULE));
        deny_emptied = deny.is_empty();
    }
    if deny_emptied {
        permissions.remove("deny");
    }
    if permissions.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("permissions");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;

    fn claude_provider(settings: Value, web_search_compat: Option<&str>) -> Provider {
        let mut provider = Provider::with_id(
            "p1".into(),
            "Claude A".into(),
            settings,
            None,
        );
        provider.meta = web_search_compat.map(|compat| ProviderMeta {
            web_search_compat: Some(compat.to_string()),
            ..ProviderMeta::default()
        });
        provider
    }

    fn basic_settings() -> Value {
        json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "token",
                "ANTHROPIC_BASE_URL": "https://claude.example",
                "ANTHROPIC_MODEL": "claude-sonnet-4"
            }
        })
    }

    #[test]
    fn denies_web_search_meta_override_wins() {
        // meta disabled → true；enabled → false；None / auto → 空黑名单下 false
        let provider = claude_provider(basic_settings(), Some("disabled"));
        assert!(claude_provider_denies_web_search(&provider));

        let provider = claude_provider(basic_settings(), Some("enabled"));
        assert!(!claude_provider_denies_web_search(&provider));

        let provider = claude_provider(basic_settings(), Some("auto"));
        assert!(!claude_provider_denies_web_search(&provider));

        let provider = claude_provider(basic_settings(), None);
        assert!(!claude_provider_denies_web_search(&provider));
    }

    #[test]
    fn denies_web_search_empty_blacklists_default_to_allow() {
        // 黑名单为空时，任意 host / 模型都不会命中（fail-safe：宁可漏判）
        let settings = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://api.newapi.example/v1",
                "ANTHROPIC_MODEL": "somevendor/claude-opus-4"
            }
        });
        let provider = claude_provider(settings, None);
        assert!(!claude_provider_denies_web_search(&provider));
    }

    #[test]
    fn apply_policy_creates_deny_when_missing() {
        let provider = claude_provider(basic_settings(), Some("disabled"));
        let mut settings = basic_settings();
        apply_claude_web_search_policy(&mut settings, &provider);
        assert_eq!(
            settings["permissions"]["deny"],
            json!([CLAUDE_WEB_SEARCH_DENY_RULE])
        );
        // env 等其它字段不受影响
        assert_eq!(settings["env"]["ANTHROPIC_AUTH_TOKEN"], json!("token"));
    }

    #[test]
    fn apply_policy_appends_to_existing_deny() {
        let provider = claude_provider(basic_settings(), Some("disabled"));
        let mut settings = json!({
            "permissions": { "deny": ["Bash(rm *)"], "allow": ["Read"] }
        });
        apply_claude_web_search_policy(&mut settings, &provider);
        assert_eq!(
            settings["permissions"]["deny"],
            json!(["Bash(rm *)", CLAUDE_WEB_SEARCH_DENY_RULE])
        );
        assert_eq!(settings["permissions"]["allow"], json!(["Read"]));
    }

    #[test]
    fn apply_policy_does_not_duplicate_rule() {
        let provider = claude_provider(basic_settings(), Some("disabled"));
        let mut settings = json!({
            "permissions": { "deny": ["WebSearch"] }
        });
        apply_claude_web_search_policy(&mut settings, &provider);
        assert_eq!(settings["permissions"]["deny"], json!(["WebSearch"]));
    }

    #[test]
    fn apply_policy_noop_when_allowed() {
        let provider = claude_provider(basic_settings(), Some("enabled"));
        let mut settings = basic_settings();
        apply_claude_web_search_policy(&mut settings, &provider);
        assert!(settings.get("permissions").is_none());
    }

    #[test]
    fn strip_removes_injected_rule_and_cleans_empty_objects() {
        let provider = claude_provider(basic_settings(), Some("disabled"));
        let mut live = json!({
            "env": { "ANTHROPIC_AUTH_TOKEN": "token" },
            "permissions": { "deny": ["WebSearch"] }
        });
        strip_injected_web_search_deny(&mut live, &provider);
        assert!(live.get("permissions").is_none(), "got: {live}");
        assert_eq!(live["env"]["ANTHROPIC_AUTH_TOKEN"], json!("token"));
    }

    #[test]
    fn strip_keeps_other_deny_rules() {
        let provider = claude_provider(basic_settings(), Some("disabled"));
        let mut live = json!({
            "permissions": { "deny": ["WebSearch", "Bash(rm *)"] }
        });
        strip_injected_web_search_deny(&mut live, &provider);
        assert_eq!(live["permissions"]["deny"], json!(["Bash(rm *)"]));
    }

    #[test]
    fn strip_keeps_user_owned_rule() {
        // provider 存储配置里本来就有 WebSearch deny：live 里这条归用户所有，不剥
        let mut settings = basic_settings();
        settings["permissions"] = json!({ "deny": ["WebSearch"] });
        let provider = claude_provider(settings, Some("disabled"));

        let mut live = json!({
            "permissions": { "deny": ["WebSearch", "Bash(rm *)"] }
        });
        strip_injected_web_search_deny(&mut live, &provider);
        assert_eq!(
            live["permissions"]["deny"],
            json!(["WebSearch", "Bash(rm *)"])
        );
    }

    #[test]
    fn strip_noop_when_provider_allows_web_search() {
        let provider = claude_provider(basic_settings(), Some("enabled"));
        let mut live = json!({
            "permissions": { "deny": ["WebSearch"] }
        });
        strip_injected_web_search_deny(&mut live, &provider);
        assert_eq!(live["permissions"]["deny"], json!(["WebSearch"]));
    }
}
