//! Linux 虚拟机磁盘容量调整。
//!
//! Android 16（API 36）及以下的 Terminal 应用里，虚拟机根分区默认只有几 GB，
//! 装完桌面环境 + 常用软件基本就满了。应用自带「⋮ → 设置 → 调整磁盘大小」，
//! 但那条路要靠手指拖滑块：不仅不好精确控制，滑块一步就是 4 MB 精度、
//! 屏幕上却只有一千多个像素，拖出来的值只能是个近似数。
//!
//! 那个界面最终做的事情其实很简单：**带着目标大小重启 Terminal 应用**。
//! `MainActivity` 把 `EXTRA_DISK_SIZE` 交给 `VmLauncherService`，
//! 服务在启动虚拟机之前调用 `InstalledImage.resize()` 去改镜像。
//! 所以这里走完全相同的路径，只是用 Intent extra 精确传递大小：
//!
//! ```text
//! am force-stop com.android.virtualization.terminal
//! am start -n com.android.virtualization.terminal/.MainActivity \
//!     --el com.android.virtualization.terminal.EXTRA_DISK_SIZE <字节数>
//! ```
//!
//! ## Android 16 与 Android 17 的隔离
//!
//! Android 17 起 Terminal 换成 new2（Compose）实现：
//!
//! * 主界面 Activity 从 `.MainActivity` 变成 `.new2.ui.MainActivity`；
//! * 磁盘容量改由系统的 storage balloon 管理，应用会忽略 `EXTRA_DISK_SIZE`。
//!
//! 两套实现**包名相同**（`com.android.virtualization.terminal`），但扩容机制完全不同。
//! 所以这里先判断设备上到底是哪一代，再决定要不要动作：
//!
//! * API ≥ 37，或旧版 Activity 不存在 → 直接跳过，一个字节都不碰；
//! * 只有确认跑的是旧版 Terminal 时，才会发扩容指令。
//!
//! ## 只扩不缩
//!
//! `InstalledImage.resize()` 拿到比当前小的目标时会真的缩小文件系统。
//! 工具的目标是「保证容量够用」，不是「强行改成一个更小的数」，
//! 因此当前容量已经够大时一律跳过，绝不把用户调大的磁盘缩回去。

use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

use crate::adb::Adb;
use crate::detect;
use crate::terminal;

/// Terminal 应用包名（Android 16 的旧版与 Android 17 的 new2 是同一个包）。
const PKG: &str = "com.android.virtualization.terminal";
/// 旧版（Android 16 及以下）Terminal 的主界面 Activity。
const MAIN_ACTIVITY: &str =
    "com.android.virtualization.terminal/com.android.virtualization.terminal.MainActivity";
/// 应用用来接收磁盘大小的 Intent extra。
const EXTRA_DISK_SIZE: &str = "com.android.virtualization.terminal.EXTRA_DISK_SIZE";
/// Android 17（API 37）起 Terminal 换成 new2，磁盘容量由系统统一管理。
const ANDROID_17_SDK: i32 = 37;
/// 1 GB = 1024^3 字节。
const GB: u64 = 1024 * 1024 * 1024;

/// 磁盘调整结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskResize {
    /// 是否真的执行了调整。
    pub applied: bool,
    /// 目标大小（GB）。
    pub target_gb: u32,
    /// 调整前的根分区大小（GB），读不到时为 0。
    pub before_gb: f64,
    /// 调整后的根分区大小（GB），读不到时为 0。
    pub after_gb: f64,
    /// skipped_new_android | skipped_balloon | skipped_same_size | resized | failed
    pub action: String,
    pub message: String,
}

/// 镜像文件的当前大小。
#[derive(Debug, Clone)]
struct ImageSize {
    /// 镜像文件的实际大小（字节）。
    ///
    /// 这正是 Terminal 内部 `InstalledImage.getApparentSize()` 的值 ——
    /// 应用判断"要不要扩容"用的就是它，所以拿它做判断最准确，
    /// 也不会出现"以为是 32 GB、实际是 32.3 GB"的偏差。
    bytes: u64,
    /// 是否由系统的 storage balloon 接管。
    ///
    /// 这种模式下应用走的是 `truncateDiskIfNecessary()`，**根本不会调用**
    /// `image.resize()`，发过去的扩容指令会被忽略掉。
    ballooned: bool,
}

/// 设备上跑的 Terminal 属于哪一代。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalFlavor {
    /// 旧版（Android 16 及以下）：自己管磁盘容量，可以发扩容指令。
    Legacy,
    /// new2（Android 17）：磁盘容量由系统 balloon 管理，不发扩容指令。
    New2,
}

/// 取一行文本里第一个整数（允许前面有空格）。
fn leading_number(s: &str) -> Option<u64> {
    let digits: String = s
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// 从 logcat 里解析出来的、与磁盘有关的信息。
#[derive(Debug, Default)]
struct DiskLog {
    /// 最近一次启动虚拟机时打印的镜像大小。
    image: Option<ImageSize>,
    /// 最近一次扩容的结果：文件系统被设置成了多少 MB。
    last_resize_mb: Option<u64>,
    /// 扩容记录一共出现了多少次（用来判断这一次是不是真的扩容了）。
    resize_count: u64,
}

/// 从 logcat 里读出镜像大小与扩容记录。
///
/// Terminal 每次启动虚拟机都会打印一行（`VmLauncherService`）：
///
/// ```text
/// Current disk size: apparent=34728837120, physical=34012352512
/// ```
///
/// 这里读的是应用自己打出来的日志，既不用解析终端屏幕，也不用往虚拟机里
/// 敲命令，因此不会受"软键盘弹不出来"这类问题影响，
/// 而且拿到的就是应用做比较时用的那个数。
///
/// 如果系统开了 storage balloon，走的是另一行：
///
/// ```text
/// rootfs apparent size=..., physical size=..., expectedSize=...
/// ```
///
/// 真正改完镜像之后，应用还会打印扩容结果：
///
/// ```text
/// resize2fs completed: <镜像路径>, size: 32768M
/// ```
///
/// 这行才是"实际扩到了多少"的权威答案 —— 上面那行打印的是扩容**之前**的大小。
async fn read_disk_log(adb: &Adb) -> DiskLog {
    let out = match adb
        .shell(
            "logcat -d -v brief -s VmTerminalApp 2>/dev/null \
             | grep -E 'Current disk size|rootfs apparent size|resize2fs completed:'",
        )
        .await
    {
        Ok(out) => out,
        Err(_) => return DiskLog::default(),
    };
    parse_disk_log(&out)
}

/// 解析 Terminal 应用打印的磁盘相关日志。
fn parse_disk_log(out: &str) -> DiskLog {
    let mut log = DiskLog::default();
    for line in out.lines() {
        if let Some(rest) = line.split("Current disk size: apparent=").nth(1) {
            if let Some(bytes) = leading_number(rest) {
                log.image = Some(ImageSize {
                    bytes,
                    ballooned: false,
                });
            }
        } else if let Some(rest) = line.split("rootfs apparent size=").nth(1) {
            if let Some(bytes) = leading_number(rest) {
                log.image = Some(ImageSize {
                    bytes,
                    ballooned: true,
                });
            }
        } else if let Some(rest) = line.split("resize2fs completed:").nth(1) {
            // 形如 " <路径>, size: 32768M"
            log.resize_count += 1;
            if let Some(mb) = rest.split("size:").nth(1).and_then(leading_number) {
                log.last_resize_mb = Some(mb);
            }
        }
    }
    log
}

/// 判断某个 Activity 在设备上是否存在。
///
/// 用 `cmd package resolve-activity` 解析：解析成功会打印
/// `包名/Activity名`，失败则打印 `No activity found`。
async fn activity_exists(adb: &Adb, component: &str) -> Option<bool> {
    let out = adb
        .shell(&format!(
            "cmd package resolve-activity --brief -n {} 2>/dev/null",
            component
        ))
        .await
        .ok()?;
    if out.trim().is_empty() {
        return None;
    }
    // 解析失败时会输出 "No activity found"，里面不含 "包名/"。
    let resolved = out.contains(&format!("{}/", PKG));
    Some(resolved)
}

/// 判断设备上 Terminal 是旧版还是 new2。
///
/// 以设备上的实际状态为准，API 级别只作为辅助判断 ——
/// 万一厂商把 new2 提前带到了 Android 16 上，也不会误发扩容指令。
async fn detect_flavor(adb: &Adb) -> TerminalFlavor {
    let sdk = detect::sdk_int(adb).await;
    if sdk >= ANDROID_17_SDK {
        return TerminalFlavor::New2;
    }
    match activity_exists(adb, MAIN_ACTIVITY).await {
        Some(true) => TerminalFlavor::Legacy,
        // 旧版 Activity 不在，但 new2 在 —— 一定是 new2。
        Some(false) => TerminalFlavor::New2,
        // 解析不出来时保持保守：只有 API 级别明确低于 37 才按旧版处理，
        // 宁可不做调整，也不要在 new2 上乱发指令。
        None => {
            if sdk > 0 {
                TerminalFlavor::Legacy
            } else {
                TerminalFlavor::New2
            }
        }
    }
}

/// 停止 Terminal 应用（虚拟机随之停止）。
///
/// 调整磁盘必须在虚拟机停止之后进行：应用是在启动虚拟机的过程中改镜像的，
/// 虚拟机还开着的时候镜像文件是挂载状态。
async fn stop_terminal(adb: &Adb) {
    let _ = adb.shell(&format!("am force-stop {}", PKG)).await;
    for _ in 0..40 {
        let pid = adb
            .shell(&format!("pidof {}", PKG))
            .await
            .unwrap_or_default()
            .trim()
            .to_string();
        if pid.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
}

/// 带目标磁盘大小重新启动 Terminal 应用。
///
/// 返回 false 表示启动失败（例如应用结构与预期不符）。
async fn start_with_disk_size(adb: &Adb, bytes: u64) -> Result<bool> {
    let cmd = format!(
        "am start -n {} --el {} {}",
        MAIN_ACTIVITY, EXTRA_DISK_SIZE, bytes
    );
    let out = adb.shell(&cmd).await.unwrap_or_default();
    if out.contains("Error") || out.contains("Exception") || out.contains("Permission Denial") {
        return Ok(false);
    }
    Ok(true)
}

/// 字节数转 GB。
fn to_gb(bytes: u64) -> f64 {
    bytes as f64 / GB as f64
}

/// 确保虚拟机根分区不小于 `target_gb` GB。
///
/// * Android 17（new2）：直接跳过（新 Terminal 由系统管理磁盘容量）。
/// * 系统启用了 storage balloon：跳过（应用会忽略扩容指令）。
/// * 已经足够大：跳过，不做无谓的虚拟机重启，也绝不把磁盘缩小。
/// * 否则：让 Terminal 应用把磁盘扩到目标大小（期间虚拟机会重启一次）。
pub async fn ensure_disk_size(adb: &Adb, target_gb: u32) -> Result<DiskResize> {
    if target_gb == 0 {
        return Ok(DiskResize {
            applied: false,
            target_gb,
            before_gb: 0.0,
            after_gb: 0.0,
            action: "skipped_same_size".to_string(),
            message: "磁盘容量调整为 0，已跳过。".to_string(),
        });
    }

    if detect_flavor(adb).await == TerminalFlavor::New2 {
        let sdk = detect::sdk_int(adb).await;
        return Ok(DiskResize {
            applied: false,
            target_gb,
            before_gb: 0.0,
            after_gb: 0.0,
            action: "skipped_new_android".to_string(),
            message: format!(
                "设备上的 Terminal 是 Android 17（API {}）起的新版实现，\
                 磁盘容量由系统统一管理，不需要手动调整，已跳过。",
                sdk
            ),
        });
    }

    let target_bytes = (target_gb as u64) * GB;
    let before_log = read_disk_log(adb).await;
    let current = before_log.image.clone();

    if let Some(img) = &current {
        if img.ballooned {
            return Ok(DiskResize {
                applied: false,
                target_gb,
                before_gb: to_gb(img.bytes),
                after_gb: to_gb(img.bytes),
                action: "skipped_balloon".to_string(),
                message: format!(
                    "系统启用了存储 balloon（镜像当前 {:.1} GB），\
                     Terminal 应用的磁盘容量由系统自动伸缩，无法也不需要手动调整，已跳过。",
                    to_gb(img.bytes)
                ),
            });
        }
        // 只扩不缩：当前已经够大就直接跳过，
        // 既省掉一次虚拟机重启，也不会把用户调大的磁盘缩回去。
        if img.bytes >= target_bytes {
            return Ok(DiskResize {
                applied: false,
                target_gb,
                before_gb: to_gb(img.bytes),
                after_gb: to_gb(img.bytes),
                action: "skipped_same_size".to_string(),
                message: format!(
                    "虚拟机磁盘当前 {:.1} GB，已经不小于目标 {} GB，无需调整。",
                    to_gb(img.bytes),
                    target_gb
                ),
            });
        }
    }

    let before_gb = current.as_ref().map(|i| to_gb(i.bytes)).unwrap_or(0.0);

    stop_terminal(adb).await;
    if !start_with_disk_size(adb, target_bytes).await? {
        return Ok(DiskResize {
            applied: false,
            target_gb,
            before_gb,
            after_gb: before_gb,
            action: "failed".to_string(),
            message: format!(
                "启动 Terminal 应用失败，无法调整磁盘大小（目标 {} GB）。",
                target_gb
            ),
        });
    }

    // 应用会先改镜像再启动虚拟机，这一步可能要几分钟（分配 32 GB 空间）。
    let phase = terminal::wait_ready(adb, 900).await.unwrap_or(terminal::TerminalPhase::Unknown);
    if phase != terminal::TerminalPhase::Ready {
        return Ok(DiskResize {
            applied: false,
            target_gb,
            before_gb,
            after_gb: before_gb,
            action: "failed".to_string(),
            message: format!(
                "调整磁盘后虚拟机没有正常启动（当前状态：{}），请手动打开 Terminal 应用确认。",
                phase.label()
            ),
        });
    }

    // 再读一次日志，确认这一次真的扩容了，以及扩到了多少。
    let after_log = read_disk_log(adb).await;
    let resized = after_log.resize_count > before_log.resize_count;
    let after_gb = if resized {
        after_log
            .last_resize_mb
            .map(|mb| mb as f64 / 1024.0)
            .unwrap_or(target_gb as f64)
    } else {
        before_gb
    };

    if !resized {
        return Ok(DiskResize {
            applied: false,
            target_gb,
            before_gb,
            after_gb: before_gb,
            action: "failed".to_string(),
            message: format!(
                "Terminal 应用启动了，但日志里没有出现新的扩容记录，磁盘大小可能没有变化。\
                 请在手机 Terminal 的「⋮ → 设置 → 调整磁盘大小」里确认当前容量。"
            ),
        });
    }

    Ok(DiskResize {
        applied: true,
        target_gb,
        before_gb,
        after_gb,
        action: "resized".to_string(),
        message: format!(
            "虚拟机磁盘已从 {:.1} GB 调整为 {:.1} GB（目标 {} GB），期间虚拟机重启了一次。",
            before_gb, after_gb, target_gb
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机上抓到的原始日志行。
    const REAL_LOG: &str = "\
D/VmTerminalApp( 5721): Current disk size: apparent=34728837120, physical=34012352512
D/VmTerminalApp( 5975): Current disk size: apparent=34728837120, physical=33878585344
D/VmTerminalApp( 5975): resize2fs completed: /data/user/0/com.android.virtualization.terminal/files/linux/root_part, size: 32768M
D/VmTerminalApp( 9369): Current disk size: apparent=34359738368, physical=33509064704";

    #[test]
    fn 解析镜像大小与扩容记录() {
        let log = parse_disk_log(REAL_LOG);
        let image = log.image.expect("应当解析出镜像大小");
        assert_eq!(image.bytes, 34359738368);
        assert!(!image.ballooned);
        assert_eq!(log.resize_count, 1);
        assert_eq!(log.last_resize_mb, Some(32768));
    }

    #[test]
    fn 识别_storage_balloon() {
        let log = parse_disk_log(
            "D/VmTerminalApp( 1234): rootfs apparent size=68719476736, physical size=1073741824, expectedSize=68719476736",
        );
        let image = log.image.expect("应当解析出镜像大小");
        assert_eq!(image.bytes, 68719476736);
        assert!(image.ballooned, "balloon 模式必须被识别出来，否则会发出无效的扩容指令");
    }

    #[test]
    fn 识别缩容记录不被误当成扩容() {
        // shrinkToMinimumSize 打的是 "resize2fs -M completed:"，没有 size 字段，
        // 不能被当成一次扩容。
        let log = parse_disk_log(
            "D/VmTerminalApp( 1234): resize2fs -M completed: /data/user/0/x/root_part",
        );
        assert_eq!(log.resize_count, 0);
        assert_eq!(log.last_resize_mb, None);
    }

    #[test]
    fn 空日志不报错() {
        let log = parse_disk_log("");
        assert!(log.image.is_none());
        assert_eq!(log.resize_count, 0);
    }

    #[test]
    fn 容量换算() {
        assert!((to_gb(34359738368) - 32.0).abs() < 0.001);
        assert!((to_gb(34728837120) - 32.34375).abs() < 0.001);
    }
}
