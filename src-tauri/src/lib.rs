//! AndroidCreateLinuxGUITool
//!
//! 在支持 Android 虚拟化框架（AVF）的 Android 16+ 设备上，
//! 自动化完成「开启 Linux 开发环境 → 安装 Debian 桌面环境 → 通过 VNC 远程使用」的完整流程。

mod adb;
mod commands;
mod detect;
mod devopts;
mod installer;
mod ports;
mod state;
mod terminal;
mod uia;
mod vnc;

use adb::Adb;
use state::AppState;

/// 诊断模式：不启动 GUI，直接对已连接设备跑一遍检测并输出报告。
///
/// 用法：`AndroidCreateLinuxGUITool.exe --diagnose`
///
/// Release 版是 GUI 子系统程序，标准输出可能接不上（父进程是 ConPTY 时
/// AttachConsole 会失败），所以报告同时写到 exe 同目录下的
/// `acl-diagnose-report.txt`，保证一定能拿到结果。
pub fn diagnose() -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let mut report: Vec<String> = Vec::new();
        macro_rules! say {
            ($($arg:tt)*) => {{
                let line = format!($($arg)*);
                println!("{}", line);
                report.push(line);
            }};
        }
        macro_rules! fail {
            ($($arg:tt)*) => {{
                let line = format!($($arg)*);
                eprintln!("{}", line);
                report.push(line);
            }};
        }

        let exe = match Adb::locate() {
            Ok(p) => p,
            Err(e) => {
                fail!("[错误] {}", e);
                let _ = write_report(&report);
                return 1;
            }
        };
        let adb = Adb::new(exe, None);
        say!("adb 路径: {}", adb.exe().display());
        match adb.raw(&["version"]).await {
            Ok(o) if o.ok() => say!(
                "adb 版本: {}",
                o.stdout.lines().next().unwrap_or("").trim()
            ),
            Ok(o) => {
                fail!("[错误] adb version 失败: {}", o.combined().trim());
                let _ = write_report(&report);
                return 1;
            }
            Err(e) => {
                fail!("[错误] {}", e);
                let _ = write_report(&report);
                return 1;
            }
        }

        let devices = match adb.devices().await {
            Ok(d) => d,
            Err(e) => {
                fail!("[错误] 无法列出设备: {}", e);
                let _ = write_report(&report);
                return 1;
            }
        };
        say!("\n=== 设备列表 ({} 台) ===", devices.len());
        for d in &devices {
            say!(
                "  {}  [{}]  model={} product={}",
                d.serial, d.state, d.model, d.product
            );
        }
        let serial = match devices.iter().find(|d| d.state == "device") {
            Some(d) => d.serial.clone(),
            None => {
                fail!("\n[错误] 没有处于 device 状态的设备，请连接手机并开启 USB 调试。");
                let _ = write_report(&report);
                return 2;
            }
        };
        let adb = adb.with_serial(serial.clone());

        say!("\n=== AVF 能力检测 ({}) ===", serial);
        match detect::check_avf(&adb).await {
            Ok(r) => {
                say!("  机型         : {} {}", r.manufacturer, r.model);
                say!("  系统         : Android {} (API {})", r.android_release, r.sdk_int);
                let feature = if r.feature_line.is_empty() {
                    "未检测到 virtualization_framework".to_string()
                } else {
                    r.feature_line.clone()
                };
                say!("  系统特性     : {}", feature);
                let apex = if r.virt_apex_present {
                    "com.android.virt 已就绪"
                } else {
                    "缺失"
                };
                say!("  AVF 运行时   : {}", apex);
                let vm = if r.vm_info_ok { "执行成功" } else { "执行失败" };
                say!("  vm info      : {}", vm);
                let term = if !r.terminal_pkg_present {
                    "未包含"
                } else if r.terminal_pkg_enabled {
                    "已安装并启用"
                } else {
                    "已安装但未启用"
                };
                say!("  Terminal 应用: {}", term);
                let verdict = if r.supported {
                    "支持 Linux 虚拟机"
                } else {
                    "不支持 Linux 虚拟机"
                };
                say!("  结论         : {}", verdict);
                for reason in &r.reasons {
                    say!("    - {}", reason);
                }
                if r.vm_info_ok && !r.vm_info.is_empty() {
                    say!("\n--- vm info 原始输出 ---");
                    for line in r.vm_info.lines() {
                        say!("  {}", line);
                    }
                }
            }
            Err(e) => {
                fail!("[错误] AVF 检测失败: {}", e);
                let _ = write_report(&report);
                return 3;
            }
        }

        say!("\n=== 开发者选项 ===");
        match devopts::report(&adb).await {
            Ok(r) => {
                let dev = if r.dev_options_enabled { "已开启" } else { "未开启" };
                say!("  开发者选项   : {}", dev);
                let usb = if r.adb_enabled { "已开启" } else { "未开启" };
                say!("  USB 调试     : {}", usb);
                let linux = if r.linux_env_enabled { "已开启" } else { "未开启" };
                say!("  Linux 开发环境: {}", linux);
                let policy = if r.device_policy_present {
                    format!("存在 ({})", r.device_policy_detail)
                } else {
                    "无".to_string()
                };
                say!("  设备策略     : {}", policy);
                for n in &r.notes {
                    say!("    - {}", n);
                }
            }
            Err(e) => {
                fail!("[错误] 读取开发者选项状态失败: {}", e);
                let _ = write_report(&report);
                return 3;
            }
        }

        say!("\n诊断完成。");
        let path = write_report(&report);
        if let Some(p) = path {
            say!("报告已保存到: {}", p.display());
        }
        0
    })
}

/// 把诊断报告写到 exe 同目录（不可写则退回临时目录）。
fn write_report(lines: &[String]) -> Option<std::path::PathBuf> {
    let content = lines.join("\n");
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("acl-diagnose-report.txt"));
        }
    }
    candidates.push(
        std::env::temp_dir().join("android-create-linux-gui-tool").join("acl-diagnose-report.txt"),
    );
    for path in candidates {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if std::fs::write(&path, content.as_bytes()).is_ok() {
            return Some(path);
        }
    }
    None
}

/// 自测模式：真机跑一遍「开启 Linux 开发环境 → 打开 Terminal → 识别阶段」。
///
/// 与 --diagnose 不同，这里会真的修改设备状态（启用 Terminal 应用），
/// 用于验证工具的核心链路是否可用。
pub fn selftest() -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let mut report: Vec<String> = Vec::new();
        macro_rules! say {
            ($($arg:tt)*) => {{
                let line = format!($($arg)*);
                println!("{}", line);
                report.push(line);
            }};
        }

        let exe = match Adb::locate() {
            Ok(p) => p,
            Err(e) => {
                say!("[错误] {}", e);
                let _ = write_report(&report);
                return 1;
            }
        };
        let adb = Adb::new(exe, None);
        let devices = adb.devices().await.unwrap_or_default();
        let serial = match devices.iter().find(|d| d.state == "device") {
            Some(d) => d.serial.clone(),
            None => {
                say!("[错误] 没有可用设备");
                let _ = write_report(&report);
                return 2;
            }
        };
        let adb = adb.with_serial(serial.clone());
        say!("设备: {}", serial);

        say!("\n=== 1) 开启 Linux 开发环境 ===");
        match devopts::enable_linux_env(&adb).await {
            Ok(m) => say!("  {}", m),
            Err(e) => {
                say!("  [失败] {}", e);
                let _ = write_report(&report);
                return 3;
            }
        }
        let (present, enabled) = devopts::terminal_enabled(&adb).await;
        say!("  Terminal 包存在: {}  已启用: {}", present, enabled);

        say!("\n=== 2) 核对开发者选项里的开关 ===");
        match devopts::verify_linux_entry(&adb).await {
            Ok((visible, label, checked)) => {
                say!("  找到条目: {}  「{}」", visible, label);
                say!(
                    "  开关状态: {}",
                    match checked {
                        Some(true) => "已打开".to_string(),
                        Some(false) => "未打开".to_string(),
                        None => "未取到".to_string(),
                    }
                );
            }
            Err(e) => say!("  [失败] {}", e),
        }

        say!("\n=== 3) 打开 Terminal 应用并识别阶段 ===");
        if let Err(e) = terminal::open(&adb).await {
            say!("  [失败] 启动失败: {}", e);
        }
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        match terminal::phase(&adb).await {
            Ok(p) => say!("  当前阶段: {} ({:?})", p.label(), p),
            Err(e) => say!("  [失败] 阶段识别失败: {}", e),
        }
        say!("  （未自动点击安装按钮，系统镜像由用户确认后再下载）");

        say!("\n自测完成。");
        let path = write_report(&report);
        if let Some(p) = path {
            say!("报告已保存到: {}", p.display());
        }
        0
    })
}

/// 命令行安装模式：完整跑一遍安装流程并实时打印日志。
///
/// 用法：`AndroidCreateLinuxGUITool.exe --install [--desktop=xfce] [--stage=base|desktop|apps|vncsvc] [--official-source]`
pub fn install_cli(args: &[String]) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let exe = match Adb::locate() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[错误] {}", e);
                return 1;
            }
        };
        let adb = Adb::new(exe, None);
        let devices = adb.devices().await.unwrap_or_default();
        let serial = match devices.iter().find(|d| d.state == "device") {
            Some(d) => d.serial.clone(),
            None => {
                eprintln!("[错误] 没有可用设备");
                return 2;
            }
        };
        let adb = adb.with_serial(serial.clone());
        println!("设备: {}", serial);

        let mut opts = installer::InstallOptions::default();
        let mut stage = "all".to_string();
        for a in args {
            if let Some(v) = a.strip_prefix("--desktop=") {
                opts.desktop = v.to_string();
            }
            if let Some(v) = a.strip_prefix("--stage=") {
                stage = v.to_string();
            }
            if a == "--official-source" {
                opts.cn_mirror = false;
                opts.locale = "en_US.UTF-8".to_string();
                opts.install_ime = false;
            }
            if a == "--no-apps" {
                opts.install_apps = false;
            }
        }
        println!(
            "配置: 桌面={} 国内源={} 语言={} 密码={} 分辨率={}",
            opts.desktop, opts.cn_mirror, opts.locale, opts.vnc_password, opts.vnc_geometry
        );

        let workdir = std::env::temp_dir().join("android-create-linux-gui-tool");
        match installer::run_headless(&adb, &opts, &stage, &workdir).await {
            Ok(status) => {
                println!("\n最终状态: {} - {}", status.state, status.message);
                if status.state == "success" {
                    0
                } else {
                    4
                }
            }
            Err(e) => {
                eprintln!("[错误] 安装失败: {}", e);
                5
            }
        }
    })
}

/// 命令行：强制下载 TigerVNC Viewer（免安装版）到本地目录。
///
/// 用法：`AndroidCreateLinuxGUITool.exe --fetch-viewer`
pub fn fetch_viewer_cli() -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let dir = vnc::tool_data_dir();
        tokio::fs::create_dir_all(&dir).await.ok();
        let target = dir.join("vncviewer.exe");
        let mut progress = |msg: String| println!("{}", msg);
        match vnc::download_viewer(&target, &mut progress).await {
            Ok(size) => {
                println!("下载完成: {}（{} 字节）", target.display(), size);
                0
            }
            Err(e) => {
                eprintln!("[错误] {}", e);
                1
            }
        }
    })
}

/// 命令行调试模式：在虚拟机终端里执行一条命令，并把输出取回来。
///
/// 用法：`AndroidCreateLinuxGUITool.exe --exec="ls -la /mnt/shared"`
///
/// 命令不会直接 `input text`（那样特殊字符会被设备 shell 拆坏），
/// 而是先写成一个脚本推到共享目录，再让终端执行 `bash <脚本>`，
/// 脚本的输出写回共享目录，PC 端轮询取回。
pub fn exec_cli(args: &[String]) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let user_cmd = args
            .iter()
            .find_map(|a| a.strip_prefix("--exec="))
            .unwrap_or("")
            .trim()
            .to_string();
        if user_cmd.is_empty() {
            eprintln!("用法: --exec=\"<要执行的命令>\"");
            return 2;
        }

        let exe = match Adb::locate() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[错误] {}", e);
                return 1;
            }
        };
        let adb = Adb::new(exe, None);
        let devices = adb.devices().await.unwrap_or_default();
        let serial = match devices.iter().find(|d| d.state == "device") {
            Some(d) => d.serial.clone(),
            None => {
                eprintln!("[错误] 没有可用设备");
                return 2;
            }
        };
        let adb = adb.with_serial(serial.clone());
        println!("设备: {}", serial);

        if let Err(e) = terminal::open(&adb).await {
            eprintln!("[错误] 启动 Terminal 失败: {}", e);
            return 3;
        }
        let _ = terminal::dismiss_permission_dialogs(&adb).await;

        let mapping = match installer::probe_shared(&adb).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[错误] 探测共享目录失败: {}", e);
                return 3;
            }
        };
        let guest_dir = mapping.guest_dir.trim_end_matches('/').to_string();
        let phone_dir = mapping.phone_dir.trim_end_matches('/').to_string();
        println!("共享目录: 虚拟机 {} <-> 手机 {}", guest_dir, phone_dir);

        let script = format!(
            "#!/bin/bash\nexec > {g}/acl_exec_out.txt 2>&1\n{c}\necho \"__EXIT__=$?\"\necho \"__DONE__\"\n",
            g = guest_dir,
            c = user_cmd
        );
        let workdir = std::env::temp_dir().join("android-create-linux-gui-tool");
        tokio::fs::create_dir_all(&workdir).await.ok();
        let local = workdir.join("acl_exec.sh");
        if let Err(e) = tokio::fs::write(&local, script.as_bytes()).await {
            eprintln!("[错误] 写入本地脚本失败: {}", e);
            return 4;
        }
        let remote = format!("{}/acl_exec.sh", phone_dir);
        if let Err(e) = adb.push(&local, &remote).await {
            eprintln!("[错误] 推送脚本失败: {}", e);
            return 4;
        }
        adb.shell(&format!("rm -f '{}/acl_exec_out.txt'", phone_dir))
            .await
            .ok();

        let cmd = format!("bash {}/acl_exec.sh", guest_dir);
        if let Err(e) = terminal::run_command(&adb, &cmd).await {
            eprintln!("[错误] 执行命令失败: {}", e);
            return 5;
        }

        let out_path = format!("{}/acl_exec_out.txt", phone_dir);
        for _ in 0..180 {
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            let content = adb
                .shell(&format!("cat '{}' 2>/dev/null", out_path))
                .await
                .unwrap_or_default();
            if content.contains("__DONE__") {
                print!("{}", content);
                return 0;
            }
        }
        eprintln!("[错误] 等待命令输出超时。");
        6
    })
}

/// 命令行端口模式：放行端口 + 建立 adb forward + 尝试打开 TigerVNC。
///
/// 用法：`AndroidCreateLinuxGUITool.exe --port=5901`
pub fn port_cli(args: &[String]) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[错误] 无法创建异步运行时: {}", e);
            return 1;
        }
    };
    rt.block_on(async {
        let exe = match Adb::locate() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[错误] {}", e);
                return 1;
            }
        };
        let adb = Adb::new(exe, None);
        let devices = adb.devices().await.unwrap_or_default();
        let serial = match devices.iter().find(|d| d.state == "device") {
            Some(d) => d.serial.clone(),
            None => {
                eprintln!("[错误] 没有可用设备");
                return 2;
            }
        };
        let adb = adb.with_serial(serial.clone());
        let port: u16 = args
            .iter()
            .find_map(|a| a.strip_prefix("--port="))
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(5901);
        println!("设备: {}  端口: {}", serial, port);

        println!("\n=== 放行端口 ===");
        match ports::enable_port(&adb, port).await {
            Ok(r) => {
                println!("结果: {}  action={}", if r.ok { "成功" } else { "失败" }, r.action);
                println!("说明: {}", r.message);
            }
            Err(e) => eprintln!("[错误] {}", e),
        }

        println!("\n=== 端口转发 + 客户端 ===");
        let password = args
            .iter()
            .find_map(|a| a.strip_prefix("--password="))
            .unwrap_or("linux")
            .to_string();
        let mut progress = |msg: String| println!("  {}", msg);
        match vnc::setup_with(&adb, port, &password, &mut progress).await {
            Ok(s) => {
                println!("转发: {}", s.forward_detail);
                println!("客户端: {}", if s.viewer_path.is_empty() { "未找到" } else { &s.viewer_path });
                println!("{}", s.message);
            }
            Err(e) => eprintln!("[错误] {}", e),
        }

        println!("\n=== 连接测试 ===");
        match probe_vnc(&adb, port).await {
            Ok(msg) => println!("{}", msg),
            Err(e) => eprintln!("[错误] {}", e),
        }

        println!("\n=== TigerVNC Viewer ===");
        match vnc::launch_tigervnc(port, &password).await {
            Ok((launched, path, msg)) => {
                if !path.is_empty() {
                    println!("客户端: {}", path);
                }
                println!("{}", msg);
                if !launched {
                    return 3;
                }
            }
            Err(e) => eprintln!("[错误] {}", e),
        }
        0
    })
}

/// 连到转发后的端口，读一下 VNC 的协议握手串，验证链路是否真的通了。
async fn probe_vnc(_adb: &Adb, port: u16) -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let mut buf = [0u8; 12];
    let n = stream.read(&mut buf).await?;
    let banner = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    if banner.starts_with("RFB ") {
        // 回一个客户端版本，完整走一次握手
        let _ = stream.write_all(b"RFB 003.008\n").await;
        Ok(format!(
            "127.0.0.1:{} 握手成功，服务端协议版本: {}",
            port, banner
        ))
    } else {
        Ok(format!(
            "127.0.0.1:{} 已连接，但返回的不是 VNC 握手串: {:?}",
            port, banner
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::check_adb,
            commands::set_adb_path,
            commands::list_devices,
            commands::check_avf,
            commands::dev_options,
            commands::open_dev_options,
            commands::enable_linux_env,
            commands::scan_security,
            commands::click_allow,
            commands::terminal_phase,
            commands::detect_linux_image,
            commands::install_linux_image,
            commands::open_terminal,
            commands::detect_installed,
            commands::run_install,
            commands::install_status,
            commands::cancel_install,
            commands::enable_port,
            commands::setup_vnc,
            commands::install_avnc,
            commands::capture_screen,
            commands::open_external,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
