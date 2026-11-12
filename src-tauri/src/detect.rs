//! 设备与 AVF（Android Virtualization Framework）能力检测。

use anyhow::Result;
use serde::Serialize;
use tokio::time::{timeout, Duration};

use crate::adb::Adb;

/// AOSP 中 Linux Terminal 应用的包名。
pub const TERMINAL_PKG: &str = "com.android.virtualization.terminal";
/// AOSP 中 AVF apex 内的 vm 工具路径。
pub const VM_BIN: &str = "/apex/com.android.virt/bin/vm";

/// AVF / Linux 虚拟机能力报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvfReport {
    pub serial: String,
    pub model: String,
    pub manufacturer: String,
    pub android_release: String,
    pub sdk_int: i32,
    pub is_pixel: bool,
    /// `pm list features` 中命中 virtualization_framework 的行。
    pub feature_line: String,
    pub feature_supported: bool,
    /// `/apex/com.android.virt/bin/vm info` 是否执行成功。
    pub vm_info_ok: bool,
    pub vm_info: String,
    pub virt_apex_present: bool,
    pub terminal_pkg_present: bool,
    pub terminal_pkg_enabled: bool,
    pub supported: bool,
    pub reasons: Vec<String>,
}

/// 带超时的 shell 执行。
async fn shell_timeout(adb: &Adb, cmd: &str, secs: u64) -> Result<(bool, String)> {
    match timeout(Duration::from_secs(secs), adb.shell_status(cmd)).await {
        Ok(res) => res,
        Err(_) => Ok((false, format!("命令超时({}s): {}", secs, cmd))),
    }
}

async fn getprop(adb: &Adb, key: &str) -> String {
    adb.shell(&format!("getprop {}", key))
        .await
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 完整检测设备是否支持 Linux 虚拟机。
pub async fn check_avf(adb: &Adb) -> Result<AvfReport> {
    let serial = adb.serial().unwrap_or_default().to_string();
    let model = getprop(adb, "ro.product.model").await;
    let manufacturer = getprop(adb, "ro.product.manufacturer").await;
    let android_release = getprop(adb, "ro.build.version.release").await;
    let sdk_int = getprop(adb, "ro.build.version.sdk")
        .await
        .parse::<i32>()
        .unwrap_or(0);
    let is_pixel = model.to_lowercase().contains("pixel");

    // 1) pm list features | grep virtualization_framework
    let features = adb.shell("pm list features").await.unwrap_or_default();
    let feature_line = features
        .lines()
        .map(|l| l.trim())
        .find(|l| l.contains("virtualization_framework"))
        .unwrap_or("")
        .to_string();
    let feature_supported = !feature_line.is_empty();

    // 2) AVF 运行时是否存在。
    //    注意：Android 17 等系统上 `ls /apex/` 对 shell 用户是 Permission denied，
    //    必须直接探测 vm 二进制本身，不能靠列目录。
    let apex_probe = adb
        .shell(
            "test -x /apex/com.android.virt/bin/vm && echo APEX_OK || echo APEX_MISSING",
        )
        .await
        .unwrap_or_default();
    let virt_apex_present = apex_probe.contains("APEX_OK");

    // 3) /apex/com.android.virt/bin/vm info
    let (vm_info_ok, vm_info) =
        shell_timeout(adb, &format!("{} info", VM_BIN), 20).await?;

    // 4) 终端 App 是否安装 / 已启用
    let present = adb
        .shell(&format!("pm list packages {}", TERMINAL_PKG))
        .await
        .unwrap_or_default()
        .contains(TERMINAL_PKG);
    let disabled_list = adb
        .shell(&format!("pm list packages -d {}", TERMINAL_PKG))
        .await
        .unwrap_or_default();
    let disabled = disabled_list.contains(TERMINAL_PKG);

    let mut reasons: Vec<String> = Vec::new();
    if !feature_supported {
        reasons.push(
            "系统未声明 virtualization_framework 特性：该机型/系统版本不提供 AVF 虚拟环境。"
                .to_string(),
        );
    }
    if !virt_apex_present {
        reasons.push("设备上不存在 /apex/com.android.virt（AVF 运行时缺失）。".to_string());
    }
    if !vm_info_ok {
        reasons.push(format!("`{} info` 执行失败，输出：{}", VM_BIN, vm_info.trim()));
    }
    if sdk_int > 0 && sdk_int < 35 {
        reasons.push(format!(
            "系统 API 级别为 {}，低于 Android 15 (API 35)，不支持 Linux 终端。",
            sdk_int
        ));
    }

    // 具备特征 + AVF 运行时即可认为支持；缺少终端 App 时提示但不算致命。
    let supported = feature_supported && virt_apex_present && vm_info_ok;
    if !present {
        reasons.push(
            "当前系统镜像中没有 Terminal 应用（com.android.virtualization.terminal）。\
             它是随系统分发的内置应用，应用商店无法单独安装，需要升级到包含该应用的系统版本。"
                .to_string(),
        );
    }

    Ok(AvfReport {
        serial,
        model,
        manufacturer,
        android_release,
        sdk_int,
        is_pixel,
        feature_line,
        feature_supported,
        vm_info_ok,
        vm_info: vm_info.trim().to_string(),
        virt_apex_present,
        terminal_pkg_present: present,
        terminal_pkg_enabled: present && !disabled,
        supported,
        reasons,
    })
}
