//! 开发者选项与 "Linux 开发环境" 开关的处理。
//!
//! 关键事实（AOSP packages/apps/Settings .../development/linuxterminal/）：
//!   * 开发者选项中的入口 key 为 `linux_terminal`，点击后进入 Terminal 面板；
//!   * 面板上的开关 `enable_linux_terminal` 实际执行的是
//!     `PackageManager.setApplicationEnabledSetting(terminalPkg, ENABLED)`。
//! 因此"打开 Linux 开发环境"在系统层面等价于 `pm enable com.android.virtualization.terminal`，
//! 可以直接执行，无需模拟点击，也就不受机型/语言/安全弹窗影响。

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::adb::Adb;
use crate::detect::TERMINAL_PKG;
use crate::uia::{self, UiNode};

/// 需要用户手工处理的安全类问题。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityIssue {
    /// `usb_security` | `enterprise_lock` | `security_folder` | `dialog` | `unknown`
    pub kind: String,
    pub title: String,
    pub detail: String,
    /// 是否可以由工具自动点击解决。
    pub auto_clickable: bool,
}

/// 开发者选项状态报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevOptionReport {
    pub dev_options_enabled: bool,
    pub adb_enabled: bool,
    /// Linux Terminal 应用是否已启用（即"Linux 开发环境"开关是否打开）。
    pub linux_env_enabled: bool,
    pub terminal_pkg_present: bool,
    /// 开发者选项页面里是否能看到 "Linux development environment / Linux 开发环境" 条目。
    pub linux_entry_visible: bool,
    pub linux_entry_label: String,
    /// 开发者选项里该开关的实际状态（找到条目时才有值）。
    pub linux_switch_checked: Option<bool>,
    pub is_pixel: bool,
    pub device_policy_present: bool,
    pub device_policy_detail: String,
    pub security_issue: Option<SecurityIssue>,
    pub notes: Vec<String>,
}

async fn settings_get(adb: &Adb, namespace: &str, key: &str) -> String {
    adb.shell(&format!("settings get {} {}", namespace, key))
        .await
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 读取 Terminal 应用的启用状态。
pub async fn terminal_enabled(adb: &Adb) -> (bool, bool) {
    let present = adb
        .shell(&format!("pm list packages {}", TERMINAL_PKG))
        .await
        .unwrap_or_default()
        .contains(TERMINAL_PKG);
    let disabled = adb
        .shell(&format!("pm list packages -d {}", TERMINAL_PKG))
        .await
        .unwrap_or_default()
        .contains(TERMINAL_PKG);
    (present, present && !disabled)
}

/// 打开系统开发者选项页面。
pub async fn open_dev_options(adb: &Adb) -> Result<()> {
    match adb.start_activity("com.android.settings/.development.DevelopmentSettings").await {
        Ok(()) => Ok(()),
        Err(_) => {
            adb.raw(&["shell", "am start -a android.settings.APPLICATION_DEVELOPMENT_SETTINGS"])
                .await
                .map(|_| ())
        }
    }
}

/// 启用 Linux 开发环境（等价于打开开发者选项里的开关）。
pub async fn enable_linux_env(adb: &Adb) -> Result<String> {
    let out = adb
        .raw(&["shell", &format!("pm enable {}", TERMINAL_PKG)])
        .await?;
    let text = out.combined();
    if text.contains("new state: enabled") || text.contains("already enabled") {
        return Ok("已通过系统接口打开 Linux 开发环境。".to_string());
    }
    // 兜底：直接点开发者选项里的开关
    if toggle_linux_env_via_ui(adb).await? {
        return Ok("已在开发者选项中打开「Linux 开发环境」开关。".to_string());
    }
    Err(anyhow!("启用失败: {}", text.trim()))
}

/// 关闭 Android 的「幽灵进程」限制。
///
/// Android 12+ 会在后台进程数超过 `max_phantom_processes` 时直接杀掉子进程，
/// AVF 虚拟机里正在执行的安装命令经常因此被中断。把上限调到最大并关闭监控，
/// 虚拟机就不会被系统在后台回收。
///
/// 部分设备不允许 shell 修改这些配置，失败时只记录提示，不阻断主流程。
pub async fn disable_phantom_killer(adb: &Adb) -> (bool, String) {
    let mut failed = Vec::new();
    let commands = [
        "device_config put activity_manager max_phantom_processes 2147483647",
        "settings put global settings_enable_monitor_phantom_procs false",
    ];
    for cmd in commands {
        match adb.raw(&["shell", cmd]).await {
            Ok(out) if out.ok() => {}
            _ => failed.push(cmd),
        }
    }
    if failed.is_empty() {
        (
            true,
            "已关闭幽灵进程限制（max_phantom_processes=2147483647，settings_enable_monitor_phantom_procs=false）。"
                .to_string(),
        )
    } else {
        (
            false,
            format!(
                "以下幽灵进程限制设置执行失败（设备可能禁止 shell 修改，可手动执行）：\n{}",
                failed.join("\n")
            ),
        )
    }
}

/// 检测企业锁（设备所有者 / 配置所有者）。
pub async fn device_policy(adb: &Adb) -> (bool, String) {
    let out = adb
        .shell("dpm list-owners")
        .await
        .unwrap_or_default();
    // 「没有任何所有者」的文案各 ROM 不一致：AOSP 是 "No owners."，
    // MIUI / HyperOS 是 "no owners"，还有的版本带附加说明。
    // 按大小写敏感匹配会漏掉小写版本，小米平板就被误判成"企业管理设备"。
    if out.to_lowercase().contains("no owner") {
        return (false, String::new());
    }
    let meaningful = out
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        // 真正的所有者条目一定带包名或组件名，借此过滤表头和命令报错
        .filter(|l| l.contains('.') || l.contains("ComponentInfo"))
        .filter(|l| {
            let l = l.to_lowercase();
            !l.starts_with("error") && !l.contains("exception") && !l.contains("not found")
        })
        .collect::<Vec<_>>();
    let present = !meaningful.is_empty();
    (present, meaningful.join(" | "))
}

/// 在当前界面上寻找需要人工介入的安全弹窗。
pub fn scan_security_issue(nodes: &[UiNode]) -> Option<SecurityIssue> {
    // 企业锁 / 策略限制
    for n in nodes {
        let t = format!("{} {}", n.text, n.desc);
        if (t.contains("企业") || t.contains("策略") || t.contains("Device administrator")
            || t.contains("device admin"))
            && (t.contains("无法") || t.contains("blocked") || t.contains("阻止")
                || t.contains("disallow") || t.contains("restricted"))
        {
            return Some(SecurityIssue {
                kind: "enterprise_lock".to_string(),
                title: "设备受企业策略管控".to_string(),
                detail: format!(
                    "系统提示当前设备被企业策略/设备所有者锁定，无法自动开启开发者选项。\n请先在手机上移除企业管理（设备所有者 / 工作资料），再重新执行本步骤。\n\n界面原文：{}",
                    t.trim()
                ),
                auto_clickable: false,
            });
        }
    }

    // 三星 Knox / 安全文件夹
    for n in nodes {
        let t = format!("{} {}", n.text, n.desc);
        if (t.contains("Knox") || t.contains("安全文件夹") || t.contains("Secure Folder"))
            && (t.contains("允许") || t.contains("Allow") || t.contains("确认"))
        {
            return Some(SecurityIssue {
                kind: "security_folder".to_string(),
                title: "三星设备安全提示".to_string(),
                detail: format!(
                    "检测到 Knox / 安全文件夹相关授权弹窗。\n请在手机上点击“允许 / Allow”完成授权（或临时关闭安全文件夹与双录保护），再点击“重试”。\n\n界面原文：{}",
                    t.trim()
                ),
                auto_clickable: true,
            });
        }
    }

    // USB 安全调试
    if let Some(n) = nodes
        .iter()
        .find(|n| n.mentions("USB 安全调试") || n.mentions("安全调试") || n.mentions("Security settings"))
    {
        let t = format!("{} {}", n.text, n.desc);
        return Some(SecurityIssue {
            kind: "usb_security".to_string(),
            title: "需要开启 USB 安全调试".to_string(),
            detail: format!(
                "当前机型（非 Pixel）在开发者选项中要求额外开启【USB 安全调试 / USB debugging (Security settings)】。\n请在手机：设置 → 开发者选项 → 打开“USB 安全调试”，然后点击“重试”。\n\n界面原文：{}",
                t.trim()
            ),
            auto_clickable: false,
        });
    }

    // 通用授权弹窗（可自动点击“允许 / Allow”）
    for label in ["允许", "Allow", "确定", "OK"] {
        if let Some(node) = nodes
            .iter()
            .find(|n| n.clickable && n.text.trim().eq_ignore_ascii_case(label))
        {
            return Some(SecurityIssue {
                kind: "dialog".to_string(),
                title: "系统授权确认".to_string(),
                detail: format!("检测到系统授权弹窗：{}", node.text.trim()),
                auto_clickable: true,
            });
        }
    }

    None
}

/// 自动点击界面上的“允许 / Allow / 确定”按钮（用于安全确认弹窗）。
pub async fn click_allow(adb: &Adb, nodes: &[UiNode]) -> Result<bool> {
    for label in ["允许", "Allow", "ALLOW", "确定", "OK", "继续", "Continue"] {
        if let Some(node) = nodes
            .iter()
            .find(|n| n.clickable && n.text.trim() == label)
        {
            uia::tap_node(adb, node).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// 生成开发者选项状态报告。
pub async fn report(adb: &Adb) -> Result<DevOptionReport> {
    let dev_options_enabled = settings_get(adb, "global", "development_settings_enabled").await == "1";
    let adb_enabled = settings_get(adb, "global", "adb_enabled").await == "1";
    let (present, enabled) = terminal_enabled(adb).await;
    let (policy_present, policy_detail) = device_policy(adb).await;
    let model = adb
        .shell("getprop ro.product.model")
        .await
        .unwrap_or_default()
        .trim()
        .to_string();
    let is_pixel = model.to_lowercase().contains("pixel");

    let mut notes = Vec::new();
    if !dev_options_enabled {
        notes.push("开发者选项尚未开启（需在「关于手机」连点 7 次版本号）。".to_string());
    }
    if policy_present {
        notes.push(format!("检测到设备策略所有者：{}", policy_detail));
    }
    if !is_pixel {
        notes.push("当前为非 Pixel 机型，可能出现 Knox / USB 安全调试等额外授权步骤。".to_string());
    }

    Ok(DevOptionReport {
        dev_options_enabled,
        adb_enabled,
        linux_env_enabled: enabled,
        terminal_pkg_present: present,
        linux_entry_visible: false,
        linux_entry_label: String::new(),
        linux_switch_checked: None,
        is_pixel,
        device_policy_present: policy_present,
        device_policy_detail: policy_detail,
        security_issue: None,
        notes,
    })
}

/// 找到条目标题/副标题节点。
fn find_linux_entry_node(nodes: &[UiNode]) -> Option<&UiNode> {
    nodes.iter().find(|n| {
        (n.id.ends_with("android:id/title") || n.id.ends_with("android:id/summary"))
            && n.text.to_lowercase().contains("linux")
    })
}

/// 找到与该条目同一行的开关控件。
fn row_switch<'a>(nodes: &'a [UiNode], title: &UiNode) -> Option<&'a UiNode> {
    let mut best: Option<(&'a UiNode, i64)> = None;
    for n in nodes {
        if !(n.id.ends_with("switchWidget") || n.id.ends_with("android:id/switch_widget")) {
            continue;
        }
        let dy = (n.bounds.center().1 - title.bounds.center().1).abs();
        if dy <= 70 && best.map(|(_, d)| dy < d).unwrap_or(true) {
            best = Some((n, dy));
        }
    }
    best.map(|(n, _)| n)
}

/// 打开开发者选项并核对 "Linux 开发环境" 开关的实际状态。
///
/// 返回 (是否找到条目, 条目标题, 开关是否处于打开状态)。
pub async fn verify_linux_entry(adb: &Adb) -> Result<(bool, String, Option<bool>)> {
    open_dev_options(adb).await?;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    for _ in 0..14 {
        let nodes = uia::dump(adb).await?;
        if let Some(t) = find_linux_entry_node(&nodes) {
            let checked = row_switch(&nodes, t).map(|s| s.checked);
            return Ok((true, t.text.clone(), checked));
        }
        uia::scroll_down(adb).await?;
        tokio::time::sleep(std::time::Duration::from_millis(650)).await;
    }
    Ok((false, String::new(), None))
}

/// 兜底方案：直接点开发者选项里的开关（当 pm enable 不可用时）。
pub async fn toggle_linux_env_via_ui(adb: &Adb) -> Result<bool> {
    open_dev_options(adb).await?;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    for _ in 0..14 {
        let nodes = uia::dump(adb).await?;
        if let Some(t) = find_linux_entry_node(&nodes) {
            if let Some(sw) = row_switch(&nodes, t) {
                if !sw.checked {
                    uia::tap_node(adb, sw).await?;
                    return Ok(true);
                }
                return Ok(false);
            }
        }
        uia::scroll_down(adb).await?;
        tokio::time::sleep(std::time::Duration::from_millis(650)).await;
    }
    Ok(false)
}
