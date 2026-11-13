//! Tauri 命令层：把各个能力模块暴露给前端。

use std::path::PathBuf;

use tauri::{AppHandle, State};

use crate::adb::Adb;
use crate::detect::AvfReport;
use crate::devopts::{self, DevOptionReport, SecurityIssue};
use crate::installer::{self, InstallOptions, InstallStatus};
use crate::ports::{self, PortResult};
use crate::state::AppState;
use crate::terminal::{self, TerminalPhase};
use crate::uia;
use crate::vnc;

/// adb 运行环境信息。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvInfo {
    pub adb_path: String,
    pub adb_found: bool,
    pub adb_version: String,
    pub error: Option<String>,
}

async fn resolve_adb(
    state: &AppState,
    adb_path: Option<String>,
    serial: Option<String>,
) -> Result<Adb, String> {
    let exe: PathBuf = match adb_path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => {
            let saved = state.adb_path.lock().await.clone();
            match saved {
                Some(p) if !p.trim().is_empty() => PathBuf::from(p),
                _ => Adb::locate().map_err(|e| e.to_string())?,
            }
        }
    };
    Ok(Adb::new(exe, serial))
}

/// 检查 adb 是否可用。
#[tauri::command]
pub async fn check_adb(state: State<'_, AppState>, adb_path: Option<String>) -> Result<EnvInfo, String> {
    let exe = match adb_path {
        Some(p) if !p.trim().is_empty() => Ok(PathBuf::from(p)),
        _ => {
            let saved = state.adb_path.lock().await.clone();
            match saved {
                Some(p) if !p.trim().is_empty() => Ok(PathBuf::from(p)),
                _ => Adb::locate(),
            }
        }
    };
    match exe {
        Ok(path) => {
            let adb = Adb::new(path.clone(), None);
            match adb.raw(&["version"]).await {
                Ok(out) if out.ok() => Ok(EnvInfo {
                    adb_path: path.to_string_lossy().to_string(),
                    adb_found: true,
                    adb_version: out.stdout.lines().next().unwrap_or("").trim().to_string(),
                    error: None,
                }),
                Ok(out) => Ok(EnvInfo {
                    adb_path: path.to_string_lossy().to_string(),
                    adb_found: false,
                    adb_version: String::new(),
                    error: Some(format!("执行 adb version 失败: {}", out.combined().trim())),
                }),
                Err(e) => Ok(EnvInfo {
                    adb_path: path.to_string_lossy().to_string(),
                    adb_found: false,
                    adb_version: String::new(),
                    error: Some(e.to_string()),
                }),
            }
        }
        Err(e) => Ok(EnvInfo {
            adb_path: String::new(),
            adb_found: false,
            adb_version: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

/// 保存用户指定的 adb 路径。
#[tauri::command]
pub async fn set_adb_path(state: State<'_, AppState>, adb_path: String) -> Result<(), String> {
    let mut guard = state.adb_path.lock().await;
    if adb_path.trim().is_empty() {
        *guard = None;
    } else {
        *guard = Some(adb_path);
    }
    Ok(())
}

/// 列出已连接的设备。
#[tauri::command]
pub async fn list_devices(
    state: State<'_, AppState>,
    adb_path: Option<String>,
) -> Result<Vec<crate::adb::DeviceEntry>, String> {
    let adb = resolve_adb(&state, adb_path, None).await?;
    adb.devices().await.map_err(|e| e.to_string())
}

/// 检测设备是否支持 Linux 虚拟机。
#[tauri::command]
pub async fn check_avf(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<AvfReport, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    crate::detect::check_avf(&adb).await.map_err(|e| e.to_string())
}

/// 读取开发者选项 / Linux 开发环境状态。
#[tauri::command]
pub async fn dev_options(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<DevOptionReport, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    devopts::report(&adb).await.map_err(|e| e.to_string())
}

/// 打开开发者选项页面。
#[tauri::command]
pub async fn open_dev_options(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<(), String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    devopts::open_dev_options(&adb).await.map_err(|e| e.to_string())
}

/// 打开「Linux 开发环境」开关（等价于 pm enable 终端应用）。
#[tauri::command]
pub async fn enable_linux_env(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<DevOptionReport, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    devopts::enable_linux_env(&adb).await.map_err(|e| e.to_string())?;
    let mut report = devopts::report(&adb).await.map_err(|e| e.to_string())?;
    // 打开开发者选项做一次视觉核对，确认开关确实处于打开状态
    if let Ok((visible, label, checked)) = devopts::verify_linux_entry(&adb).await {
        report.linux_entry_visible = visible;
        report.linux_entry_label = label;
        report.linux_switch_checked = checked;
    }
    Ok(report)
}

/// 扫描当前界面是否存在需要人工处理的安全弹窗。
#[tauri::command]
pub async fn scan_security(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<Option<SecurityIssue>, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let nodes = uia::dump(&adb).await.map_err(|e| e.to_string())?;
    let (policy, detail) = devopts::device_policy(&adb).await;
    if policy {
        return Ok(Some(SecurityIssue {
            kind: "enterprise_lock".to_string(),
            title: "设备受企业策略管控".to_string(),
            detail: format!(
                "检测到设备策略所有者：{}\n企业策略会阻止自动开启开发者选项，请先在手机上移除企业管理后重试。",
                detail
            ),
            auto_clickable: false,
        }));
    }
    Ok(devopts::scan_security_issue(&nodes))
}

/// 自动点击界面上的「允许 / Allow」按钮。
#[tauri::command]
pub async fn click_allow(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<bool, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let nodes = uia::dump(&adb).await.map_err(|e| e.to_string())?;
    devopts::click_allow(&adb, &nodes).await.map_err(|e| e.to_string())
}

/// 查询 Linux Terminal 应用所处阶段。
#[tauri::command]
pub async fn terminal_phase(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<TerminalPhase, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    terminal::phase(&adb).await.map_err(|e| e.to_string())
}

/// 只读检测「Linux 系统镜像是否已经安装」，不会触发下载。
#[tauri::command]
pub async fn detect_linux_image(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<terminal::ImageStatus, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    terminal::detect_image(&adb).await.map_err(|e| e.to_string())
}

/// 「安装 Linux 系统」步骤：授权、打开 Terminal、必要时点安装、等待终端就绪。
#[tauri::command]
pub async fn install_linux_image(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<terminal::ImageStatus, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let handle = app.clone();
    let mut emit = move |status: terminal::ImageStatus| {
        use tauri::Emitter;
        let _ = handle.emit("image-progress", status);
    };
    terminal::install_image(&adb, &mut emit)
        .await
        .map_err(|e| e.to_string())
}

/// 打开 Linux Terminal 应用。
#[tauri::command]
pub async fn open_terminal(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<(), String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    // 打开 Terminal 之前先授予录音与通知权限，避免权限弹窗挡住安装引导页
    terminal::grant_permissions(&adb).await;
    terminal::open(&adb).await.map_err(|e| e.to_string())
}

/// 检测虚拟机内已安装的桌面环境、常用软件与中文输入法。
#[tauri::command]
pub async fn detect_installed(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<installer::InstalledReport, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let workdir = state.workdir();
    installer::detect_installed(&adb, &workdir)
        .await
        .map_err(|e| e.to_string())
}

/// 执行一个安装阶段（准备 + 启动 + 后台监控）。
///
/// `stage` 取 base / desktop / apps / vncsvc，缺省时执行全部阶段。
#[tauri::command]
pub async fn run_install(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
    options: InstallOptions,
    stage: Option<String>,
) -> Result<InstallStatus, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let workdir = state.workdir();
    let install_state = state.install.clone();
    let stage = stage.unwrap_or_else(|| "all".to_string());

    let mapping = installer::prepare(&app, &adb, &options, &stage, &workdir, &install_state)
        .await
        .map_err(|e| e.to_string())?;
    installer::start(&app, &adb, &options, &mapping, install_state.clone(), &stage)
        .await
        .map_err(|e| e.to_string())?;

    let s = install_state.lock().await;
    Ok(s.status.clone())
}

/// 读取安装状态。
#[tauri::command]
pub async fn install_status(state: State<'_, AppState>) -> Result<InstallStatus, String> {
    let s = state.install.lock().await;
    Ok(s.status.clone())
}

/// 取消安装。
#[tauri::command]
pub async fn cancel_install(state: State<'_, AppState>) -> Result<(), String> {
    installer::cancel(&state.install).await;
    Ok(())
}

/// 在 Terminal 应用中放行指定端口。
#[tauri::command]
pub async fn enable_port(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
    port: u16,
) -> Result<PortResult, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    ports::enable_port(&adb, port).await.map_err(|e| e.to_string())
}

/// 建立端口转发并启动 TigerVNC Viewer。
///
/// 本机没有安装 TigerVNC 时，会自动下载官方免安装版；下载过程通过
/// `vnc-progress` 事件把进度推给前端。
#[tauri::command]
pub async fn setup_vnc(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
    port: u16,
    password: Option<String>,
) -> Result<vnc::VncSetup, String> {
    use tauri::Emitter;
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let handle = app.clone();
    let mut progress = move |msg: String| {
        let _ = handle.emit("vnc-progress", msg);
    };
    let password = password.unwrap_or_else(|| "linux".to_string());
    vnc::setup_with(&adb, port, &password, &mut progress)
        .await
        .map_err(|e| e.to_string())
}

/// 在手机上安装并启动 AVNC 客户端（优先用程序内置的 APK）。
#[tauri::command]
pub async fn install_avnc(
    app: AppHandle,
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
    password: Option<String>,
) -> Result<String, String> {
    use tauri::Emitter;
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let handle = app.clone();
    let mut progress = move |msg: String| {
        let _ = handle.emit("vnc-progress", msg);
    };
    let password = password.unwrap_or_else(|| "linux".to_string());
    vnc::install_avnc(&adb, &password, &mut progress)
        .await
        .map_err(|e| e.to_string())
}

/// 截取手机屏幕（PNG），保存到本地并返回文件路径。
///
/// 排查安全弹窗、确认界面状态时非常有用。
#[tauri::command]
pub async fn capture_screen(
    state: State<'_, AppState>,
    serial: String,
    adb_path: Option<String>,
) -> Result<String, String> {
    let adb = resolve_adb(&state, adb_path, Some(serial)).await?;
    let data = adb.screenshot().await.map_err(|e| e.to_string())?;
    let dir = state.workdir().join("screenshots");
    std::fs::create_dir_all(&dir).ok();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("screen-{}.png", ts));
    std::fs::write(&path, &data).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// 在 PC 上用默认浏览器打开链接。
#[tauri::command]
pub async fn open_external(url: String) -> Result<(), String> {
    vnc::open_url(&url).map_err(|e| e.to_string())
}
