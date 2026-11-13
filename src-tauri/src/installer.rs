//! 桌面环境安装的编排逻辑。
//!
//! 关键设计：**不读取终端屏幕**。AVF 虚拟机与 Android 之间通过 virtiofs 共享存储
//! （虚拟机内的 /mnt/shared 对应手机上的 Download 目录），于是形成一条可靠的双向通道：
//!
//!   PC ──adb push──▶ 手机共享目录 ──▶ 虚拟机内 /mnt/shared
//!   PC ◀──adb cat ─── 手机共享目录 ◀── 虚拟机内 /mnt/shared
//!
//! 脚本把进度与结果写入共享目录里的日志文件，PC 端轮询读取该文件即可获得
//! 实时进度、失败原因与退出状态；相比 OCR 或解析终端节点树，这条通道稳定得多。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::adb::Adb;
use crate::terminal::{self, TerminalPhase};

/// 虚拟机内可能存在的共享目录。
const GUEST_CANDIDATES: [&str; 4] = [
    "/mnt/shared",
    "/mnt/shared/Download",
    "/mnt/shared/Downloads",
    "/mnt/shared/shared",
];

/// 手机上可能对应的目录。
///
/// 注意：Terminal 应用共享的是**内部存储根目录**（`/storage/emulated/0`），
/// 而不是 Download 子目录——这一点在 Android 17 的 new2 版本上尤其明显，
/// 写进 `/mnt/shared` 的文件会直接落在 `/storage/emulated/0/` 下。
const PHONE_CANDIDATES: [&str; 5] = [
    "/storage/emulated/0",
    "/sdcard",
    "/storage/emulated/0/Download",
    "/sdcard/Download",
    "/storage/emulated/0/Downloads",
];

const SCRIPT_NAME: &str = "acl_install.sh";
const LOG_NAME: &str = "acl_install.log";
const TEMPLATE: &str = include_str!("../resources/install-desktop.sh");

/// 环境检测脚本（只读检查，不改动系统）。
const PROBE_SCRIPT_NAME: &str = "acl_probe.sh";
const PROBE_LOG_NAME: &str = "acl_probe.log";
const PROBE_TEMPLATE: &str = include_str!("../resources/detect-installed.sh");

/// 安装选项（来自前端）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOptions {
    /// xfce | gnome | kde | mate | cinnamon | lxqt | lxde | flashback
    pub desktop: String,
    /// 是否切换到国内镜像源。
    pub cn_mirror: bool,
    /// zh_CN.UTF-8 | en_US.UTF-8
    pub locale: String,
    pub vnc_password: String,
    pub vnc_display: String,
    pub vnc_geometry: String,
    pub install_apps: bool,
    pub install_ime: bool,
    pub ssh_port: u16,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            desktop: "xfce".to_string(),
            cn_mirror: true,
            locale: "zh_CN.UTF-8".to_string(),
            vnc_password: "linux".to_string(),
            vnc_display: ":1".to_string(),
            vnc_geometry: "1920x1080".to_string(),
            install_apps: true,
            install_ime: true,
            ssh_port: 10022,
        }
    }
}

/// 安装流程状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStatus {
    /// idle | preparing | running | success | failed | cancelled
    pub state: String,
    pub message: String,
    /// 当前阶段标识，如 apt_update / desktop / vnc。
    pub step: String,
    pub guest_dir: String,
    pub phone_dir: String,
    pub script_path: String,
}

impl Default for InstallStatus {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            message: String::new(),
            step: String::new(),
            guest_dir: String::new(),
            phone_dir: String::new(),
            script_path: String::new(),
        }
    }
}

/// 共享存储映射关系。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedMapping {
    pub guest_dir: String,
    pub phone_dir: String,
    pub detected: bool,
}

/// 全局运行状态。
#[derive(Default)]
pub struct InstallState {
    pub status: InstallStatus,
    pub cancel: AtomicBool,
}

pub type SharedState = Arc<tokio::sync::Mutex<InstallState>>;

const STEP_LABELS: &[(&str, &str)] = &[
    ("environment", "检查虚拟机环境"),
    ("detect_system", "识别 Debian 版本"),
    ("mirror", "切换国内软件源"),
    ("apt_update", "更新软件包"),
    ("locale", "配置语言与字体"),
    ("network_patch", "应用网络补丁"),
    ("ssh", "配置 SSH 服务"),
    ("desktop", "安装桌面环境"),
    ("vnc", "配置 TigerVNC"),
    ("shared", "挂载共享目录"),
    ("optional", "安装可选组件"),
    ("done", "收尾"),
];

fn step_label(step: &str) -> String {
    STEP_LABELS
        .iter()
        .find(|(k, _)| *k == step)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| step.to_string())
}

/// 渲染安装脚本（注入配置区）。
pub fn render_script(opts: &InstallOptions, log_path: &str) -> Result<String> {
    let config = format!(
        r#"DESKTOP="{desktop}"
CN_MIRROR="{cn_mirror}"
SYS_LOCALE="{locale}"
VNC_PASSWORD="{vnc_password}"
VNC_DISPLAY="{vnc_display}"
VNC_GEOMETRY="{vnc_geometry}"
INSTALL_APPS="{install_apps}"
INSTALL_IME="{install_ime}"
SSH_PORT="{ssh_port}"
ACL_LOG_FILE="{log_path}""#,
        desktop = opts.desktop,
        cn_mirror = if opts.cn_mirror { 1 } else { 0 },
        locale = opts.locale,
        vnc_password = opts.vnc_password.replace('"', ""),
        vnc_display = opts.vnc_display,
        vnc_geometry = opts.vnc_geometry,
        install_apps = if opts.install_apps { 1 } else { 0 },
        install_ime = if opts.install_ime { 1 } else { 0 },
        ssh_port = opts.ssh_port,
        log_path = log_path,
    );

    let lines: Vec<&str> = TEMPLATE.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("配置区 ==="))
        .ok_or_else(|| anyhow!("脚本模板缺少配置区标记"))?;
    let end = lines
        .iter()
        .position(|l| l.contains("配置区结束"))
        .ok_or_else(|| anyhow!("脚本模板缺少配置区结束标记"))?;

    let mut out: Vec<String> = Vec::new();
    out.extend(lines[..start].iter().map(|s| s.to_string()));
    out.push(config);
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    Ok(out.join("\n"))
}

/// 探测虚拟机内共享目录与手机目录的映射关系。
pub async fn probe_shared(adb: &Adb) -> Result<SharedMapping> {
    // 在虚拟机里创建探针文件（只使用安全字符，便于 input text）
    let mut cmd = String::from("touch");
    for (i, dir) in GUEST_CANDIDATES.iter().enumerate() {
        cmd.push_str(&format!(" {}/acl_probe_{}", dir, i));
    }
    cmd.push_str(" 2>/dev/null");
    terminal::run_command(adb, &cmd).await?;
    tokio::time::sleep(Duration::from_millis(2000)).await;

    for phone_dir in PHONE_CANDIDATES {
        let listing = adb
            .shell(&format!("ls -1 '{}' 2>/dev/null", phone_dir))
            .await
            .unwrap_or_default();
        for (i, guest_dir) in GUEST_CANDIDATES.iter().enumerate() {
            if listing.contains(&format!("acl_probe_{}", i)) {
                // 清理探针文件
                let cleanup = format!("rm -f {}/acl_probe_{}", guest_dir, i);
                let _ = terminal::run_command(adb, &cleanup).await;
                return Ok(SharedMapping {
                    guest_dir: guest_dir.to_string(),
                    phone_dir: phone_dir.to_string(),
                    detected: true,
                });
            }
        }
    }

    // 未探测到时使用默认映射（Terminal 应用共享的是内部存储根目录），脚本仍会尽力写入日志
    Ok(SharedMapping {
        guest_dir: "/mnt/shared".to_string(),
        phone_dir: "/sdcard".to_string(),
        detected: false,
    })
}

/// 把脚本推送到手机共享目录。
pub async fn push_script(
    adb: &Adb,
    phone_dir: &str,
    script: &str,
    workdir: &std::path::Path,
) -> Result<String> {
    tokio::fs::create_dir_all(workdir).await.ok();
    let local = workdir.join(SCRIPT_NAME);
    tokio::fs::write(&local, script)
        .await
        .with_context(|| format!("写入本地脚本失败: {}", local.display()))?;
    let remote = format!("{}/{}", phone_dir.trim_end_matches('/'), SCRIPT_NAME);
    adb.push(&local, &remote).await?;
    Ok(remote)
}

/// 从日志内容中解析结果标记。
fn parse_result(content: &str) -> Option<(bool, String)> {
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("@@RESULT:OK") {
            return Some((true, rest.trim().to_string()));
        }
        if let Some(rest) = line.strip_prefix("@@RESULT:FAIL:") {
            return Some((false, rest.trim().to_string()));
        }
    }
    None
}

/// 虚拟机内已安装环境的检测结果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledReport {
    /// 是否真正执行并取回了检测结果。
    pub checked: bool,
    /// 是否检测到任意桌面环境。
    pub desktop: bool,
    /// 检测到的桌面名称（命中多个时取第一个）。
    pub desktop_name: String,
    /// 命中的桌面 id（xfce / gnome / kde ...）。
    pub desktops: Vec<String>,
    /// 常用软件是否全部安装。
    pub apps: bool,
    pub apps_installed: u32,
    pub apps_total: u32,
    pub ime: bool,
    pub vnc: bool,
    pub ssh: bool,
    pub message: String,
}

fn desktop_label(id: &str) -> &'static str {
    match id {
        "xfce" => "XFCE",
        "gnome" => "GNOME",
        "kde" => "KDE Plasma",
        "mate" => "MATE",
        "cinnamon" => "Cinnamon",
        "lxqt" => "LXQt",
        "lxde" => "LXDE",
        "flashback" => "GNOME Flashback",
        _ => "未知桌面",
    }
}

/// 解析检测脚本输出的 KEY=VALUE 行。
fn parse_probe(content: &str) -> InstalledReport {
    let mut r = InstalledReport {
        checked: true,
        ..Default::default()
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("DESKTOP=") {
            r.desktops = v
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect();
            r.desktop = !r.desktops.is_empty();
            r.desktop_name = r
                .desktops
                .first()
                .map(|id| desktop_label(id).to_string())
                .unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("APPS=") {
            if let Some((have, total)) = v.split_once('/') {
                r.apps_installed = have.trim().parse().unwrap_or(0);
                r.apps_total = total.trim().parse().unwrap_or(0);
                r.apps = r.apps_total > 0 && r.apps_installed == r.apps_total;
            }
        } else if let Some(v) = line.strip_prefix("IME=") {
            r.ime = v.trim() == "1";
        } else if let Some(v) = line.strip_prefix("VNC=") {
            r.vnc = v.trim() == "1";
        } else if let Some(v) = line.strip_prefix("SSH=") {
            r.ssh = v.trim() == "1";
        }
    }
    r.message = if r.desktop {
        format!("已检测到桌面环境：{}", r.desktop_name)
    } else {
        "未检测到已安装的桌面环境".to_string()
    };
    r
}

/// 在虚拟机内检测已安装的桌面环境、常用软件与中文输入法。
///
/// 走与安装相同的共享目录通道：推送检测脚本 → 在 Terminal 中 `bash` 执行 →
/// 轮询脚本写回的日志。检测脚本只读取 `dpkg` 状态，不会改动系统。
pub async fn detect_installed(adb: &Adb, workdir: &std::path::Path) -> Result<InstalledReport> {
    // 终端没就绪时先尝试打开（虚拟机冷启动可能需要一段时间）
    let current = terminal::phase(adb).await.unwrap_or(TerminalPhase::Unknown);
    if current != TerminalPhase::Ready {
        terminal::open(adb).await?;
        let p = terminal::wait_ready(adb, 180).await?;
        if p != TerminalPhase::Ready {
            bail!(
                "Linux Terminal 未就绪（当前状态：{}），暂时无法检测已安装环境。",
                p.label()
            );
        }
    }

    let mapping = probe_shared(adb).await?;
    let guest_script = format!(
        "{}/{}",
        mapping.guest_dir.trim_end_matches('/'),
        PROBE_SCRIPT_NAME
    );
    let guest_log = format!(
        "{}/{}",
        mapping.guest_dir.trim_end_matches('/'),
        PROBE_LOG_NAME
    );
    let phone_log = format!(
        "{}/{}",
        mapping.phone_dir.trim_end_matches('/'),
        PROBE_LOG_NAME
    );

    tokio::fs::create_dir_all(workdir).await.ok();
    let local = workdir.join(PROBE_SCRIPT_NAME);
    tokio::fs::write(&local, PROBE_TEMPLATE).await?;
    adb.push(
        &local,
        &format!(
            "{}/{}",
            mapping.phone_dir.trim_end_matches('/'),
            PROBE_SCRIPT_NAME
        ),
    )
    .await?;

    adb.shell(&format!("rm -f '{}'", phone_log)).await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let command = format!("bash {} {}", guest_script, guest_log);
    terminal::run_command(adb, &command).await?;

    for round in 0..40 {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let content = adb
            .shell(&format!("cat '{}' 2>/dev/null", phone_log))
            .await
            .unwrap_or_default();
        if content.contains("@@DONE") {
            return Ok(parse_probe(&content));
        }
        // 命令可能没被终端接收：10 秒还没有任何输出就重发一次
        if round == 9 && content.trim().is_empty() {
            let _ = terminal::run_command(adb, &command).await;
        }
    }

    bail!(
        "检测超时：虚拟机内没有返回结果。\n\
         请确认手机保持亮屏，并手动点一下 Terminal 的命令行区域后重试。"
    )
}

/// 后台监控日志文件并推送事件。
pub async fn monitor(
    app: AppHandle,
    adb: Adb,
    phone_log: String,
    command: String,
    state: SharedState,
) {
    let mut pos: usize = 0;
    let mut first_seen = false;
    let mut elapsed = 0u32;
    let mut attempt = 0u32;
    let mut since_exec = 0u32;

    loop {
        {
            let s = state.lock().await;
            if s.cancel.load(Ordering::SeqCst) {
                let mut s = s;
                s.status.state = "cancelled".to_string();
                s.status.message = "已取消安装。".to_string();
                let _ = app.emit("install-status", &s.status);
                return;
            }
        }

        let content = adb
            .shell(&format!("cat '{}' 2>/dev/null", phone_log))
            .await
            .unwrap_or_default();

        if !content.is_empty() {
            first_seen = true;
            if content.len() > pos {
                let chunk = &content[pos..];
                if let Some(nl) = chunk.rfind('\n') {
                    let printable = &chunk[..nl + 1];
                    pos += nl + 1;
                    for line in printable.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Some(step) = line.trim().strip_prefix("@@STEP:") {
                            let label = step_label(step);
                            {
                                let mut s = state.lock().await;
                                s.status.step = step.to_string();
                                s.status.message = label.clone();
                                let _ = app.emit("install-step", &s.status);
                            }
                        } else if line.trim().starts_with("@@RESULT:") {
                            // 结果行单独处理
                        } else {
                            let _ = app.emit("install-log", line.to_string());
                        }
                    }
                }
            }

            if let Some((ok, msg)) = parse_result(&content) {
                let mut s = state.lock().await;
                s.status.state = if ok { "success" } else { "failed" }.to_string();
                s.status.message = if msg.is_empty() {
                    if ok {
                        "桌面环境安装完成。".to_string()
                    } else {
                        "安装失败。".to_string()
                    }
                } else {
                    msg
                };
                let payload = s.status.clone();
                drop(s);
                let _ = app.emit("install-status", &payload);
                return;
            }
        }

        elapsed += 1;
        since_exec += 1;
        // 命令可能因为没点中终端而根本没执行——重新聚焦再试一次
        if !first_seen && since_exec >= 12 {
            attempt += 1;
            if attempt >= 3 {
                let mut s = state.lock().await;
                s.status.state = "failed".to_string();
                s.status.message = format!(
                    "在 {} 秒内没有检测到虚拟机内脚本的日志输出。\n\
                     可能原因：虚拟机仍在启动、共享目录不可写，或自动输入的命令未被终端接收。\n\n\
                     可以在手机 Terminal 应用里手动执行下面这条命令，然后点击「重试」：\n{}",
                    elapsed * 15 / 10, command
                );
                let _ = app.emit("install-status", &s.status);
                return;
            }
            {
                let mut s = state.lock().await;
                s.status.message = format!("命令似乎没有在终端中执行，正在重新聚焦后重试（第 {} 次）...", attempt);
                let _ = app.emit("install-status", &s.status);
            }
            adb.shell(&format!("rm -f '{}'", phone_log)).await.ok();
            tokio::time::sleep(Duration::from_millis(600)).await;
            if let Err(e) = terminal::run_command(&adb, &command).await {
                let mut s = state.lock().await;
                s.status.state = "failed".to_string();
                s.status.message = format!("重新执行命令失败: {}", e);
                let _ = app.emit("install-status", &s.status);
                return;
            }
            since_exec = 0;
        }
        if !first_seen && elapsed > 120 {
            let mut s = state.lock().await;
            s.status.state = "failed".to_string();
            s.status.message = format!(
                "在 {} 秒内没有检测到虚拟机内脚本的日志输出。\n\
                 可能原因：虚拟机仍在启动、共享目录不可写，或自动输入的命令未被终端接收。\n\n\
                 可以在手机 Terminal 应用里手动执行下面这条命令，然后点击「重试」：\n{}",
                elapsed, command
            );
            let payload = s.status.clone();
            drop(s);
            let _ = app.emit("install-status", &payload);
            return;
        }

        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
}

/// 准备阶段：确保终端就绪、探测共享目录、推送脚本。
///
/// `emit` 用于把进度回调给上层（GUI 发事件 / 命令行打印）。
pub async fn prepare_core(
    adb: &Adb,
    opts: &InstallOptions,
    workdir: &std::path::Path,
    emit: &mut (dyn FnMut(InstallStatus) + Send),
) -> Result<SharedMapping> {
    let mut set = |state: &str, message: &str, step: &str| {
        emit(InstallStatus {
            state: state.to_string(),
            message: message.to_string(),
            step: step.to_string(),
            guest_dir: String::new(),
            phone_dir: String::new(),
            script_path: String::new(),
        });
    };

    set("preparing", "正在启动 Linux Terminal 应用...", "");

    terminal::open(adb).await?;
    // 首次运行可能弹出权限申请，自动放行
    let _ = terminal::dismiss_permission_dialogs(adb).await;

    // 等待终端就绪；若显示安装引导页则自动点击安装
    let phase = terminal::phase(adb).await?;
    if phase == TerminalPhase::NeedsInstall {
        set(
            "preparing",
            "正在安装 Linux 终端系统镜像（约 600 MB，请保持网络畅通）...",
            "",
        );
        terminal::tap_install(adb).await?;
        let phase = terminal::wait_ready(adb, 3600).await?;
        if phase != TerminalPhase::Ready {
            bail!(
                "Linux 终端未就绪（当前状态：{}）。请在手机上确认 Terminal 应用已正常打开并显示命令行提示符后重试。",
                phase.label()
            );
        }
    } else {
        let phase = terminal::wait_ready(adb, 300).await?;
        if phase != TerminalPhase::Ready {
            bail!(
                "Linux 终端未就绪（当前状态：{}）。请在手机上确认 Terminal 应用已正常打开并显示命令行提示符后重试。",
                phase.label()
            );
        }
    }

    set("preparing", "正在探测虚拟机共享存储...", "");
    let mapping = probe_shared(adb).await?;

    let guest_log = format!("{}/{}", mapping.guest_dir.trim_end_matches('/'), LOG_NAME);
    let script = render_script(opts, &guest_log)?;
    let remote = push_script(adb, &mapping.phone_dir, &script, workdir).await?;

    emit(InstallStatus {
        state: "preparing".to_string(),
        message: format!("脚本已推送至 {}", remote),
        step: String::new(),
        guest_dir: mapping.guest_dir.clone(),
        phone_dir: mapping.phone_dir.clone(),
        script_path: format!("{}/{}", mapping.guest_dir.trim_end_matches('/'), SCRIPT_NAME),
    });

    Ok(mapping)
}

/// 准备阶段（GUI 版）。
pub async fn prepare(
    app: &AppHandle,
    adb: &Adb,
    opts: &InstallOptions,
    workdir: &std::path::Path,
    state: &SharedState,
) -> Result<SharedMapping> {
    let app = app.clone();
    let state = state.clone();
    let mut emit = move |s: InstallStatus| {
        if let Ok(mut guard) = state.try_lock() {
            guard.status = s.clone();
        }
        let _ = app.emit("install-status", &s);
    };
    prepare_core(adb, opts, workdir, &mut emit).await
}

/// 命令行版：完整跑一遍安装（准备 → 执行 → 等待结果），实时打印日志。
pub async fn run_headless(
    adb: &Adb,
    opts: &InstallOptions,
    workdir: &std::path::Path,
) -> Result<InstallStatus> {
    let mut last = InstallStatus::default();
    {
        let mut emit = |s: InstallStatus| {
            if !s.message.is_empty() {
                println!("[{}] {}", s.state, s.message);
            }
        };
        let mapping = prepare_core(adb, opts, workdir, &mut emit).await?;

        last.state = "running".to_string();
        last.message = "正在虚拟机终端中执行安装脚本...".to_string();
        println!("[running] {}", last.message);

        let cmd = format!(
            "bash {}/{}",
            mapping.guest_dir.trim_end_matches('/'),
            SCRIPT_NAME
        );
        let phone_log = format!(
            "{}/{}",
            mapping.phone_dir.trim_end_matches('/'),
            LOG_NAME
        );
        // 清掉上一轮的日志：日志是追加写的，留着旧的 @@RESULT:OK 会被误读成本轮成功
        adb.shell(&format!("rm -f '{}'", phone_log)).await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
        terminal::run_command(adb, &cmd).await?;

        // 轮询日志
        let mut pos: usize = 0;
        let mut first_seen = false;
        let mut elapsed = 0u32;
        let mut attempt = 0u32;
        let mut since_exec = 0u32;
        loop {
            let content = adb
                .shell(&format!("cat '{}' 2>/dev/null", phone_log))
                .await
                .unwrap_or_default();
            if !content.is_empty() {
                first_seen = true;
                if content.len() > pos {
                    let chunk = &content[pos..];
                    if let Some(nl) = chunk.rfind('\n') {
                        let printable = &chunk[..nl + 1];
                        pos += nl + 1;
                        for line in printable.lines() {
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Some(step) = line.trim().strip_prefix("@@STEP:") {
                                println!("▶ {}", step_label(step));
                            } else if line.trim().starts_with("@@RESULT:") {
                                // 结果行稍后单独处理
                            } else {
                                println!("{}", line);
                            }
                        }
                    }
                }
                if let Some((ok, msg)) = parse_result(&content) {
                    last.state = if ok { "success" } else { "failed" }.to_string();
                    last.message = if msg.is_empty() {
                        if ok { "桌面环境安装完成。".to_string() } else { "安装失败。".to_string() }
                    } else {
                        msg
                    };
                    println!("\n@@ {} {}", last.state, last.message);
                    return Ok(last);
                }
            }
            elapsed += 1;
            since_exec += 1;
            // 命令可能因为没点中终端而根本没执行——重新聚焦再试一次
            if !first_seen && since_exec >= 12 {
                attempt += 1;
                if attempt >= 3 {
                    last.state = "failed".to_string();
                    last.message = format!(
                        "{} 秒内没有检测到虚拟机内脚本的日志输出。\n可能原因：虚拟机仍在启动、共享目录不可写，或自动输入的命令未被终端接收。\n\n可在手机 Terminal 里手动执行：\n{}",
                        elapsed * 15 / 10, cmd
                    );
                    return Ok(last);
                }
                println!(
                    "[重试] 命令似乎没有在终端中执行，重新聚焦后重试（第 {} 次）...",
                    attempt
                );
                adb.shell(&format!("rm -f '{}'", phone_log)).await.ok();
                tokio::time::sleep(Duration::from_millis(600)).await;
                terminal::run_command(adb, &cmd).await?;
                since_exec = 0;
            }
            if elapsed > 120 {
                last.state = "failed".to_string();
                last.message = format!(
                    "{} 秒内没有检测到虚拟机内脚本的日志输出。\n可能原因：虚拟机仍在启动、共享目录不可写，或自动输入的命令未被终端接收。\n\n可在手机 Terminal 里手动执行：\n{}",
                    elapsed * 15 / 10, cmd
                );
                return Ok(last);
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }
}

/// 启动安装：进入终端执行脚本，并开启日志监控。
pub async fn start(
    app: &AppHandle,
    adb: &Adb,
    _opts: &InstallOptions,
    mapping: &SharedMapping,
    state: SharedState,
) -> Result<()> {
    {
        let mut s = state.lock().await;
        s.cancel.store(false, Ordering::SeqCst);
        s.status.state = "running".to_string();
        s.status.step = String::new();
        s.status.message = "正在虚拟机终端中执行安装脚本...".to_string();
        let payload = s.status.clone();
        let _ = app.emit("install-status", &payload);
    }

    let cmd = format!(
        "bash {}/{}",
        mapping.guest_dir.trim_end_matches('/'),
        SCRIPT_NAME
    );
    let phone_log = format!(
        "{}/{}",
        mapping.phone_dir.trim_end_matches('/'),
        LOG_NAME
    );
    // 清掉上一轮的日志（追加写的旧日志会被误读成本轮成功）
    adb.shell(&format!("rm -f '{}'", phone_log)).await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    terminal::run_command(adb, &cmd).await?;

    let handle = app.clone();
    let adb_clone = adb.clone();
    let state_clone = state.clone();
    let cmd_clone = cmd.clone();
    tauri::async_runtime::spawn(async move {
        monitor(handle, adb_clone, phone_log, cmd_clone, state_clone).await;
    });

    Ok(())
}

/// 请求取消当前安装。
pub async fn cancel(state: &SharedState) {
    let s = state.lock().await;
    s.cancel.store(true, Ordering::SeqCst);
}
