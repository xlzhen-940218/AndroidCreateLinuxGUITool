//! VNC 相关：端口转发、TigerVNC 客户端下载/启动、AVNC 引导。

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::adb::Adb;

/// AVNC（Android VNC 客户端，https://github.com/gujjwal00/avnc）的应用 ID。
pub const AVNC_PACKAGE: &str = "com.gaurav.avnc";
/// GitHub 上没有可用的 APK 时，引导用户去 F-Droid 手动安装。
pub const AVNC_FDROID_URL: &str = "https://f-droid.org/packages/com.gaurav.avnc/";
/// AVNC 最新版本信息（GitHub Releases API）。
const AVNC_RELEASE_API: &str =
    "https://api.github.com/repos/gujjwal00/avnc/releases/latest";
pub const TIGERVNC_DOWNLOAD_URL: &str = "https://tigervnc.org/download/";

/// TigerVNC 查看器候选版本（新 → 旧）。
///
/// Windows 上用的是官方提供的 **免安装独立版** `vncviewer64-X.Y.Z.exe`：
/// 下载后直接就能运行，不需要管理员权限、不写注册表、也不会污染系统。
const TIGERVNC_WINDOWS_VERSIONS: [&str; 4] = ["1.16.0", "1.15.0", "1.14.0", "1.13.0"];

/// 端口转发 + 客户端启动的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VncSetup {
    pub port: u16,
    pub forward_ok: bool,
    pub forward_detail: String,
    pub viewer_found: bool,
    /// 客户端是本次自动下载得到的（而不是系统里本来就有的）。
    pub viewer_downloaded: bool,
    pub viewer_path: String,
    pub viewer_launched: bool,
    pub connect_hint: String,
    pub message: String,
}

/// 工具自己的数据目录（存放自动下载的查看器）。
pub fn tool_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("AndroidCreateLinuxGUITool")
}

/// 找到系统里的 curl（Windows 10 1803+ 自带 curl.exe，macOS/Linux 自带 curl）。
fn find_curl() -> Option<String> {
    let exe = if cfg!(windows) { "curl.exe" } else { "curl" };
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        let p = dir.join(exe);
        if p.is_file() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

/// 用 curl 查一下远端文件的大小（字节），拿不到返回 0。
async fn remote_size(curl: &str, url: &str) -> u64 {
    let mut cmd = tokio::process::Command::new(curl);
    cmd.arg("-sIL").arg("--max-time").arg("30").arg(url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(false);
    crate::adb::hide_console(&mut cmd);
    let out = match cmd.output().await {
        Ok(o) => o,
        Err(_) => return 0,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut best = 0u64;
    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            if let Some(v) = line.split(':').nth(1) {
                if let Ok(n) = v.trim().parse::<u64>() {
                    best = best.max(n);
                }
            }
        }
    }
    best
}

/// 下载文件到本地，返回实际字节数。
///
/// 直接用系统自带的 curl 把内容流到 stdout，再边写盘边统计字节，
/// 这样既能拿到精确的下载进度，也不必给程序引入 TLS 依赖。
async fn download_with_progress(
    curl: &str,
    url: &str,
    dest: &Path,
    label: &str,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<u64> {
    use tokio::io::AsyncWriteExt;

    let total = remote_size(curl, url).await;

    let mut cmd = tokio::process::Command::new(curl);
    cmd.arg("-L")
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--max-time")
        .arg("1800")
        .arg(url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(false);
    crate::adb::hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| anyhow!("启动 curl 失败: {}", e))?;
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("无法读取下载流"))?;
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| anyhow!("创建 {} 失败: {}", dest.display(), e))?;

    let mut got: u64 = 0;
    let mut last_pct: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    use tokio::io::AsyncReadExt;
    loop {
        let n = match stdout.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = child.kill().await;
                return Err(anyhow!("下载中断: {}", e));
            }
        };
        file.write_all(&buf[..n]).await?;
        got += n as u64;
        if total > 0 {
            let pct = got * 100 / total;
            if pct >= last_pct + 5 {
                last_pct = pct;
                progress(format!(
                    "正在下载 {}… {}% （{:.1} / {:.1} MB）",
                    label,
                    pct,
                    got as f64 / 1048576.0,
                    total as f64 / 1048576.0
                ));
            }
        }
    }
    let _ = file.flush().await;
    let status = child.wait().await;
    if let Ok(st) = status {
        if !st.success() {
            return Err(anyhow!("下载失败，curl 退出码 {:?}", st.code()));
        }
    }
    Ok(got)
}

/// 下载官方免安装版 TigerVNC Viewer 到指定路径，返回文件大小。
///
/// 版本按「新 → 旧」依次尝试，任一版本下载成功即返回。
pub async fn download_viewer(
    target: &Path,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<u64> {
    if !cfg!(windows) {
        bail!(
            "当前系统暂不支持自动下载客户端，请手动安装 TigerVNC Viewer：{}",
            TIGERVNC_DOWNLOAD_URL
        );
    }
    let curl = find_curl().ok_or_else(|| {
        anyhow!(
            "系统里没有 curl，无法自动下载客户端。请手动安装 TigerVNC Viewer：{}",
            TIGERVNC_DOWNLOAD_URL
        )
    })?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let mut last_err = String::new();
    for ver in TIGERVNC_WINDOWS_VERSIONS {
        let url = format!(
            "https://sourceforge.net/projects/tigervnc/files/stable/{0}/vncviewer64-{0}.exe/download",
            ver
        );
        progress(format!("正在下载 TigerVNC Viewer {} （约 24 MB）…", ver));
        match download_with_progress(&curl, &url, target, "TigerVNC Viewer", progress).await {
            Ok(size) if size > 1_000_000 => {
                progress(format!("TigerVNC Viewer 已下载到 {}", target.display()));
                return Ok(size);
            }
            Ok(size) => last_err = format!("下载内容不完整（{} 字节）", size),
            Err(e) => last_err = e.to_string(),
        }
    }
    bail!(
        "自动下载 TigerVNC Viewer 失败：{}。请手动下载安装：{}",
        last_err,
        TIGERVNC_DOWNLOAD_URL
    )
}

/// 确保本机有可用的 VNC 查看器。
///
/// 找不到已安装的 TigerVNC 时，自动从官方 SourceForge 下载**免安装独立版**，
/// 保存到用户目录下直接使用（无需管理员权限）。
pub async fn ensure_viewer(progress: &mut (dyn FnMut(String) + Send)) -> Result<PathBuf> {
    if let Some(p) = find_tigervnc_viewer() {
        return Ok(p);
    }

    let dir = tool_data_dir();
    let target = dir.join("vncviewer.exe");
    if target.is_file()
        && std::fs::metadata(&target)
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(false)
    {
        return Ok(target);
    }

    download_viewer(&target, progress).await?;
    Ok(target)
}

/// 建立 `adb forward`，把 PC 的本地端口指向设备的同名端口。
pub async fn setup_forward(adb: &Adb, port: u16) -> Result<(bool, String)> {
    let local = format!("tcp:{}", port);
    let remote = format!("tcp:{}", port);
    match adb.forward(&local, &remote).await {
        Ok(()) => {
            let list = adb.forward_list().await.unwrap_or_default();
            if list.contains(&local) {
                Ok((true, format!("已建立转发 {} -> {}", local, remote)))
            } else {
                Ok((true, format!("已执行转发 {} -> {}", local, remote)))
            }
        }
        Err(e) => Ok((false, format!("端口转发失败: {}", e))),
    }
}

/// 查找本机的 TigerVNC Viewer。
pub fn find_tigervnc_viewer() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        let mut roots: Vec<PathBuf> = Vec::new();
        for key in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(v) = std::env::var(key) {
                if v.trim().is_empty() {
                    continue;
                }
                let p = PathBuf::from(v);
                roots.push(p.join("TigerVNC").join("vncviewer.exe"));
                roots.push(p.join("Tigervnc").join("vncviewer.exe"));
                roots.push(p.join("Programs").join("TigerVNC").join("vncviewer.exe"));
            }
        }
        candidates.extend(roots);
        // 自动下载的免安装版
        candidates.push(tool_data_dir().join("vncviewer.exe"));
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/TigerVNC.app/Contents/MacOS/vncviewer"));
        candidates.push(PathBuf::from(
            "/Applications/TigerVNC Community Edition.app/Contents/MacOS/vncviewer",
        ));
    }

    // PATH 查找
    if let Ok(path) = std::env::var("PATH") {
        let exe = if cfg!(windows) { "vncviewer.exe" } else { "vncviewer" };
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(exe));
        }
    }

    candidates.into_iter().find(|p| p.is_file())
}

/// 启动 TigerVNC Viewer 并连接到 127.0.0.1:<port>。
///
/// 地址与端口通过命令行参数直接给出（TigerVNC 的 `host::port` 语法表示显式端口），
/// 客户端打开即自动填好地址并发起连接，不需要用户再手动输入。
///
/// `passwd` 非空时会额外传入 `-passwd <文件>`：TigerVNC 会用这个文件里的密码
/// 自动完成 VNC 认证，用户连密码框都不用看到。
pub async fn launch_tigervnc_at(
    viewer: &Path,
    port: u16,
    passwd: Option<&Path>,
) -> Result<(bool, String, String)> {
    // 额外写一个 .vnc 配置文件，方便用户以后手动打开
    let dir = std::env::temp_dir().join("android-linux-vnc");
    std::fs::create_dir_all(&dir).ok();
    let cfg = dir.join("android-linux.vnc");
    let content = format!(
        "[connection]\nhost=127.0.0.1\nport={}\n\n[security]\ntype=VncAuth\n",
        port
    );
    std::fs::write(&cfg, content).ok();

    let mut cmd = tokio::process::Command::new(viewer);
    // 注意：`host:1` 里的 1 是 display 号（= 端口 5900+1），
    // 要指定真实端口必须用双冒号语法 `host::port`。
    if let Some(p) = passwd {
        cmd.arg("-passwd").arg(p);
    }
    cmd.arg(format!("127.0.0.1::{}", port));
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(false);
    crate::adb::hide_console(&mut cmd);

    match cmd.spawn() {
        Ok(_) => Ok((
            true,
            viewer.to_string_lossy().to_string(),
            format!(
                "已启动 TigerVNC Viewer，地址与端口已自动填好，正在连接 127.0.0.1:{}",
                port
            ),
        )),
        Err(e) => Ok((
            false,
            viewer.to_string_lossy().to_string(),
            format!("启动 TigerVNC Viewer 失败: {}", e),
        )),
    }
}

/// 生成 TigerVNC 的 passwd 文件内容。
///
/// TigerVNC 存的不是加密密文，而是**每个字节按位取反后的密码**
/// （不足 8 字节补 0，只有前 8 个字符有效）——这正是 VNC 认证里 DES 的密钥形式。
/// 所以 PC 端可以直接算出与虚拟机里 `vncpasswd` 生成的一模一样的文件。
///
/// 例：密码 `linux` → `36 96 76 ae 1e 00 00 00`（真机已核对）。
pub fn vnc_passwd_bytes(password: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, b) in password.as_bytes().iter().take(8).enumerate() {
        out[i] = b.reverse_bits();
    }
    out
}

/// 把密码写成一个 passwd 文件，返回文件路径（供查看器 `-passwd` 使用）。
pub fn write_passwd_file(password: &str) -> Result<PathBuf> {
    if password.trim().is_empty() {
        bail!("VNC 密码为空，无法生成自动认证用的密码文件。");
    }
    let dir = tool_data_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("创建 {} 失败: {}", dir.display(), e))?;
    let path = dir.join("vncpasswd");
    std::fs::write(&path, vnc_passwd_bytes(password))
        .map_err(|e| anyhow!("写入 {} 失败: {}", path.display(), e))?;
    Ok(path)
}

/// 启动 TigerVNC Viewer（自动使用已安装或已下载的客户端）。
pub async fn launch_tigervnc(port: u16, password: &str) -> Result<(bool, String, String)> {
    match find_tigervnc_viewer() {
        Some(p) => {
            let passwd = write_passwd_file(password).ok();
            launch_tigervnc_at(&p, port, passwd.as_deref()).await
        }
        None => Ok((
            false,
            String::new(),
            format!(
                "未检测到本机安装 TigerVNC Viewer，请先从 {} 安装后再一键打开。",
                TIGERVNC_DOWNLOAD_URL
            ),
        )),
    }
}

/// 完整流程（带进度回调，用于在下载客户端时给前端反馈）。
pub async fn setup_with(
    adb: &Adb,
    port: u16,
    password: &str,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<VncSetup> {
    let (forward_ok, forward_detail) = setup_forward(adb, port).await?;

    // 客户端：优先用系统里已安装的，没有就自动下载免安装版
    let installed = find_tigervnc_viewer();
    let (viewer_path, downloaded) = match installed {
        Some(p) => (p, false),
        None => match ensure_viewer(progress).await {
            Ok(p) => (p, true),
            Err(e) => {
                return Ok(VncSetup {
                    port,
                    forward_ok,
                    forward_detail,
                    viewer_found: false,
                    viewer_downloaded: false,
                    viewer_path: String::new(),
                    viewer_launched: false,
                    connect_hint: format!("127.0.0.1:{}", port),
                    message: format!(
                        "{}\n\n也可以先在手机上使用 AVNC 连接（地址 127.0.0.1，端口 {}）。",
                        e, port
                    ),
                })
            }
        },
    };

    // 生成自动认证用的密码文件：查看器用 -passwd 免输入完成 VNC 认证
    let passwd_file = write_passwd_file(password).ok();
    let (launched, viewer_path, msg) =
        launch_tigervnc_at(&viewer_path, port, passwd_file.as_deref()).await?;

    let message = if !forward_ok {
        format!("{}（请先在 Terminal 应用中放行端口 {}）", forward_detail, port)
    } else if downloaded {
        format!("{}（TigerVNC Viewer 已自动下载安装）", msg)
    } else {
        msg
    };

    Ok(VncSetup {
        port,
        forward_ok,
        forward_detail,
        viewer_found: !viewer_path.is_empty(),
        viewer_downloaded: downloaded,
        viewer_path,
        viewer_launched: launched,
        connect_hint: format!("127.0.0.1:{}", port),
        message,
    })
}

/// AVNC 是否已经装在手机上。
pub async fn avnc_installed(adb: &Adb) -> bool {
    adb.shell(&format!("pm path {}", AVNC_PACKAGE))
        .await
        .map(|s| s.contains("package:"))
        .unwrap_or(false)
}

/// 打开手机上的 AVNC。
async fn launch_avnc(adb: &Adb) -> bool {
    adb.shell(&format!(
        "monkey -p {} -c android.intent.category.LAUNCHER 1",
        AVNC_PACKAGE
    ))
    .await
    .map(|s| !s.contains("No activities found"))
    .unwrap_or(false)
}

/// 从 GitHub Releases API 里取出最新 APK 的版本号与下载地址。
fn find_avnc_apk(json: &str) -> Option<(String, String)> {
    fn field(json: &str, key: &str) -> Option<String> {
        let pat = format!("\"{}\"", key);
        let start = json.find(&pat)? + pat.len();
        let rest = &json[start..];
        let rest = &rest[rest.find(':')? + 1..];
        let rest = &rest[rest.find('"')? + 1..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }

    let tag = field(json, "tag_name").unwrap_or_else(|| "最新版".to_string());
    let mut offset = 0usize;
    while let Some(pos) = json[offset..].find("\"browser_download_url\"") {
        let at = offset + pos;
        if let Some(url) = field(&json[at..], "browser_download_url") {
            if url.ends_with(".apk") {
                return Some((tag, url));
            }
        }
        offset = at + 1;
    }
    None
}

/// 自动把 AVNC 安装到手机上：优先用 GitHub Releases 的最新 APK，
/// 已安装则直接打开；任何一步失败都回退到 F-Droid 下载页引导手动安装。
pub async fn install_avnc(
    adb: &Adb,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<String> {
    const HINT: &str = "请在 AVNC 中填写：地址 127.0.0.1，端口 5901。";

    if avnc_installed(adb).await {
        let launched = launch_avnc(adb).await;
        return Ok(if launched {
            format!("AVNC 已安装，已在手机上打开。\n{}", HINT)
        } else {
            format!("AVNC 已安装，请在手机桌面打开它。\n{}", HINT)
        });
    }

    async fn manual(adb: &Adb, reason: String) -> Result<String> {
        let _ = adb.start_view(AVNC_FDROID_URL).await;
        Ok(format!(
            "{}。已打开 F-Droid 下载页，可在手机上手动安装。\n地址 127.0.0.1，端口 5901。",
            reason
        ))
    }

    let curl = match find_curl() {
        Some(c) => c,
        None => {
            return manual(adb, "系统里没有 curl，无法自动下载 AVNC 安装包".to_string()).await;
        }
    };

    progress("正在查询 AVNC 最新版本…".to_string());
    let json = match curl_get(&curl, AVNC_RELEASE_API).await {
        Ok(j) => j,
        Err(e) => return manual(adb, format!("查询 AVNC 最新版本失败：{}", e)).await,
    };
    let (tag, url) = match find_avnc_apk(&json) {
        Some(v) => v,
        None => return manual(adb, "GitHub Releases 中未找到 APK".to_string()).await,
    };

    let apk = tool_data_dir().join("avnc.apk");
    progress(format!("正在下载 AVNC {} …", tag));
    let size = match download_with_progress(&curl, &url, &apk, "AVNC", progress).await {
        Ok(n) => n,
        Err(e) => return manual(adb, format!("下载 AVNC 失败：{}", e)).await,
    };
    if size < 1_000_000 {
        return manual(adb, format!("下载的 AVNC 安装包不完整（{} 字节）", size)).await;
    }

    progress("正在把 AVNC 安装到手机…".to_string());
    let out = adb
        .raw(&["install", "-r", &apk.to_string_lossy()])
        .await
        .map_err(|e| anyhow!("调用 adb install 失败: {}", e))?;
    let text = out.combined();
    if !out.ok() || !text.to_lowercase().contains("success") {
        return manual(adb, format!("自动安装失败：{}", text.trim())).await;
    }

    let launched = launch_avnc(adb).await;
    progress("AVNC 安装完成。".to_string());
    Ok(format!(
        "AVNC {} 已安装到手机{}。\n地址 127.0.0.1，端口 5901。",
        tag,
        if launched { "并已打开" } else { "" }
    ))
}

/// 用 curl 取一段文本（用于 GitHub API）。
async fn curl_get(curl: &str, url: &str) -> Result<String> {
    let mut cmd = tokio::process::Command::new(curl);
    cmd.arg("-sL")
        .arg("--fail")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--max-time")
        .arg("60")
        .arg("-H")
        .arg("Accept: application/vnd.github+json")
        .arg("-A")
        .arg("AndroidCreateLinuxGUITool")
        .arg(url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(false);
    crate::adb::hide_console(&mut cmd);
    let out = cmd.output().await.map_err(|e| anyhow!("启动 curl 失败: {}", e))?;
    if !out.status.success() {
        bail!("curl 退出码 {:?}", out.status.code());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 在 PC 上用系统默认浏览器打开链接。
pub fn open_url(url: &str) -> Result<()> {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    crate::adb::hide_console_std(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| anyhow!("打开链接失败: {}", e))
}
