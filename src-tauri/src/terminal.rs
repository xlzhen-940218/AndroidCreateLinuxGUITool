//! 控制 Android 上的 Linux Terminal 应用（com.android.virtualization.terminal）。
//!
//! 交互方式：
//!   * 通过 `am start` 启动应用；
//!   * 通过 uiautomator 节点树判断当前处于「安装系统镜像 / 启动虚拟机 / 就绪」哪个阶段；
//!   * 通过 `input tap` 聚焦终端 WebView，再用 `input text` + `input keyevent 66` 执行命令。
//! 读取命令输出不走终端屏幕，而是由虚拟机内的脚本写入共享存储日志（见 installer.rs）。

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::time::Duration;

use crate::adb::Adb;
use crate::uia::{self, UiNode};

pub const TERMINAL_MAIN_ACTIVITY: &str =
    "com.android.virtualization.terminal/com.android.virtualization.terminal.MainActivity";
/// Android 17 起 Terminal 换成 new2 包结构（Activity 路径不同）。
pub const TERMINAL_MAIN_ACTIVITY_NEW2: &str =
    "com.android.virtualization.terminal/.new2.ui.MainActivity";

/// 动态解析 Terminal 应用的启动 Activity。
///
/// 不同系统版本里 Activity 路径不同（main 分支是 `.MainActivity`，
/// Android 17 的 new2 包是 `.new2.ui.MainActivity`），因此不能写死。
pub async fn resolve_launch_activity(adb: &Adb) -> Result<String> {
    let out = adb
        .shell(&format!(
            "cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.LAUNCHER {}",
            crate::detect::TERMINAL_PKG
        ))
        .await
        .unwrap_or_default();
    for line in out.lines() {
        let line = line.trim();
        if line.starts_with(crate::detect::TERMINAL_PKG) && line.contains('/') {
            // `com.android.virtualization.terminal/.new2.ui.MainActivity`
            // → 保留 `/.new2.ui.MainActivity` 这一段，去掉中间的斜杠即可
            let short = line[crate::detect::TERMINAL_PKG.len()..].trim_start_matches('/');
            return Ok(format!("{}/{}", crate::detect::TERMINAL_PKG, short));
        }
    }
    // 解析失败时依次尝试已知的两种路径
    Ok(TERMINAL_MAIN_ACTIVITY_NEW2.to_string())
}

/// 终端应用当前所处阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminalPhase {
    /// 应用未安装。
    NotInstalled,
    /// 显示「安装 Linux 终端」引导页，等待用户确认下载镜像。
    NeedsInstall,
    /// 正在安装系统镜像。
    Installing,
    /// 虚拟机正在启动。
    Booting,
    /// 终端已就绪，可以输入命令。
    Ready,
    /// 状态未知。
    Unknown,
}

impl TerminalPhase {
    pub fn label(&self) -> &'static str {
        match self {
            TerminalPhase::NotInstalled => "未安装 Terminal 应用",
            TerminalPhase::NeedsInstall => "等待安装 Linux 终端系统镜像",
            TerminalPhase::Installing => "正在安装系统镜像",
            TerminalPhase::Booting => "正在启动 Linux 虚拟机",
            TerminalPhase::Ready => "终端已就绪",
            TerminalPhase::Unknown => "状态未知",
        }
    }
}

/// 通过节点树判断当前阶段。
///
/// 兼容两种界面形态：
///   * 旧版（main 分支）：有稳定 resource-id，如 `installer_install_button` / `webview`；
///   * 新版（Android 17 的 new2，Compose 界面）：没有 resource-id，只能靠文本判断。
pub fn phase_from_nodes(nodes: &[UiNode]) -> TerminalPhase {
    // 安装引导页：旧版有明确的 id，新版靠「下载 XX MB / Would you like to proceed」文本
    if let Some(n) = uia::find_by_id(nodes, "installer_install_button")
        .or_else(|| uia::find_by_id(nodes, "installer_btn_install"))
    {
        return if n.enabled {
            TerminalPhase::NeedsInstall
        } else {
            TerminalPhase::Installing
        };
    }
    if uia::find_by_id(nodes, "installer_progress").is_some() {
        return TerminalPhase::Installing;
    }
    // 引导页文案随系统语言变化：英文 "Would you like to proceed?" /
    // "Download roughly ... MB"，中文 "是否要继续？" / "需要下载约 ... MB"。
    let asks_to_download = nodes.iter().any(|n| {
        let t = n.text.to_lowercase();
        t.contains("would you like to proceed")
            || t.contains("download roughly")
            || (t.contains("mb") && (t.contains("下载") || t.contains("download")))
            || t.contains("是否要继续")
            || t.contains("是否继续")
    });
    if asks_to_download {
        let install_btn = nodes.iter().find(|n| {
            let t = n.text.trim().to_lowercase();
            t == "install" || t == "安装" || t == "install now" || t == "立即安装"
        });
        return match install_btn {
            Some(n) if n.enabled => TerminalPhase::NeedsInstall,
            _ => TerminalPhase::Installing,
        };
    }

    if uia::find_by_id(nodes, "boot_progress").is_some() {
        return TerminalPhase::Booting;
    }
    // 终端本体：旧版是 WebView 节点；新版是 Compose 的终端容器
    if uia::find_by_id(nodes, "webview").is_some()
        || nodes
            .iter()
            .any(|n| n.class.contains("WebView") || n.id.to_lowercase().contains("terminal_view"))
    {
        return TerminalPhase::Ready;
    }
    // new2 界面里终端标签标题形如 "droid@debian: ~"，出现即说明 guest shell 已经起来
    if nodes.iter().any(|n| {
        let t = n.text.trim();
        t.contains('@') && (t.contains('~') || t.ends_with('$') || t.ends_with('#'))
    }) {
        return TerminalPhase::Ready;
    }
    TerminalPhase::Unknown
}

/// 查询当前阶段。
pub async fn phase(adb: &Adb) -> Result<TerminalPhase> {
    let nodes = uia::dump(adb).await?;
    Ok(phase_from_nodes(&nodes))
}

/// 启动 Terminal 应用。
pub async fn open(adb: &Adb) -> Result<()> {
    let activity = resolve_launch_activity(adb).await?;
    match adb.start_activity(&activity).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // 兜底：尝试已知的另一种路径
            for alt in [TERMINAL_MAIN_ACTIVITY_NEW2, TERMINAL_MAIN_ACTIVITY] {
                if alt == activity {
                    continue;
                }
                if adb.start_activity(alt).await.is_ok() {
                    return Ok(());
                }
            }
            Err(anyhow!(
                "无法启动 Linux Terminal 应用：{}\n请确认已在开发者选项中开启「Linux 开发环境」，并且系统已包含 Terminal 应用。",
                e
            ))
        }
    }
}

/// 点击「安装」按钮（安装 Linux 终端系统镜像）。
pub async fn tap_install(adb: &Adb) -> Result<bool> {
    let nodes = uia::dump(adb).await?;
    // 优先用 resource-id（旧版）
    if let Some(btn) = uia::find_by_id(&nodes, "installer_install_button")
        .or_else(|| uia::find_by_id(&nodes, "installer_btn_install"))
    {
        uia::tap_node(adb, btn).await?;
        return Ok(true);
    }
    // 新版 Compose 界面没有 id，按文本找（系统语言可能是中文或英文）
    for label in ["Install", "安装", "INSTALL", "Install now", "立即安装"] {
        if let Some(btn) = nodes
            .iter()
            .find(|n| n.text.trim().eq_ignore_ascii_case(label))
        {
            uia::tap_node(adb, btn).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// 点掉安装确认对话框上的「继续 / 下载 / 确定」（部分系统版本会再确认一次）。
pub async fn tap_install_confirm(adb: &Adb) -> Result<bool> {
    let nodes = uia::dump(adb).await?;
    for label in ["Continue", "Download", "OK", "继续", "下载", "确定", "确认"] {
        if let Some(btn) = nodes
            .iter()
            .find(|n| n.text.trim().eq_ignore_ascii_case(label))
        {
            uia::tap_node(adb, btn).await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// 等待终端进入就绪状态。
///
/// 返回 `Err` 表示超时；调用方应重新查询 `phase` 以区分「需要安装镜像」等情况。
pub async fn wait_ready(adb: &Adb, timeout_secs: u64) -> Result<TerminalPhase> {
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut last = TerminalPhase::Unknown;
    while std::time::Instant::now() < deadline {
        match phase(adb).await {
            Ok(p) => {
                if p == TerminalPhase::Ready {
                    return Ok(p);
                }
                if p == TerminalPhase::NotInstalled {
                    bail!("Linux Terminal 应用未安装。");
                }
                if p != last {
                    last = p;
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    Ok(last)
}

/// 预先授予 Terminal 应用「录音」与「通知」权限。
///
/// Terminal 首次启动时会弹权限框，挡住「安装 Linux」引导页；纯 UI 点击容易漏。
/// 用 `pm grant` 直接授予最可靠。未声明的权限（老版本没有 POST_NOTIFICATIONS）
/// 会报错，忽略即可。
pub async fn grant_permissions(adb: &Adb) -> Vec<String> {
    let pkg = crate::detect::TERMINAL_PKG;
    let mut granted = Vec::new();
    for perm in [
        "android.permission.RECORD_AUDIO",
        "android.permission.POST_NOTIFICATIONS",
    ] {
        if adb
            .shell(&format!("pm grant {} {}", pkg, perm))
            .await
            .is_ok()
        {
            granted.push(perm.to_string());
        }
    }
    granted
}

/// Linux 系统镜像安装/启动的进度。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageStatus {
    pub phase: TerminalPhase,
    pub ready: bool,
    pub message: String,
}

/// 「安装 Linux 系统」步骤：授权 → 打开 Terminal → 必要时点安装 → 等到终端就绪。
///
/// 这是「Linux 开发环境」与「安装桌面环境」之间的独立工作流：
/// 系统镜像（约 600 MB）下载 + 虚拟机首次启动可能要很久，单独作为一步
/// 可以让用户看清进度，也避免混在桌面安装的进度里。
pub async fn install_image(
    adb: &Adb,
    emit: &mut (dyn FnMut(ImageStatus) + Send),
) -> Result<ImageStatus> {
    let status = |phase: TerminalPhase, message: &str| ImageStatus {
        phase,
        ready: phase == TerminalPhase::Ready,
        message: message.to_string(),
    };

    emit(status(TerminalPhase::Unknown, "正在授予录音与通知权限..."));
    let granted = grant_permissions(adb).await;
    if !granted.is_empty() {
        emit(status(
            TerminalPhase::Unknown,
            &format!("已授予权限：{}", granted.join("、")),
        ));
    }

    emit(status(TerminalPhase::Unknown, "正在打开 Terminal 应用..."));
    open(adb).await?;
    // 授权已经覆盖大部分弹窗，这里再兜底点一次
    let _ = dismiss_permission_dialogs(adb).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let mut current_phase = phase(adb).await.unwrap_or(TerminalPhase::Unknown);
    let mut needs_image = current_phase == TerminalPhase::NeedsInstall;
    if needs_image {
        emit(status(
            current_phase,
            "正在安装 Linux 系统镜像（约 600 MB，请保持网络畅通）...",
        ));
        if !tap_install(adb).await? {
            bail!(
                "Terminal 显示了安装引导页，但没能找到「安装」按钮。\n\
                 请在手机上手动点击安装，等终端出现命令行提示符后重试。"
            );
        }
        tokio::time::sleep(Duration::from_millis(2000)).await;
        // 中文界面下点「安装」之后还可能弹一个「继续/下载」确认框
        let _ = tap_install_confirm(adb).await;
    }

    let timeout_secs = if needs_image { 3600 } else { 600 };
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_message = String::new();
    let mut unknown_streak = 0u32;

    while std::time::Instant::now() < deadline {
        current_phase = phase(adb).await.unwrap_or(TerminalPhase::Unknown);
        let message = match current_phase {
            TerminalPhase::Ready => "Linux 终端已就绪。",
            TerminalPhase::NeedsInstall => "等待在手机上确认下载 Linux 系统镜像...",
            TerminalPhase::Installing => {
                "正在下载并安装 Linux 系统镜像，请保持屏幕唤醒与网络畅通..."
            }
            TerminalPhase::Booting => "Linux 虚拟机正在启动...",
            TerminalPhase::NotInstalled => bail!("未安装 Linux Terminal 应用。"),
            TerminalPhase::Unknown => "正在等待 Terminal 界面...",
        };
        if message != last_message {
            emit(status(current_phase, message));
            last_message = message.to_string();
        }
        if current_phase == TerminalPhase::Ready {
            return Ok(status(current_phase, "Linux 终端已就绪，可以继续安装桌面环境。"));
        }
        if current_phase == TerminalPhase::NeedsInstall && !needs_image {
            // 用户在安装过程中取消了引导页，再点一次
            needs_image = tap_install(adb).await.unwrap_or(false);
        }
        if current_phase == TerminalPhase::Unknown {
            unknown_streak += 1;
            if unknown_streak % 10 == 0 {
                // 界面被切走（比如用户回到了桌面），重新拉回前台
                let _ = open(adb).await;
            }
        } else {
            unknown_streak = 0;
        }
        tokio::time::sleep(Duration::from_millis(2000)).await;
    }

    bail!(
        "等待 Linux 系统镜像安装超时（当前状态：{}）。\n\
         请确认手机保持联网、屏幕未休眠，安装完成后再重试。",
        current_phase.label()
    )
}

/// 读取输入法服务（InputMethodService）的状态。
pub async fn ime_dump(adb: &Adb) -> String {
    adb.shell("dumpsys input_method 2>/dev/null | grep -E 'mServedView|mInputShown|mCurMethodId'")
        .await
        .unwrap_or_default()
}

/// 判断输入法当前服务的视图是不是终端本身。
///
/// 注意：这只是一个**辅助信号**，不能作为硬条件。
/// Android 17 的 Terminal（new2，Compose 界面）内部用 WebView 承载 xterm.js，
/// 文字最终是交给 WebView 里那个隐藏 `<textarea>`（无障碍节点表现为
/// `android.widget.EditText` / content-desc="Cursor"）的。
/// 实测：点击终端区域后 `mServedView` 往往仍然是外层的 `AndroidComposeView`，
/// 但文字照样能被终端接收；而一旦没点到终端区域，文字就会凭空消失。
/// 所以真正的判据是「有没有把焦点点到终端的输入框上」。
/// 软键盘是否真的弹出来了。
///
/// 实测（Android 17 Terminal）：键盘被系统收起后，即使无障碍节点里那个隐藏的
/// `<textarea>` 仍然是 focused=true，`input text` 注入的文字也会被直接丢弃，
/// 终端毫无反应。因此这是「能否输入」的硬条件。
pub fn keyboard_shown(dump: &str) -> bool {
    dump.lines().any(|l| l.trim() == "mInputShown=true")
}

pub fn served_view_is_terminal(dump: &str) -> bool {
    for line in dump.lines() {
        let line = line.trim();
        let Some(value) = line.strip_prefix("mServedView=") else {
            continue;
        };
        if value.contains("AndroidComposeView") {
            return false;
        }
        return value.contains("virtualization.terminal")
            || value.contains("Ttyd")
            || value.contains("android.webkit.WebView");
    }
    false
}

/// 找到终端 WebView（xterm.js 承载层）所在的节点。
///
/// 优先用 content-desc="Terminal display" 的节点，它是 new2 版本里最稳定的标识；
/// 其次退回到 resource-id 含 webview 的节点，最后退回任意高度够大的 WebView。
fn terminal_webview(nodes: &[UiNode]) -> Option<&UiNode> {
    nodes
        .iter()
        .find(|n| n.desc.contains("Terminal display"))
        .or_else(|| nodes.iter().find(|n| n.id.ends_with("webview")))
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.class.contains("WebView") && n.bounds.height() > 120)
        })
}

/// 点击终端区域使其获得输入焦点。
///
/// 做法：找到终端 WebView 的位置，在它内部依次点击若干候选点，
/// 优先点击最靠近提示符的那一行（xterm.js 最容易在那里拿到焦点），
/// 并以「软键盘是否真的弹出」作为成功判据。
///
/// 重要：Android 17 的 Terminal 里 `mServedView` 并不是可靠判据
/// （点击终端后它依然可能是 `AndroidComposeView`），但 `mInputShown`
/// （软键盘弹出状态）是硬条件——键盘没弹出来时注入的文字会被系统丢弃。
pub async fn focus(adb: &Adb) -> Result<()> {
    // 通知栏展开时终端会失去前台焦点，先收起来
    adb.raw(&["shell", "cmd statusbar collapse"]).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 收集候选点击点
    let mut candidates: Vec<(i64, i64)> = Vec::new();
    if let Ok(nodes) = uia::dump(adb).await {
        // 隐藏输入框（xterm.js 的 textarea）本身的位置最可靠
        for n in &nodes {
            if n.class.contains("EditText") && n.bounds.height() > 0 {
                candidates.push(n.bounds.center());
            }
        }
        if let Some(wv) = terminal_webview(&nodes) {
            let b = wv.bounds;
            let cx = b.center().0;
            // 靠近提示符的一行最容易让 xterm.js 拿到焦点
            candidates.push((cx, b.y2 - 50));
            candidates.push((cx, b.y1 + (b.height() / 4).max(8)));
            candidates.push((cx, b.center().1));
            candidates.push((cx, b.y1 + 30));
        }
    }
    // 兜底：屏幕中部偏上（实测 (w/2, 500) 是稳定命中点）
    let (w, h) = uia::display_size(adb).await.unwrap_or((1080, 2400));
    candidates.push((w / 2, 500));
    candidates.push((w / 2, h / 4));

    for (x, y) in candidates {
        uia::tap_xy(adb, x, y).await.ok();
        tokio::time::sleep(Duration::from_millis(800)).await;

        // 点击可能把前台切走（例如误点了终端里的链接打开浏览器）
        let fg = uia::foreground_package(adb).await.unwrap_or_default();
        if !fg.contains("virtualization.terminal") {
            let _ = adb.raw(&["shell", "input keyevent 4"]).await; // 返回终端
            tokio::time::sleep(Duration::from_millis(600)).await;
            continue;
        }

        // 硬条件：软键盘必须弹出。键盘没弹出来时，即使无障碍节点里那个隐藏
        // textarea 仍显示 focused=true，`input text` 注入的文字也会被系统丢弃。
        let ime = ime_dump(adb).await;
        if !keyboard_shown(&ime) {
            continue;
        }
        if served_view_is_terminal(&ime) {
            return Ok(());
        }
        // 键盘已弹出、served view 是外层 Compose 时，退回「隐藏输入框是否有焦点」判断
        match uia::dump(adb).await.map(|n| uia::has_focused_text_input(&n)) {
            Ok(Some(false)) => continue,
            _ => return Ok(()),
        }
    }

    bail!(
        "终端没有接收输入：点击终端区域后软键盘仍未弹出，自动输入会被系统丢弃。\n\
         请手动点一下手机 Terminal 的命令行区域，确认键盘弹出后点击「重试」。"
    )
}

/// 首次运行时 Terminal 可能弹出权限申请，这里自动点「允许」。
///
/// 兼容两种按钮：
///   * 系统权限框 `com.permissioncontroller:id/permission_allow_button`
///   * 应用内自定义的「允许 / Allow」
pub async fn dismiss_permission_dialogs(adb: &Adb) -> Result<bool> {
    for _ in 0..3 {
        let nodes = uia::dump(adb).await?;
        // 系统权限框
        for id in [
            "permission_allow_button",
            "permission_allow_foreground_only_button",
            "permission_allow_one_time_button",
        ] {
            if let Some(node) = uia::find_by_id(&nodes, id) {
                if node.enabled && node.clickable {
                    uia::tap_node(adb, node).await?;
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                    return Ok(true);
                }
            }
        }
        // 应用内自定义按钮（多语言）
        let mut clicked = false;
        for label in ["允许", "ALLOW", "Allow", "始终允许", "始终", "同意", "While using the app"] {
            if let Some(node) = nodes
                .iter()
                .find(|n| n.clickable && n.text.trim().eq_ignore_ascii_case(label))
            {
                uia::tap_node(adb, node).await?;
                clicked = true;
                break;
            }
        }
        if !clicked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    Ok(false)
}

/// 在终端里执行一条命令（回车确认）。
///
/// 两个关键细节：
///
/// 1. **必须先聚焦**。Android 17 的 Terminal 用 WebView 承载 xterm.js，只有当
///    输入法服务的 served view 是这个 WebView 时，文字才会被终端接收；
///    否则 `input text` 会被外层 Compose 吞掉，终端毫无反应。
/// 2. **命令末尾追加 ` #`**。终端同时会收到两条通道的文字：输入法提交的文本，
///    以及原始按键事件。这两者有竞态，偶尔会得到两份相同的命令文本
///    （`echo A` 变成 `echo Aecho A`）。行尾的 `#` 让 shell 把重复的那份
///    当成注释吃掉，从而保证命令**恰好执行一次**；没有重复时 `#` 也不影响执行。
pub async fn run_command(adb: &Adb, command: &str) -> Result<()> {
    // 屏幕熄灭或锁屏时，系统会把 `input text` 直接丢弃，先唤醒并解锁
    let _ = adb.raw(&["shell", "input keyevent 224"]).await; // KEYCODE_WAKEUP
    let _ = adb.raw(&["shell", "wm dismiss-keyguard"]).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    // 确保 Terminal 在前台，避免把命令打进别的界面
    let fg = uia::foreground_package(adb).await.unwrap_or_default();
    if !fg.contains("virtualization.terminal") {
        open(adb).await?;
        tokio::time::sleep(Duration::from_millis(1800)).await;
    }
    focus(adb).await?;
    // 先清行，避免上一次输入残留（Ctrl+C，KEYCODE_CTRL_LEFT + KEYCODE_C）
    let _ = adb.raw(&["shell", "input keycombination 113 31"]).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    uia::type_text(adb, &format!("{} #", command)).await?;
    tokio::time::sleep(Duration::from_millis(350)).await;
    uia::press_enter(adb).await?;
    Ok(())
}
