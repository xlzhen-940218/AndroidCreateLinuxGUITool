//! 在手机上安装、查询、启动 APK。
//!
//! 内置 APK（TigerVNC 的 Android 客户端 AVNC、音频播放器）都通过这里落盘并安装，
//! 避免每个模块各写一份 `adb install` 与 `pm path`。

use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

use crate::adb::Adb;

/// 把内置 APK 写到工具数据目录。
pub fn write_bundled(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    crate::util::write_bundled_file(name, bytes)
        .ok_or_else(|| anyhow!("无法把内置的 {} 写入工具数据目录", name))
}

/// 应用是否已经装在手机上。
pub async fn installed(adb: &Adb, package: &str) -> bool {
    adb.shell(&format!("pm path {}", package))
        .await
        .map(|s| s.contains("package:"))
        .unwrap_or(false)
}

/// 用 launcher intent 打开应用，返回是否成功。
pub async fn launch(adb: &Adb, package: &str) -> bool {
    adb.shell(&format!(
        "monkey -p {} -c android.intent.category.LAUNCHER 1",
        package
    ))
    .await
    .map(|s| !s.contains("No activities found"))
    .unwrap_or(false)
}

/// 用 `adb install -r` 安装一个本地 APK。
pub async fn install(adb: &Adb, apk: &Path) -> Result<()> {
    let out = adb
        .raw(&["install", "-r", &apk.to_string_lossy()])
        .await
        .map_err(|e| anyhow!("调用 adb install 失败: {}", e))?;
    let text = out.combined();
    if !out.ok() || !text.to_lowercase().contains("success") {
        bail!("{}", text.trim());
    }
    Ok(())
}
