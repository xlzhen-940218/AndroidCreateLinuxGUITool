//! VNC 相关：手机端通道、TigerVNC 客户端、AVNC 引导。
//!
//! 连接链路：
//!   PC 客户端 → 本机 127.0.0.1:5901
//!              → adb forward
//!              → 手机 127.0.0.1:5901（本工具拉起的中继，或 Terminal 的端口放行）
//!              → 虚拟机 5901（Xtigervnc）
//!
//! 客户端（TigerVNC Viewer）随程序内置，不再依赖联网下载；手机端 AVNC 同样
//! 优先使用内置 APK，只有内置安装失败时才回退到 GitHub / F-Droid。

use anyhow::{anyhow, bail, Result};
use cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use des::Des;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::adb::Adb;
use crate::{apk, util};

/// AVNC（Android VNC 客户端，https://github.com/gujjwal00/avnc）的应用 ID。
pub const AVNC_PACKAGE: &str = "com.gaurav.avnc";
/// GitHub 上没有可用的 APK 时，引导用户去 F-Droid 手动安装。
pub const AVNC_FDROID_URL: &str = "https://f-droid.org/packages/com.gaurav.avnc/";
/// AVNC 最新版本信息（GitHub Releases API）。
const AVNC_RELEASE_API: &str = "https://api.github.com/repos/gujjwal00/avnc/releases/latest";
/// 随程序内置的 AVNC 安装包（离线可用，避免国内下载 GitHub 失败）。
const BUNDLED_AVNC_APK: &[u8] = include_bytes!("../resources/avnc.apk");
/// 内置安装包对应的版本号，仅用于提示文案。
const BUNDLED_AVNC_TAG: &str = "3.3.1";
/// 随程序内置的 TigerVNC Viewer（官方免安装独立版 1.16.0，离线可用）。
/// 内置的只有 Windows 版；Linux 交给发行版的 `tigervnc-viewer` 包（deb 依赖里已声明）。
#[cfg(windows)]
const BUNDLED_VIEWER: &[u8] = include_bytes!("../resources/vncviewer.exe");
/// VNC 固定端口，与 `install-desktop.sh` 中的 `VNC_PORT` 保持一致。
pub const VNC_PORT: u16 = 5901;

/// 端口转发 + 客户端启动的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VncSetup {
    pub port: u16,
    pub forward_ok: bool,
    pub forward_detail: String,
    /// 手机端把端口接到虚拟机的说明（走的是哪条通道）。
    pub relay_detail: String,
    /// 是否真的能连上 VNC 服务（完成 RFB 认证握手）。
    pub reachable: bool,
    /// 探测详情（成功或失败原因）。
    pub probe_detail: String,
    pub viewer_found: bool,
    pub viewer_path: String,
    pub viewer_launched: bool,
    pub connect_hint: String,
    pub message: String,
}

impl VncSetup {
    fn new(port: u16, connect_hint: String) -> Self {
        Self {
            port,
            forward_ok: false,
            forward_detail: String::new(),
            relay_detail: String::new(),
            reachable: false,
            probe_detail: String::new(),
            viewer_found: false,
            viewer_path: String::new(),
            viewer_launched: false,
            connect_hint,
            message: String::new(),
        }
    }
}

/* ------------------------------ VNC 认证探测 ------------------------------ */

/// 探测 VNC 端口是否真的可用：用给定密码做一次完整的 VncAuth 握手。
///
/// 只读 12 字节 `RFB 003.008` 不能说明问题：服务端密码配错、没有启用密码认证
/// 时同样会回握手串。这里走完「选择安全类型 → 解挑战 → 校验 SecurityResult」，
/// 能直接区分「密码不对」「失败次数过多被临时封禁」「没开 VncAuth」。
///
/// `attempts` > 1 时会重试：端口刚放行或虚拟机刚启动时，转发链路可能还没生效。
pub async fn probe_vnc(port: u16, password: &str, attempts: u32) -> (bool, String) {
    let mut last = String::new();
    for i in 0..attempts {
        match probe_auth_once(port, password).await {
            Ok(msg) => return (true, msg),
            Err(e) => {
                last = e.to_string();
                // 认证类失败重试没有意义，而且每次失败都会累加服务端的封禁计数。
                if is_auth_failure(&last) {
                    break;
                }
            }
        }
        if i + 1 < attempts {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    (false, last)
}

fn is_auth_failure(detail: &str) -> bool {
    detail.contains("认证") || detail.contains("安全类型")
}

async fn probe_auth_once(port: u16, password: &str) -> Result<String> {
    use tokio::io::AsyncWriteExt;

    let mut stream = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map_err(|_| anyhow!("连接超时"))?
    .map_err(|e| anyhow!("连接被拒绝或不可达：{}", e))?;

    let mut banner = [0u8; 12];
    read_exact(&mut stream, &mut banner, "等待 VNC 握手超时").await?;
    let version = String::from_utf8_lossy(&banner).trim().to_string();
    if !version.starts_with("RFB ") {
        bail!("端口有响应，但返回的不是 VNC 协议：{:?}", version);
    }
    stream.write_all(b"RFB 003.008\n").await?;

    let mut count = [0u8; 1];
    read_exact(&mut stream, &mut count, "等待安全类型超时").await?;
    if count[0] == 0 {
        let reason = read_reason(&mut stream).await.unwrap_or_default();
        bail!(
            "{}",
            if reason.is_empty() {
                "服务端拒绝连接".into()
            } else {
                reason
            }
        );
    }
    let mut types = vec![0u8; count[0] as usize];
    read_exact(&mut stream, &mut types, "读取安全类型超时").await?;
    if !types.contains(&2) {
        bail!(
            "服务端没有启用密码认证（VncAuth），可用的安全类型：{:?}",
            types
        );
    }
    stream.write_all(&[2]).await?;

    let mut challenge = [0u8; 16];
    read_exact(&mut stream, &mut challenge, "等待认证挑战超时").await?;
    stream
        .write_all(&vnc_auth_response(password, &challenge))
        .await?;

    let mut result = [0u8; 4];
    read_exact(&mut stream, &mut result, "等待认证结果超时").await?;
    if u32::from_be_bytes(result) == 0 {
        return Ok(format!("VNC 密码认证成功（服务端版本 {}）", version));
    }
    let reason = read_reason(&mut stream).await.unwrap_or_default();
    if reason.to_lowercase().contains("too many") {
        bail!(
            "服务端拒绝了本次认证：{}。短时间内失败次数过多会触发临时封禁，\
             请重跑「TigerVNC 服务」或等 60 秒后再试。",
            reason
        );
    }
    bail!(
        "VNC 密码认证失败：{}（界面里的密码与服务端不一致）",
        if reason.is_empty() {
            "密码错误".to_string()
        } else {
            reason
        }
    );
}

async fn read_exact<S>(stream: &mut S, buf: &mut [u8], timeout_msg: &str) -> Result<()>
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(buf))
        .await
        .map_err(|_| anyhow!("{}", timeout_msg))?
        .map_err(|e| anyhow!("读取认证数据失败：{}", e))?;
    Ok(())
}

async fn read_reason<S>(stream: &mut S) -> Result<String>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    read_exact(stream, &mut len, "读取拒绝原因超时").await?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > 4096 {
        return Ok(String::new());
    }
    let mut buf = vec![0u8; n];
    read_exact(stream, &mut buf, "读取拒绝原因超时").await?;
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

/* -------------------------------- DES 工具 -------------------------------- */

/// 用标准 DES 加密若干 8 字节块。
///
/// TigerVNC 的 d3des 与标准 DES 只差在**密钥字节的位序相反**（每个字节按位
/// 取反），数据部分完全一致。所以任何需要 d3des 的地方，都先用
/// [`reversed_key`] 处理密钥，再交给标准 DES。
fn des_encrypt(key: &[u8; 8], data: &[u8]) -> Vec<u8> {
    let cipher = Des::new(GenericArray::from_slice(key));
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(8) {
        if chunk.len() != 8 {
            break;
        }
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    out
}

fn reversed_key(bytes: &[u8]) -> [u8; 8] {
    let mut key = [0u8; 8];
    for (i, b) in bytes.iter().take(8).enumerate() {
        key[i] = b.reverse_bits();
    }
    key
}

/// VncAuth 的挑战响应：用密码（补 0 到 8 字节）作为 d3des 密钥，加密 16 字节挑战。
///
/// 服务端真正用的密钥是**明文密码**，不是 `passwd` 文件里的混淆字节。
/// 客户端认证失败最常见的原因就是把混淆后的 8 字节当成了密钥。
fn vnc_auth_response(password: &str, challenge: &[u8]) -> [u8; 16] {
    let key = reversed_key(password.as_bytes());
    let out = des_encrypt(&key, challenge);
    let mut response = [0u8; 16];
    response.copy_from_slice(&out[..16]);
    response
}

/// 生成 TigerVNC 的 passwd 文件内容。
///
/// TigerVNC 先把明文密码补 0 到 8 字节，再用固定密钥
/// `{23,82,107,6,35,78,88,7}` 做一次 d3des 加密（见 TigerVNC
/// `common/rfb/obfuscate.cxx`）。
///
/// 实测：密码 `linux` → `54 6d 3b 7b 9a 13 56 cc`（真机 `vncpasswd -f` 输出）。
pub fn vnc_passwd_bytes(password: &str) -> [u8; 8] {
    const OBFUSCATION_KEY: [u8; 8] = [23, 82, 107, 6, 35, 78, 88, 7];

    let key = reversed_key(&OBFUSCATION_KEY);
    let mut plain = [0u8; 8];
    for (i, b) in password.as_bytes().iter().take(8).enumerate() {
        plain[i] = *b;
    }

    let out = des_encrypt(&key, &plain);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&out);
    bytes
}

/* ------------------------------ 客户端与密码 ------------------------------ */

/// 把内置的 TigerVNC Viewer 解压到工具数据目录，返回可执行文件路径。
#[cfg(windows)]
pub fn bundled_viewer() -> Option<PathBuf> {
    util::write_bundled_file("vncviewer.exe", BUNDLED_VIEWER)
}

/// 非 Windows 平台没有内置 Viewer，使用系统里安装的版本。
#[cfg(not(windows))]
pub fn bundled_viewer() -> Option<PathBuf> {
    None
}

/// 查找系统里已安装的 TigerVNC Viewer（不包含内置版本）。
fn find_installed_viewer() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        for key in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(v) = std::env::var(key) {
                if v.trim().is_empty() {
                    continue;
                }
                let p = PathBuf::from(v);
                candidates.push(p.join("TigerVNC").join("vncviewer.exe"));
                candidates.push(p.join("Tigervnc").join("vncviewer.exe"));
                candidates.push(p.join("Programs").join("TigerVNC").join("vncviewer.exe"));
            }
        }
        // 历史版本自动下载到数据目录的免安装版
        candidates.push(util::tool_data_dir().join("vncviewer.exe"));
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from("/Applications/TigerVNC.app/Contents/MacOS/vncviewer"));
        candidates.push(PathBuf::from(
            "/Applications/TigerVNC Community Edition.app/Contents/MacOS/vncviewer",
        ));
    } else {
        // Linux：Debian/Ubuntu 的 tigervnc-viewer 包同时提供 vncviewer 与 xtigervncviewer
        for p in [
            "/usr/bin/vncviewer",
            "/usr/bin/xtigervncviewer",
            "/usr/local/bin/vncviewer",
            "/usr/local/bin/xtigervncviewer",
            "/snap/bin/vncviewer",
            "/var/lib/flatpak/exports/bin/com.tigervnc.vncviewer",
        ] {
            candidates.push(PathBuf::from(p));
        }
    }

    if let Ok(path) = std::env::var("PATH") {
        let exe = if cfg!(windows) { "vncviewer.exe" } else { "vncviewer" };
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(exe));
        }
    }

    candidates.into_iter().find(|p| p.is_file())
}

/// 查找本机的 TigerVNC Viewer。
///
/// 优先返回程序内置的版本（离线可用、版本固定），其次才是系统里已安装的。
pub fn find_tigervnc_viewer() -> Option<PathBuf> {
    bundled_viewer().or_else(find_installed_viewer)
}

/// 启动 TigerVNC Viewer 并连接到 127.0.0.1:<port>。
///
/// 地址与端口通过命令行参数直接给出（TigerVNC 的 `host::port` 语法表示显式端口）。
/// `passwd` 非空时额外传入 `-passwd <文件>`，客户端会用文件里的密码自动完成认证。
pub async fn launch_tigervnc_at(
    viewer: &Path,
    port: u16,
    passwd: Option<&Path>,
) -> Result<(bool, String, String)> {
    let address = format!("127.0.0.1::{}", port);
    let mut args: Vec<String> = Vec::new();
    if let Some(p) = passwd {
        args.push("-passwd".to_string());
        args.push(p.to_string_lossy().to_string());
    }
    args.push(address);
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    match util::spawn_detached(viewer, &args) {
        Ok(()) => Ok((
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

/// 把密码写成一个 passwd 文件，返回文件路径（供查看器 `-passwd` 使用）。
pub fn write_passwd_file(password: &str) -> Result<PathBuf> {
    if password.trim().is_empty() {
        bail!("VNC 密码为空，无法生成自动认证用的密码文件。");
    }
    let dir = util::tool_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("创建 {} 失败: {}", dir.display(), e))?;
    let path = dir.join("vncpasswd");
    std::fs::write(&path, vnc_passwd_bytes(password))
        .map_err(|e| anyhow!("写入 {} 失败: {}", path.display(), e))?;
    Ok(path)
}

/* ------------------------------ 虚拟机内修复 ------------------------------ */

/// 认证失败或 VNC 服务没起来时，自动修复虚拟机里的 VNC 服务。
///
/// 背景：Debian 的 TigerVNC 包装脚本（`tigervncserver` / `tigervncsession`）在
/// 认为密码文件“不可用”时，会先删掉它再非交互地调用 `vncpasswd` —— 非交互环境
/// 下读不到密码，只会把文件截断成 0 字节。这里统一改成：
///   1) 用 `vncpasswd -f` 重新生成**校验过的 8 字节**密码文件；
///   2) 重启本工具自己的 `acl-vnc.service`（它直接跑 Xtigervnc，不经过包装脚本）；
///   3) 等端口真正开始监听再返回成功。
async fn repair_guest_vnc(adb: &Adb, password: &str, port: u16) -> Result<String> {
    let pw = format!("'{}'", password.replace('\'', "'\\''"));
    let script = r#"
# 注意：这段脚本会被 guest_exec 包进一个更大的脚本里，
# 不能用 exit（会跳过外层的 __EXIT__/__DONE__ 标记），统一用函数 + return。
repair_vnc() {
  if ! command -v vncpasswd >/dev/null 2>&1; then echo "NO_VNCPASSWD"; return 1; fi
  mkdir -p "$HOME/.vnc" "$HOME/.config/tigervnc"
  TMP="$(mktemp /tmp/acl_fix_pw.XXXXXX 2>/dev/null || echo /tmp/acl_fix_pw.$$)"
  printf '%s\n' {PW} | vncpasswd -f > "$TMP" 2>/dev/null || true
  if [ "$(wc -c < "$TMP" 2>/dev/null | tr -d ' ')" != "8" ]; then
    echo "PW_BAD"; rm -f "$TMP" 2>/dev/null || true; return 1
  fi
  cp "$TMP" "$HOME/.vnc/passwd" && chmod 600 "$HOME/.vnc/passwd"
  cp "$TMP" "$HOME/.config/tigervnc/passwd" && chmod 600 "$HOME/.config/tigervnc/passwd"
  rm -f "$TMP" 2>/dev/null || true

  port_listening() {
    (ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -q ":{PORT} "
  }
  if command -v systemctl >/dev/null 2>&1; then
    sudo -n systemctl reset-failed acl-vnc.service >/dev/null 2>&1 || true
    sudo -n systemctl restart acl-vnc.service >/dev/null 2>&1 || true
  fi
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if port_listening; then echo "RESTARTED"; return 0; fi
    sleep 0.5
  done
  echo "NO_LISTEN"
  return 1
}
repair_vnc
"#
    .replace("{PORT}", &port.to_string())
    .replace("{PW}", &pw);

    crate::installer::guest_exec(adb, &script).await
}

/// 自动修复前先确认 Terminal 真的可用。
///
/// `guest_exec` 依赖软键盘输入；手机锁屏、Terminal 不在前台时注入会被丢弃，
/// 函数会一直等到超时（几分钟）。这里先快速失败，把下一步该做什么直接告诉用户。
async fn prepare_guest_repair(adb: &Adb) -> bool {
    let _ = crate::terminal::open(adb).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    matches!(
        crate::terminal::phase(adb).await,
        Ok(crate::terminal::TerminalPhase::Ready)
    )
}

/// 调用一次修复；`repaired` 保证整个流程最多自动重启一次 VNC 服务。
async fn try_repair(
    adb: &Adb,
    password: &str,
    port: u16,
    repaired: &mut bool,
) -> (bool, String) {
    if *repaired {
        return (false, String::new());
    }
    *repaired = true;
    if !prepare_guest_repair(adb).await {
        return (
            false,
            "（Terminal 不在前台或尚未就绪，无法自动启动 VNC 服务；\
             请解锁手机并打开 Terminal，或重跑「TigerVNC 服务」步骤）"
                .to_string(),
        );
    }
    match repair_guest_vnc(adb, password, port).await {
        Ok(out) if out.contains("RESTARTED") => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            (true, String::new())
        }
        Ok(out) => {
            let note = out
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with("__"))
                .unwrap_or("VNC 服务未能启动")
                .trim()
                .to_string();
            (false, format!("（自动启动 VNC 服务未生效：{}）", note))
        }
        Err(e) => (false, format!("（自动启动 VNC 服务失败：{}）", e)),
    }
}

/* ------------------------------ 主流程 ------------------------------ */

/// 建立 `adb forward`：PC 的本地端口 → 手机上真正提供 VNC 的端口。
///
/// 两个端口不一定相同：Redmi K Pad 的 5901 被 Terminal 的僵尸转发占着，
/// 中继会改用 15901，于是 PC 端仍然是 `127.0.0.1:5901`，转发目标却是 15901。
pub async fn setup_forward(
    adb: &Adb,
    local_port: u16,
    device_port: u16,
) -> Result<(bool, String)> {
    let local = format!("tcp:{}", local_port);
    let remote = format!("tcp:{}", device_port);
    // 同一条 forward 可能上一轮指向别的设备端口，先移除再建立，避免 rebind 冲突。
    let _ = adb.raw(&["forward", "--remove", &local]).await;
    match adb.forward(&local, &remote).await {
        Ok(()) => Ok((
            true,
            if local_port == device_port {
                format!("已建立转发 {} -> {}", local, remote)
            } else {
                format!("已建立转发 {} -> {}（中继端口）", local, remote)
            },
        )),
        Err(e) => Ok((false, format!("端口转发失败: {}", e))),
    }
}

/// 建立手机端通道：优先用中继，中继不可用时才退回 Terminal 的端口放行。
///
/// 返回 `(是否可用, 说明, 手机端实际端口)`。第三个值给 `adb forward` 用：
/// 正常情况就是 `port`（5901）；Redmi K Pad 的 5901 被 Terminal 的僵尸转发
/// 占着时，会是中继使用的 15901。
async fn ensure_channel(adb: &Adb, port: u16) -> (bool, String, u16) {
    // 公共端口已经能读到 RFB（Terminal 转发正常，或上一轮的中继就在该端口）。
    if crate::relay::device_serves_vnc(adb, "127.0.0.1", port).await {
        return (
            true,
            format!("手机端已在 :{} 上转发到虚拟机，复用现有通道", port),
            port,
        );
    }

    // 虚拟机里的 VNC 必须先真的在监听，否则 Terminal 放行也帮不上忙。
    let guest = match crate::relay::find_vnc_guest(adb, port).await {
        Ok(ip) => ip,
        Err(e) => return (false, e.to_string(), port),
    };

    // 选一个不会被 Terminal 僵尸转发占用的设备端口中继。
    let relay_port = crate::relay::choose_relay_port(adb, port).await;

    // 上一轮的中继还在工作 → 直接复用。
    if crate::relay::device_serves_vnc(adb, "127.0.0.1", relay_port).await {
        return (
            true,
            format!(
                "手机端中继已在 :{} 上转发到虚拟机 {}:{}",
                relay_port, guest, port
            ),
            relay_port,
        );
    }

    match crate::relay::start_relay(adb, relay_port, &guest, port).await {
        Ok(msg) => (true, msg, relay_port),
        Err(e) => {
            let first = e.to_string();
            // 中继起不来时才退回 Terminal 的端口放行；放行后必须在公共端口上
            // 真正读到 RFB 才算成功。
            match crate::ports::enable_port(adb, port).await {
                Ok(r) if r.ok => {
                    if crate::relay::device_serves_vnc(adb, "127.0.0.1", port).await {
                        (true, format!("已通过 Terminal 放行端口 {}（{}）", port, r.message), port)
                    } else {
                        (
                            false,
                            format!(
                                "手机端中继失败（{}）；Terminal 已放行端口 {}，但仍收不到 VNC 握手",
                                first, port
                            ),
                            port,
                        )
                    }
                }
                Ok(r) => (
                    false,
                    format!(
                        "手机端中继失败（{}）；Terminal 放行也失败：{}",
                        first, r.message
                    ),
                    port,
                ),
                Err(e2) => (
                    false,
                    format!("手机端中继失败（{}）；Terminal 放行也失败：{}", first, e2),
                    port,
                ),
            }
        }
    }
}

fn failure_message(
    port: u16,
    forward_ok: bool,
    channel_ok: bool,
    probe_detail: &str,
    detail: &str,
) -> String {
    let hint = if !channel_ok {
        "手机端没有建立到虚拟机的端口通道。\n\n"
    } else if !forward_ok {
        "adb 端口转发没有建立（请重新插拔 USB 后重试）。\n\n"
    } else if probe_detail.contains("失败次数过多") {
        "刚才的多次失败触发了服务端的临时封禁，VNC 本身是起来的。\n\n"
    } else if probe_detail.contains("密码认证失败") {
        "服务端要求密码认证，但界面里的密码和服务端不一致。\n\n"
    } else if probe_detail.contains("没有启用密码认证") {
        "服务端没有启用 VncAuth 密码认证，请重跑「TigerVNC 服务」步骤。\n\n"
    } else if probe_detail.contains("拒绝") || probe_detail.contains("不可达") {
        "手机上的该端口没有监听 —— 多半是虚拟机还没起来，或者 VNC 服务没有启动。\n\n"
    } else {
        ""
    };
    format!(
        "{}无法连接 127.0.0.1:{}：{}\n\n\
         常见原因：\n\
         1) 虚拟机里的 TigerVNC 没有启动 —— 请回到「TigerVNC 服务」步骤重跑，\n\
            并查看执行日志里的失败原因；\n\
         2) 端口通道没有建立 —— 确认手机上的 Linux 虚拟机还在运行，\n\
            然后重新点一次上面的按钮；\n\
         3) 端口刚建立、转发尚未生效 —— 等几秒后重试。",
        hint, port, detail
    )
}

/// 完整流程（带进度回调，用于在前端展示每一步）。
pub async fn setup_with(
    adb: &Adb,
    port: u16,
    password: &str,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<VncSetup> {
    let connect_hint = format!("127.0.0.1:{}", port);
    let mut recovered = VncSetup::new(port, connect_hint.clone());
    let mut repaired = false;

    // 第一步：把手机上的端口接到虚拟机。
    progress("正在建立手机端端口通道…".to_string());
    let (mut channel_ok, mut relay_detail, mut device_port) = ensure_channel(adb, port).await;
    progress(relay_detail.clone());

    // 通道建不起来时，最常见的根因是虚拟机重启后 VNC 服务没起来。
    if !channel_ok {
        progress("虚拟机里的 VNC 服务没有响应，正在尝试自动启动…".to_string());
        let (ok, note) = try_repair(adb, password, port, &mut repaired).await;
        if ok {
            let (channel, detail, relay) = ensure_channel(adb, port).await;
            channel_ok = channel;
            relay_detail = detail;
            device_port = relay;
        } else if !note.is_empty() {
            relay_detail = format!("{}{}", relay_detail, note);
        }
        progress(relay_detail.clone());
    }

    let (forward_ok, forward_detail) = setup_forward(adb, port, device_port).await?;

    // 先验证链路，再决定要不要拉起客户端。
    progress("正在检测 VNC 服务是否可连接…".to_string());
    let (mut reachable, mut probe_detail) = probe_vnc(port, password, 3).await;

    // 认证失败 → 重设密码并重启服务，再复测一次。
    if !reachable && is_auth_failure(&probe_detail) {
        progress("VNC 密码认证失效，正在重设密码并重启服务…".to_string());
        let (ok, note) = try_repair(adb, password, port, &mut repaired).await;
        if ok {
            let (ok, detail) = probe_vnc(port, password, 4).await;
            reachable = ok;
            probe_detail = detail;
            if ok {
                progress("密码已重设，VNC 连接恢复正常。".to_string());
            }
        } else if !note.is_empty() {
            probe_detail = format!("{}{}", note, probe_detail);
        }
    }

    // 端口不可达（不是密码问题）也尝试自动拉起一次 VNC 服务。
    if !reachable && !repaired {
        progress("VNC 服务不可达，正在尝试自动启动…".to_string());
        let (ok, note) = try_repair(adb, password, port, &mut repaired).await;
        if ok {
            let (ok, detail) = probe_vnc(port, password, 4).await;
            reachable = ok;
            probe_detail = detail;
        } else if !note.is_empty() {
            probe_detail = format!("{}{}", note, probe_detail);
        }
    }

    if !channel_ok || !forward_ok || !reachable {
        let detail = if !channel_ok {
            relay_detail.clone()
        } else if !forward_ok {
            forward_detail.clone()
        } else {
            probe_detail.clone()
        };
        recovered.forward_ok = forward_ok;
        recovered.forward_detail = forward_detail;
        recovered.relay_detail = relay_detail;
        recovered.reachable = false;
        recovered.probe_detail = probe_detail.clone();
        recovered.viewer_found = find_tigervnc_viewer().is_some();
        recovered.message = failure_message(port, forward_ok, channel_ok, &probe_detail, &detail);
        return Ok(recovered);
    }

    // 解析客户端：内置版本优先，其次系统安装版本。
    let viewer = match find_tigervnc_viewer() {
        Some(p) => p,
        None => {
            recovered.forward_ok = forward_ok;
            recovered.forward_detail = forward_detail;
            recovered.relay_detail = relay_detail;
            recovered.reachable = true;
            recovered.probe_detail = probe_detail;
            recovered.viewer_found = false;
            recovered.message = format!(
                "未找到可用的 TigerVNC Viewer。\n{}\n\
                 也可以先在手机上使用 AVNC 连接（地址 127.0.0.1，端口 {}，密码 {}）。",
                if cfg!(windows) {
                    "程序内置的客户端解压失败，也没有在系统里找到已安装版本。"
                } else {
                    "请先安装 TigerVNC Viewer（Debian/Ubuntu: sudo apt install tigervnc-viewer）。"
                },
                port,
                password
            );
            return Ok(recovered);
        }
    };

    // 生成自动认证用的密码文件：查看器用 -passwd 免输入完成 VncAuth。
    let passwd_file = write_passwd_file(password).ok();
    let (launched, viewer_path, msg) =
        launch_tigervnc_at(&viewer, port, passwd_file.as_deref()).await?;

    Ok(VncSetup {
        port,
        forward_ok,
        forward_detail,
        relay_detail,
        reachable: true,
        probe_detail,
        viewer_found: !viewer_path.is_empty(),
        viewer_path,
        viewer_launched: launched,
        connect_hint,
        message: msg,
    })
}

/* -------------------------------- AVNC -------------------------------- */

/// 自动把 AVNC 安装到手机上：优先使用程序内置的安装包（离线可用），
/// 内置的安装失败时再尝试 GitHub Releases 的最新版；已安装则直接打开。
pub async fn install_avnc(
    adb: &Adb,
    password: &str,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<String> {
    // AVNC 的 VNC 认证只用密码，用户名可以留空
    let hint = format!(
        "请在 AVNC 中新建连接：\n地址 127.0.0.1\n端口 {}\n用户名：可留空（填 droid 也可以）\n密码：{}",
        VNC_PORT, password
    );

    if apk::installed(adb, AVNC_PACKAGE).await {
        let launched = apk::launch(adb, AVNC_PACKAGE).await;
        return Ok(if launched {
            format!("AVNC 已安装，已在手机上打开。\n{}", hint)
        } else {
            format!("AVNC 已安装，请在手机桌面打开它。\n{}", hint)
        });
    }

    async fn manual(adb: &Adb, reason: String, hint: &str) -> Result<String> {
        let _ = adb.start_view(AVNC_FDROID_URL).await;
        Ok(format!(
            "{}。已打开 F-Droid 下载页，可在手机上手动安装。\n{}",
            reason, hint
        ))
    }

    let apk_path = util::tool_data_dir().join("avnc.apk");

    // 1) 内置安装包：不依赖网络，国内也能装
    if apk::write_bundled("avnc.apk", BUNDLED_AVNC_APK).is_ok() {
        progress(format!("正在安装内置的 AVNC {} …", BUNDLED_AVNC_TAG));
        if apk::install(adb, &apk_path).await.is_ok() {
            let launched = apk::launch(adb, AVNC_PACKAGE).await;
            progress("AVNC 安装完成。".to_string());
            return Ok(format!(
                "AVNC {} 已安装到手机{}。\n{}",
                BUNDLED_AVNC_TAG,
                if launched { "并已打开" } else { "" },
                hint
            ));
        }
        progress("内置安装包安装失败，尝试下载最新版…".to_string());
    }

    let curl = match find_curl() {
        Some(c) => c,
        None => {
            return manual(
                adb,
                "系统里没有 curl，无法自动下载 AVNC 安装包".to_string(),
                &hint,
            )
            .await;
        }
    };

    progress("正在查询 AVNC 最新版本…".to_string());
    let json = match curl_get(&curl, AVNC_RELEASE_API).await {
        Ok(j) => j,
        Err(e) => {
            return manual(adb, format!("查询 AVNC 最新版本失败：{}", e), &hint).await;
        }
    };
    let (tag, url) = match find_avnc_apk(&json) {
        Some(v) => v,
        None => return manual(adb, "GitHub Releases 中未找到 APK".to_string(), &hint).await,
    };

    progress(format!("正在下载 AVNC {} …", tag));
    let size = match download_with_progress(&curl, &url, &apk_path, "AVNC", progress).await {
        Ok(n) => n,
        Err(e) => return manual(adb, format!("下载 AVNC 失败：{}", e), &hint).await,
    };
    if size < 1_000_000 {
        return manual(
            adb,
            format!("下载的 AVNC 安装包不完整（{} 字节）", size),
            &hint,
        )
        .await;
    }

    progress("正在把 AVNC 安装到手机…".to_string());
    if let Err(e) = apk::install(adb, &apk_path).await {
        return manual(adb, format!("自动安装失败：{}", e), &hint).await;
    }

    let launched = apk::launch(adb, AVNC_PACKAGE).await;
    progress("AVNC 安装完成。".to_string());
    Ok(format!(
        "AVNC {} 已安装到手机{}。\n{}",
        tag,
        if launched { "并已打开" } else { "" },
        hint
    ))
}

/* --------------------------- 下载（仅 AVNC 回退） --------------------------- */

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
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("无法读取下载流"))?;
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
    let out = cmd
        .output()
        .await
        .map_err(|e| anyhow!("启动 curl 失败: {}", e))?;
    if !out.status.success() {
        bail!("curl 退出码 {:?}", out.status.code());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::{vnc_auth_response, vnc_passwd_bytes};

    #[test]
    fn passwd_file_matches_tigervnc() {
        // 真机执行 `printf 'linux\n' | vncpasswd -f` 得到的 8 字节
        assert_eq!(
            vnc_passwd_bytes("linux"),
            [0x54, 0x6d, 0x3b, 0x7b, 0x9a, 0x13, 0x56, 0xcc]
        );
        // 超过 8 个字符只取前 8 个，不足补 0
        assert_eq!(vnc_passwd_bytes("123456789"), vnc_passwd_bytes("12345678"));
    }

    #[test]
    fn auth_response_matches_vnc() {
        // 独立实现（.NET DES + 逐字节取反的密码密钥）对 challenge 0..15 的结果。
        let challenge: Vec<u8> = (0u8..16).collect();
        assert_eq!(
            vnc_auth_response("linux", &challenge),
            [
                0xf6, 0x24, 0x37, 0xff, 0x0d, 0x69, 0xc7, 0xd3, 0x38, 0x3e, 0x37, 0xfb, 0x29,
                0xe7, 0x3f, 0xd8
            ]
        );
    }
}
