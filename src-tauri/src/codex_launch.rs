//! 启动/重启 Codex 桌面端。
//!
//! 托盘"选择模型并启动"流程的第二步：模型写入 `config.toml` 后，需要
//! 重启桌面端才能生效（配置只在启动时读取）。macOS 上先优雅退出再
//! `open`；其他平台暂未实现自动重启，由调用方提示用户手动重启。

use crate::error::AppError;

/// Codex 桌面端的 macOS bundle id。
const CODEX_BUNDLE_ID: &str = "com.openai.codex";

#[derive(Debug, Clone, Copy, Default)]
pub struct CodexLaunchOutcome {
    /// 是否先等待了既有实例退出（Ok 即代表已执行启动）
    pub waited_quit: bool,
}

/// 重启 Codex 桌面端：优雅退出运行中的实例（最多等待 5 秒），然后重新启动。
/// 未运行时直接启动。
#[cfg(target_os = "macos")]
pub fn relaunch_codex_app() -> Result<CodexLaunchOutcome, AppError> {
    use std::process::Command;
    use std::time::{Duration, Instant};

    let was_running = codex_app_running();

    if was_running {
        // 优雅退出；未运行/已退出等错误忽略。
        let _ = Command::new("osascript")
            .args(["-e", &format!("quit app id \"{CODEX_BUNDLE_ID}\"")])
            .output();

        let deadline = Instant::now() + Duration::from_secs(5);
        while codex_app_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        if codex_app_running() {
            log::warn!("等待 Codex 退出超时，仍尝试直接启动");
        }
    }

    let opened = Command::new("open")
        .args(["-b", CODEX_BUNDLE_ID])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    let relaunched = if opened {
        true
    } else {
        // 回退：按应用名查找（用户可能改了安装位置/名称）。
        Command::new("open")
            .args(["-a", "Codex"])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };

    if !relaunched {
        return Err(AppError::localized(
            "codex.launch.not_found",
            "未找到 Codex 桌面应用，请手动启动",
            "Codex desktop app not found; please launch it manually",
        ));
    }

    Ok(CodexLaunchOutcome {
        waited_quit: was_running,
    })
}

#[cfg(target_os = "macos")]
fn codex_app_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "Codex"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 非 macOS 平台：暂未实现自动重启。
#[cfg(not(target_os = "macos"))]
pub fn relaunch_codex_app() -> Result<CodexLaunchOutcome, AppError> {
    Err(AppError::localized(
        "codex.launch.unsupported",
        "当前平台暂不支持自动重启，请手动重启 Codex",
        "Auto-relaunch is not supported on this platform; please restart Codex manually",
    ))
}
