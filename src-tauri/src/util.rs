//! 通用工具：工具数据目录、内置资源落盘、打开外部链接。

use anyhow::{anyhow, Result};
#[cfg(windows)]
use anyhow::bail;
use std::path::{Path, PathBuf};

/// 工具自己的数据目录（存放解压出来的 adb、TigerVNC Viewer、APK 等）。
///
/// Windows 放在 `%LOCALAPPDATA%`，Linux 遵循 XDG 规范放在
/// `$XDG_DATA_HOME`（默认 `~/.local/share`），macOS 放在 `~/Library/Application Support`。
pub fn tool_data_dir() -> PathBuf {
    const APP_DIR: &str = "AndroidCreateLinuxGUITool";
    if let Some(dir) = non_empty_env("LOCALAPPDATA") {
        return dir.join(APP_DIR);
    }
    if let Some(dir) = non_empty_env("XDG_DATA_HOME") {
        return dir.join(APP_DIR);
    }
    if let Some(home) = non_empty_env("HOME") {
        return if cfg!(target_os = "macos") {
            home.join("Library").join("Application Support").join(APP_DIR)
        } else {
            home.join(".local").join("share").join(APP_DIR)
        };
    }
    std::env::temp_dir().join(APP_DIR)
}

fn non_empty_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 把内置资源写到数据目录，返回文件路径。
///
/// 已存在且大小一致时直接复用，避免每次启动都重写几十 MB 的文件。
/// 写入失败返回 `None`，由调用方决定如何降级。
pub fn write_bundled_file(name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = tool_data_dir();
    let target = dir.join(name);
    let up_to_date = target.is_file()
        && std::fs::metadata(&target)
            .map(|m| m.len() == bytes.len() as u64)
            .unwrap_or(false);
    if up_to_date {
        return Some(target);
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&target, bytes).ok()?;
    Some(target)
}

/// 启动一个 GUI 程序并立即返回。
///
/// Windows 上使用 `ShellExecuteW`：它会在一个**不继承**当前进程句柄的新进程里
/// 启动目标程序。用 `std::process::Command` 直接 spawn 时，子进程会继承本进程的
/// stdout 管道；CLI 模式（`--port=5901`）就会一直等到用户关掉 Viewer 才退出。
pub fn spawn_detached(program: &Path, args: &[&str]) -> Result<()> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: *mut c_void,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show_cmd: i32,
            ) -> *mut c_void;
        }

        fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
            text.encode_wide().chain(std::iter::once(0)).collect()
        }
        fn quote(arg: &str) -> String {
            if arg.is_empty()
                || arg.contains(' ')
                || arg.contains('\t')
                || arg.contains('"')
            {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.to_string()
            }
        }

        const SW_SHOWNORMAL: i32 = 1;
        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let file = wide(program.as_os_str());
        let parameters: String = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        let parameters: Vec<u16> = parameters.encode_utf16().chain(std::iter::once(0)).collect();

        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                parameters.as_ptr(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW 的返回值 > 32 才表示成功。
        if result as isize <= 32 {
            bail!("ShellExecuteW 启动 {} 失败（错误码 {}）", program.display(), result as isize);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::process::Stdio;
        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| anyhow!("启动 {} 失败: {}", program.display(), e))
    }
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
