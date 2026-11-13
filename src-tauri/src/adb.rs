//! ADB 命令行封装。
//!
//! 全部通过 `adb` 可执行文件驱动（而不是自己实现 adb 协议），因为本工具重度依赖
//! `shell input`、`uiautomator dump`、`forward`、`push` 这些子命令，用 CLI 最稳。
//! 所有调用都直接传参数数组，不经过本地 shell，因此不存在转义歧义。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use tokio::process::Command;

/// `adb devices -l` 中的一台设备。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub serial: String,
    pub state: String,
    pub model: String,
    pub product: String,
    pub device: String,
    pub transport_id: String,
    /// 形如 `product:xxx/model:yyy/device:zzz`，用于二次确认机型。
    pub fingerprint: String,
}

/// Windows 的 CREATE_NO_WINDOW 标志。
///
/// GUI 程序启动 console 子进程（adb.exe 等）时，Windows 默认会给每个子进程弹一个
/// 黑色控制台窗口并一闪而过。加上这个标志可以彻底消除该闪烁。
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给异步子进程加上「不弹控制台窗口」的标志。
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 给同步子进程加上「不弹控制台窗口」的标志。
pub fn hide_console_std(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// 随程序内置的 platform-tools（离线可用，无需用户自行安装 Android SDK）。
///
/// adb.exe 运行时依赖同目录下的两个 DLL，所以三个文件要一起解压。
const BUNDLED_ADB: &[u8] = include_bytes!("../resources/adb.exe");
const BUNDLED_ADB_API: &[u8] = include_bytes!("../resources/AdbWinApi.dll");
const BUNDLED_ADB_USB_API: &[u8] = include_bytes!("../resources/AdbWinUsbApi.dll");

/// 把内置的 adb 解压到工具数据目录，返回 adb 可执行文件的路径。
///
/// 已经解压过（且大小一致）时直接复用，不做任何写入。
pub fn bundled_adb() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let dir = crate::vnc::tool_data_dir().join("platform-tools");
    let exe = dir.join("adb.exe");
    let api = dir.join("AdbWinApi.dll");
    let usb = dir.join("AdbWinUsbApi.dll");

    let up_to_date = exe.is_file()
        && api.is_file()
        && usb.is_file()
        && std::fs::metadata(&exe)
            .map(|m| m.len() == BUNDLED_ADB.len() as u64)
            .unwrap_or(false);
    if up_to_date {
        return Some(exe);
    }

    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&exe, BUNDLED_ADB).ok()?;
    std::fs::write(&api, BUNDLED_ADB_API).ok()?;
    std::fs::write(&usb, BUNDLED_ADB_USB_API).ok()?;
    Some(exe)
}

/// ADB 调用器，绑定到一个可选的设备序列号。
#[derive(Debug, Clone)]
pub struct Adb {
    exe: PathBuf,
    serial: Option<String>,
}

/// 一次命令执行的完整结果。
#[derive(Debug, Clone)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    pub fn combined(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

impl Adb {
    pub fn new(exe: PathBuf, serial: Option<String>) -> Self {
        Self { exe, serial }
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    pub fn with_serial(&self, serial: impl Into<String>) -> Adb {
        Adb {
            exe: self.exe.clone(),
            serial: Some(serial.into()),
        }
    }

    /// 在本机寻找 adb 可执行文件。
    pub fn locate() -> Result<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Ok(p) = std::env::var("ADB_PATH") {
            if !p.trim().is_empty() {
                candidates.push(PathBuf::from(p));
            }
        }

        // 程序自己内置的 adb（解压到用户数据目录），优先于系统里的任何版本
        if let Some(p) = bundled_adb() {
            candidates.push(p);
        }

        // PATH 目录扫描
        if let Ok(path) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path) {
                candidates.push(dir.join(if cfg!(windows) { "adb.exe" } else { "adb" }));
            }
        }

        // Android SDK 常见位置
        let mut sdk_roots: Vec<PathBuf> = Vec::new();
        for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Ok(v) = std::env::var(key) {
                if !v.trim().is_empty() {
                    sdk_roots.push(PathBuf::from(v));
                }
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            sdk_roots.push(PathBuf::from(local).join("Android").join("Sdk"));
        }
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            let home = PathBuf::from(home);
            sdk_roots.push(home.join("AppData").join("Local").join("Android").join("Sdk"));
            sdk_roots.push(home.join("Android").join("Sdk"));
            sdk_roots.push(home.join("Library").join("Android").join("sdk"));
        }
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            sdk_roots.push(PathBuf::from(program_files).join("Android"));
        }
        for root in sdk_roots {
            candidates.push(root.join("platform-tools").join(if cfg!(windows) { "adb.exe" } else { "adb" }));
            candidates.push(root.join("platform-tools").join("adb"));
        }

        for cand in candidates {
            if cand.is_file() {
                return Ok(cand);
            }
        }

        bail!(
            "未找到 adb 可执行文件。请安装 Android Platform Tools 并加入 PATH，\
             或在设置中手动指定 adb.exe 的完整路径。"
        )
    }

    /// 执行一条 adb 子命令，返回完整结果。
    pub async fn raw(&self, args: &[&str]) -> Result<CmdOutput> {
        let mut cmd = Command::new(&self.exe);
        // 注意：-s <serial> 必须放在子命令之前，
        // 否则 adb 会把 -s 一起传给设备端 shell（例如 `vm info -s xxx`）。
        if let Some(serial) = &self.serial {
            cmd.arg("-s").arg(serial);
        }
        cmd.args(args);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        // 关键：避免每条 adb 命令都闪一个黑色控制台窗口
        hide_console(&mut cmd);

        let output = cmd
            .output()
            .await
            .with_context(|| format!("执行 adb {} 失败", args.join(" ")))?;

        Ok(CmdOutput {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// 执行 adb 子命令并在失败时返回错误（带 stderr 便于排障）。
    pub async fn check(&self, args: &[&str]) -> Result<String> {
        let out = self.raw(args).await?;
        if out.ok() {
            Ok(out.stdout)
        } else {
            Err(anyhow!(
                "adb {} 失败 (code={}): {}",
                args.join(" "),
                out.code,
                out.combined().trim()
            ))
        }
    }

    /// 在设备上执行 shell 命令，返回 stdout。
    pub async fn shell(&self, cmd: &str) -> Result<String> {
        let out = self.raw(&["shell", cmd]).await?;
        Ok(out.stdout)
    }

    /// 在设备上执行 shell 命令，返回 (是否成功, 合并输出)。
    pub async fn shell_status(&self, cmd: &str) -> Result<(bool, String)> {
        let out = self.raw(&["shell", cmd]).await?;
        let combined = out.combined();
        // 部分 Android shell 在命令不存在时仍返回 0，因此同时看输出内容。
        let looks_failed = combined.contains("not found")
            || combined.contains("No such file or directory")
            || combined.contains("Permission denied")
            || combined.contains("Exception");
        Ok((out.ok() && !looks_failed, combined))
    }

    /// 列出已连接设备。
    pub async fn devices(&self) -> Result<Vec<DeviceEntry>> {
        let out = self.check(&["devices", "-l"]).await?;
        let mut list = Vec::new();
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("List of devices") || line.starts_with("*") {
                continue;
            }
            let mut parts = line.split_whitespace();
            let serial = match parts.next() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let state = parts.next().unwrap_or("unknown").to_string();
            let mut entry = DeviceEntry {
                serial,
                state,
                model: String::new(),
                product: String::new(),
                device: String::new(),
                transport_id: String::new(),
                fingerprint: String::new(),
            };
            for kv in parts {
                if let Some((k, v)) = kv.split_once(':') {
                    match k {
                        "model" => entry.model = v.to_string(),
                        "product" => entry.product = v.to_string(),
                        "device" => entry.device = v.to_string(),
                        "transport_id" => entry.transport_id = v.to_string(),
                        _ => {}
                    }
                }
            }
            list.push(entry);
        }
        Ok(list)
    }

    /// 推送文件到设备。
    pub async fn push(&self, local: &Path, remote: &str) -> Result<()> {
        let local_s = local.to_string_lossy().to_string();
        let out = self.raw(&["push", &local_s, remote]).await?;
        if out.ok() {
            Ok(())
        } else {
            Err(anyhow!(
                "推送文件失败: {}",
                out.combined().trim()
            ))
        }
    }

    /// 建立端口转发（PC 本地端口 -> 设备端口）。
    pub async fn forward(&self, local: &str, remote: &str) -> Result<()> {
        let out = self.raw(&["forward", local, remote]).await?;
        if out.ok() {
            Ok(())
        } else {
            Err(anyhow!("端口转发失败: {}", out.combined().trim()))
        }
    }

    /// 列出当前所有端口转发。
    pub async fn forward_list(&self) -> Result<String> {
        Ok(self
            .raw(&["forward", "--list"])
            .await
            .map(|o| o.stdout)
            .unwrap_or_else(|_| String::new()))
    }

    /// 启动一个 Android Activity。
    pub async fn start_activity(&self, component: &str) -> Result<()> {
        let out = self
            .raw(&["shell", &format!("am start -n {}", component)])
            .await?;
        let text = out.combined();
        if text.contains("Error") || text.contains("Exception") || text.contains("Permission Denial") {
            bail!("启动 Activity 失败: {}", text.trim());
        }
        Ok(())
    }

    /// 启动一个 VIEW intent（用于打开 Play 商店页面等）。
    pub async fn start_view(&self, url: &str) -> Result<()> {
        let cmd = format!("am start -a android.intent.action.VIEW -d '{}'", url);
        let out = self.raw(&["shell", &cmd]).await?;
        let text = out.combined();
        if text.contains("Error") || text.contains("Exception") {
            bail!("打开链接失败: {}", text.trim());
        }
        Ok(())
    }

    /// 截图（PNG 字节）。
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        let out = self.raw(&["exec-out", "screencap", "-p"]).await?;
        if out.code != 0 || out.stdout.is_empty() {
            bail!("截图失败: {}", out.stderr.trim());
        }
        Ok(out.stdout.into_bytes())
    }
}

/// 把一段文本安全地嵌入设备端 shell 命令（单引号包裹）。
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `input text` 无法直接输入空格，需要用 `%s` 代替。
pub fn input_text_payload(text: &str) -> String {
    text.replace(' ', "%s")
}
