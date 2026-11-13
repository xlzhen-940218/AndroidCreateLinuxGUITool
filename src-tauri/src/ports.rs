//! Terminal 应用「端口放行」的自动化。
//!
//! AOSP 实现（packages/modules/Virtualization/android/TerminalApp）：
//!   * `SettingsPortForwardingActivity` 展示虚拟机里正在监听的端口；
//!   * "Saved allowed ports" 列表里的端口会被放行，虚拟机一旦监听即自动转发到手机本机；
//!   * 放行状态保存在应用私有 SharedPreferences（".PORTS"），外部无法直接写。
//!
//! Android 17 起 Terminal 换成了 new2（Compose）实现，端口设置不再是独立 Activity，
//! 而是藏在「⋮ → Settings → Security」里（Port control / Listening ports /
//! Saved allowed ports）。因此这里采用**三条路径**：
//!   1) 新版：主界面 ⋮ → Settings → Security，打开端口开关或点「+」新增；
//!   2) 通知栏放行（两个版本都有，字符串稳定）：虚拟机开始监听端口时，
//!      Terminal 会弹出「Terminal is requesting to open a new port / Accept」通知；
//!   3) 端口控制界面（老版本有独立 Activity，按 resource-id 操作）。

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::time::Duration;

use crate::adb::Adb;
use crate::uia::{self, UiNode};

pub const PORT_FWD_ACTIVITY: &str =
    "com.android.virtualization.terminal/com.android.virtualization.terminal.SettingsPortForwardingActivity";

/// 端口放行结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortResult {
    pub port: u16,
    pub ok: bool,
    /// notification | switched_on | added | already_enabled | manual
    pub action: String,
    pub message: String,
    /// 是否在虚拟机里检测到该端口正在监听。
    pub active: bool,
}

/// 打开端口控制界面（老版本有独立 Activity）。
pub async fn open_settings(adb: &Adb) -> Result<()> {
    adb.start_activity(PORT_FWD_ACTIVITY).await.map_err(|e| {
        anyhow!("无法打开 Terminal 应用的端口控制界面：{}", e)
    })
}

/// 找到某一行里的开关控件（与端口文本在同一行）。
fn switch_in_row<'a>(nodes: &'a [UiNode], port_text: &str) -> Option<&'a UiNode> {
    let label = nodes.iter().find(|n| {
        n.id.ends_with("settings_port_forwarding_active_item_port")
            && n.text.trim().starts_with(port_text)
    })?;
    let sw = nodes.iter().find(|n| {
        n.id.ends_with("settings_port_forwarding_active_item_enabled_switch")
            && n.bounds.y1 >= label.bounds.y1 - 40
            && n.bounds.y2 <= label.bounds.y2 + 40
    })?;
    Some(sw)
}

/// 是否已出现在「已保存的允许端口」列表中。
fn in_saved_list(nodes: &[UiNode], port: u16) -> bool {
    nodes.iter().any(|n| {
        n.id.ends_with("settings_port_forwarding_inactive_item_port")
            && n.text.trim() == port.to_string()
    })
}

/// 检查设备本机是否真的在监听某个端口（/proc/net/tcp{,6}）。
pub async fn device_listens_on(adb: &Adb, port: u16) -> bool {
    let hex = format!("{:04X}", port);
    let out = adb
        .shell(&format!(
            "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null | grep -c ':{}' ",
            hex
        ))
        .await
        .unwrap_or_default();
    out.trim().parse::<u32>().unwrap_or(0) > 0
}

/// 路径 1：通知栏放行。
///
/// Terminal 检测到虚拟机有新端口监听时会发一条通知。
/// 新版（new2）实测文案为：
///   标题 "Port Forwarding Request"
///   正文 "Xtigervnc is trying to open ports: 5901. Allow forwarding?"
///   操作按钮 "Allow once" / "Allow always" / "Deny"（resource-id = android:id/action0）
///
/// 关键：**必须点中「提到目标端口的那一条通知」里的按钮**——
/// 同一屏往往还有 sshd 等其它端口的请求，点错就会放行错端口。
/// 因此先按端口文本定位通知行，再在行内找按钮。
pub async fn enable_port_via_notification(adb: &Adb, port: u16) -> Result<bool> {
    let port_str = port.to_string();
    const ACCEPT_LABELS: [&str; 12] = [
        "Allow always",
        "Allow once",
        "Accept",
        "Allow",
        "ALLOW",
        "始终允许",
        "仅允许一次",
        "允许一次",
        "允许",
        "接受",
        "确定",
        "OK",
    ];

    for _ in 0..12 {
        // 先收起再展开：`cmd statusbar expand-notifications` 是"切换"语义，
        // 通知栏本来开着时会被关掉；而且开合动画期间节点坐标是失效的。
        adb.raw(&["shell", "cmd statusbar collapse"]).await.ok();
        tokio::time::sleep(Duration::from_millis(900)).await;
        adb.raw(&["shell", "cmd statusbar expand-notifications"]).await?;
        tokio::time::sleep(Duration::from_millis(2200)).await;

        let nodes = uia::dump(adb).await?;

        // 找到提到了目标端口的文本节点（系统语言可能是中文或英文）
        let port_node = nodes.iter().find(|n| {
            let t = format!("{} {}", n.text, n.desc);
            let lower = t.to_lowercase();
            t.contains(&port_str)
                && (lower.contains("port")
                    || lower.contains("forward")
                    || lower.contains("open")
                    || t.contains("端口")
                    || t.contains("转发")
                    || t.contains("打开"))
        });

        let Some(port_node) = port_node else {
            adb.raw(&["shell", "cmd statusbar collapse"]).await?;
            tokio::time::sleep(Duration::from_millis(1200)).await;
            continue;
        };

        // 找到包含它的通知行（expandableNotificationRow）
        let row = nodes.iter().find(|n| {
            n.id.ends_with("expandableNotificationRow")
                && n.bounds.x1 <= port_node.bounds.x1
                && port_node.bounds.x2 <= n.bounds.x2
                && n.bounds.y1 <= port_node.bounds.y1
                && port_node.bounds.y2 <= n.bounds.y2
        });
        let row_bounds = row.map(|r| r.bounds);

        for label in ACCEPT_LABELS {
            let btn = nodes.iter().find(|n| {
                n.clickable
                    && n.text.trim().eq_ignore_ascii_case(label)
                    // 优先只在同一条通知内找按钮
                    && row_bounds
                        .map(|r| n.bounds.y1 >= r.y1 - 8 && n.bounds.y2 <= r.y2 + 8)
                        .unwrap_or(true)
            });
            if let Some(btn) = btn {
                uia::tap_node(adb, btn).await?;
                tokio::time::sleep(Duration::from_millis(1500)).await;

                // 校验：这条端口通知应该已经消失
                let after = uia::dump(adb).await.unwrap_or_default();
                let still_there = after.iter().any(|n| {
                    let t = format!("{} {}", n.text, n.desc);
                    t.contains(&port_str)
                        && (t.to_lowercase().contains("port") || t.contains("端口"))
                });
                adb.raw(&["shell", "cmd statusbar collapse"]).await?;
                if !still_there {
                    // 通知没了，再确认设备端确实开始监听（转发真正生效）
                    for _ in 0..6 {
                        if device_listens_on(adb, port).await {
                            return Ok(true);
                        }
                        tokio::time::sleep(Duration::from_millis(800)).await;
                    }
                    // 通知已消失但端口还没起来：多半是用户之前就放行过，
                    // 由调用方去做端口控制界面那条路径确认。
                    return Ok(true);
                }
                // 没点中就再来一轮
                tokio::time::sleep(Duration::from_millis(1000)).await;
                break;
            }
        }
    }
    Ok(false)
}

/// 读取界面节点，失败或为空时重试几次（界面动画中 dump 容易失败）。
async fn dump_retry(adb: &Adb, tries: usize) -> Result<Vec<UiNode>> {
    let mut last = String::new();
    for _ in 0..tries {
        match uia::dump(adb).await {
            Ok(n) if !n.is_empty() => return Ok(n),
            Ok(_) => last = "节点树为空".to_string(),
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
    Err(anyhow!("读取界面节点失败：{}", last))
}

/// 打开新版（new2）Terminal 的「设置 → 安全」页面。
///
/// Android 17 起 Terminal 没有独立的端口控制 Activity，正确路径是：
/// 主界面右上角「⋮」（More options）→ Settings → Security。
/// Security 页面里才有 Port control / Listening ports / Saved allowed ports。
async fn open_security_page(adb: &Adb) -> Result<()> {
    match open_security_page_once(adb).await {
        Ok(()) => Ok(()),
        Err(first) => {
            // 菜单/页面偶发没弹出来时再走一遍
            tokio::time::sleep(Duration::from_millis(800)).await;
            open_security_page_once(adb)
                .await
                .map_err(|e| anyhow!("{}；重试仍失败：{}", first, e))
        }
    }
}

async fn open_security_page_once(adb: &Adb) -> Result<()> {
    // 屏幕熄灭或通知栏展开时，节点树里是别的界面，先恢复成 Terminal 主界面
    adb.raw(&["shell", "input keyevent", "224"]).await.ok();
    adb.raw(&["shell", "wm dismiss-keyguard"]).await.ok();
    adb.raw(&["shell", "cmd statusbar collapse"]).await.ok();
    tokio::time::sleep(Duration::from_millis(600)).await;

    let activity = crate::terminal::resolve_launch_activity(adb).await?;
    adb.start_activity(&activity).await?;
    tokio::time::sleep(Duration::from_millis(1800)).await;

    // 1) 溢出菜单「⋮」。真正可点击的是包着图标的父节点，
    //    但点在图标中心同样会落到父节点上。
    //    如果当前停在设置页/子页面，先按返回回到主界面。
    let mut nodes = dump_retry(adb, 3).await?;
    for _ in 0..2 {
        if find_more_options(&nodes).is_some() {
            break;
        }
        let _ = adb.raw(&["shell", "input keyevent", "4"]).await;
        tokio::time::sleep(Duration::from_millis(900)).await;
        nodes = dump_retry(adb, 3).await?;
    }
    let more = find_more_options(&nodes)
        .ok_or_else(|| anyhow!("未能在 Terminal 主界面找到「更多选项（⋮）」按钮。"))?;
    uia::tap_node(adb, more).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // 2) 下拉菜单里的 Settings
    let nodes = dump_retry(adb, 3).await?;
    let settings = nodes
        .iter()
        .find(|n| {
            let t = n.text.trim();
            t.eq_ignore_ascii_case("settings") || t == "设置"
        })
        .ok_or_else(|| anyhow!("未能在菜单中找到「Settings / 设置」。"))?;
    uia::tap_node(adb, settings).await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // 3) 设置页里的 Security
    let nodes = dump_retry(adb, 3).await?;
    let security = nodes
        .iter()
        .find(|n| {
            let t = n.text.trim();
            t.eq_ignore_ascii_case("security") || t == "安全"
        })
        .ok_or_else(|| anyhow!("未能在设置页中找到「Security / 安全」。"))?;
    uia::tap_node(adb, security).await?;
    tokio::time::sleep(Duration::from_millis(1600)).await;

    // 4) 确认真的到了安全页
    // 条目文案随系统语言变化：英文 Port control / Listening ports /
    // Auto-forward all ports / Saved allowed ports，中文
    // 端口控制 / 正在监听的端口 / 自动转发所有端口 / 已保存的允许端口。
    let nodes = dump_retry(adb, 3).await?;
    let on_security = nodes.iter().any(|n| {
        let t = format!("{} {}", n.text, n.desc).to_lowercase();
        let en = t.contains("port")
            && (t.contains("control")
                || t.contains("listening")
                || t.contains("forward")
                || t.contains("allowed"));
        let zh = t.contains("端口")
            && (t.contains("控制")
                || t.contains("监听")
                || t.contains("转发")
                || t.contains("允许"));
        en || zh
    });
    if !on_security {
        bail!("未能进入 Terminal 的「设置 → 安全」页面。");
    }
    Ok(())
}

/// 找到顶栏的「更多选项（⋮）」按钮。
fn find_more_options<'a>(nodes: &'a [UiNode]) -> Option<&'a UiNode> {
    let visible = |n: &&UiNode| n.bounds.x2 > n.bounds.x1 && n.bounds.y2 > n.bounds.y1;
    nodes
        .iter()
        .filter(visible)
        .find(|n| {
            let d = n.desc.to_lowercase();
            d.contains("more options") || d.contains("更多选项")
        })
        .or_else(|| {
            // 兜底：顶栏右侧的按钮
            nodes
                .iter()
                .filter(visible)
                .find(|n| n.clickable && n.bounds.y2 < 400 && n.bounds.x1 > 700)
        })
}

/// 在安全页里找到某个端口所在行的开关。
fn port_row<'a>(nodes: &'a [UiNode], port: &str) -> Option<&'a UiNode> {
    let label = nodes.iter().find(|n| {
        let t = n.text.trim();
        t.contains(&format!("({})", port))
            || t.split(|c: char| !c.is_ascii_digit()).any(|w| w == port)
    })?;
    nodes.iter().find(|n| {
        n.checkable
            && n.bounds.y1 < label.bounds.y2
            && n.bounds.y2 > label.bounds.y1
    })
}

/// 找到端口对话框里的「保存 / 确定」按钮。
///
/// 只按 text 匹配（不匹配 content-desc），避免和页面右下角的「+」按钮
/// （desc = Add）混淆。
fn find_dialog_save<'a>(nodes: &'a [UiNode]) -> Option<&'a UiNode> {
    if let Some(btn) = nodes.iter().find(|n| {
        n.id == "android:id/button1" && n.enabled && n.bounds.x2 > n.bounds.x1
    }) {
        return Some(btn);
    }
    const LABELS: [&str; 5] = ["Save", "保存", "确定", "OK", "完成"];
    nodes.iter().find(|n| {
        let t = n.text.trim();
        LABELS.iter().any(|l| t.eq_ignore_ascii_case(l))
    })
}

/// 打开「+」对话框，输入端口并保存。
///
/// 关键：点进输入框后软键盘弹出，Compose 对话框会整体上移。如果沿用
/// 输入前的节点坐标去点「保存」，就会点到对话框外的遮罩上，直接把对话框
/// 关掉——表现为「输入了但没保存」。因此保存按钮必须从**输入后的最新
/// 节点树**里取坐标。
async fn add_port_via_dialog(adb: &Adb, port: u16) -> Result<()> {
    let port_str = port.to_string();

    let nodes = dump_retry(adb, 3).await?;
    let add_btn = nodes
        .iter()
        .find(|n| {
            let d = n.desc.trim();
            let t = n.text.trim();
            d.eq_ignore_ascii_case("add")
                || d == "添加"
                || d == "新增"
                || t.eq_ignore_ascii_case("add")
                || t == "添加"
                || t == "新增"
        })
        .ok_or_else(|| anyhow!("未能在安全页找到「添加端口」按钮（+）。"))?;
    uia::tap_node(adb, add_btn).await?;
    tokio::time::sleep(Duration::from_millis(900)).await;

    let dialog = dump_retry(adb, 3).await?;
    let field = dialog
        .iter()
        .find(|n| n.class.contains("EditText") && n.enabled)
        .cloned()
        .ok_or_else(|| anyhow!("未能打开端口输入框。"))?;
    uia::tap_node(adb, &field).await?;
    // 等软键盘完整弹出，否则接下来 dump 到的还是旧布局
    tokio::time::sleep(Duration::from_millis(900)).await;
    uia::type_text(adb, &port_str).await?;
    tokio::time::sleep(Duration::from_millis(600)).await;

    // 重新 dump：此时对话框已经上移，保存按钮坐标是新的
    let after_typing = dump_retry(adb, 3).await?;
    let save = find_dialog_save(&after_typing)
        .cloned()
        .ok_or_else(|| anyhow!("未能在端口对话框中找到「保存」按钮。"))?;
    uia::tap_node(adb, &save).await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let after = dump_retry(adb, 3).await?;
    // 对话框还开着说明没保存成功（输入框里也带着端口号，不能只看端口文本）
    let dialog_open = after
        .iter()
        .any(|n| n.class.contains("EditText") && n.enabled);
    if dialog_open {
        bail!("端口 {} 未保存：对话框仍然打开。", port);
    }
    if !after.iter().any(|n| n.text.contains(&port_str)) {
        bail!("端口 {} 添加后未出现在允许列表中。", port);
    }
    Ok(())
}

/// 路径 2（新版）：在「设置 → 安全」页面里放行端口。
async fn enable_port_via_security(adb: &Adb, port: u16) -> Result<PortResult> {
    open_security_page(adb).await?;
    let port_str = port.to_string();

    let nodes = dump_retry(adb, 3).await?;

    // 情况 1：Listening ports 里已经有该端口 → 打开同行开关
    if let Some(sw) = port_row(&nodes, &port_str) {
        if sw.checked {
            return Ok(PortResult {
                port,
                ok: true,
                action: "already_enabled".to_string(),
                message: format!("端口 {} 已在 Terminal「设置 → 安全」中放行。", port),
                active: true,
            });
        }
        uia::tap_node(adb, sw).await?;
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let after = dump_retry(adb, 3).await?;
        let enabled = port_row(&after, &port_str)
            .map(|s| s.checked)
            .unwrap_or(true);
        return Ok(PortResult {
            port,
            ok: enabled,
            action: "switched_on".to_string(),
            message: if enabled {
                format!("已在 Terminal「设置 → 安全」中放行端口 {}。", port)
            } else {
                format!("端口 {} 的开关未能打开，请手动确认。", port)
            },
            active: true,
        });
    }

    // 情况 2：端口已出现在「Saved allowed ports」里（这一栏没有开关）
    if nodes.iter().any(|n| n.text.contains(&port_str)) {
        return Ok(PortResult {
            port,
            ok: true,
            action: "already_saved".to_string(),
            message: format!("端口 {} 已在 Terminal「设置 → 安全」的允许列表中。", port),
            active: false,
        });
    }

    // 情况 3：没有该端口 → 点右下角「+」新增。
    // 自动化过程中对话框可能被误触关闭，失败时重新进入安全页再试，
    // 最多 3 次。
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(800)).await;
            open_security_page(adb).await?;
        }
        match add_port_via_dialog(adb, port).await {
            Ok(()) => {
                return Ok(PortResult {
                    port,
                    ok: true,
                    action: "added".to_string(),
                    message: format!("已把端口 {} 加入「设置 → 安全」的允许列表。", port),
                    active: false,
                });
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    bail!("端口 {} 添加失败：{}", port, last_err)
}

/// 路径 2：在「端口控制」界面里放行（老版本）。
async fn enable_port_via_settings(adb: &Adb, port: u16) -> Result<PortResult> {
    open_settings(adb).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let nodes = uia::dump(adb).await?;
    let port_str = port.to_string();

    // 情况 1：端口已在「正在监听」列表中
    if let Some(sw) = switch_in_row(&nodes, &port_str) {
        if sw.checked {
            return Ok(PortResult {
                port,
                ok: true,
                action: "already_enabled".to_string(),
                message: format!("端口 {} 已处于放行状态。", port),
                active: true,
            });
        }
        uia::tap_node(adb, sw).await?;
        tokio::time::sleep(Duration::from_millis(900)).await;
        let after = uia::dump(adb).await?;
        let enabled = switch_in_row(&after, &port_str).map(|s| s.checked).unwrap_or(true);
        return Ok(PortResult {
            port,
            ok: enabled,
            action: "switched_on".to_string(),
            message: if enabled {
                format!("已在「端口控制」中放行端口 {}。", port)
            } else {
                format!("端口 {} 的开关未能打开，请手动确认。", port)
            },
            active: true,
        });
    }

    // 情况 2：端口已在「已保存的允许端口」列表中
    if in_saved_list(&nodes, port) {
        return Ok(PortResult {
            port,
            ok: true,
            action: "already_enabled".to_string(),
            message: format!("端口 {} 已在允许列表中，虚拟机开始监听后自动生效。", port),
            active: false,
        });
    }

    // 情况 3：新增一个允许端口
    let add_btn = uia::find_by_id(&nodes, "settings_port_forwarding_inactive_add_button")
        .or_else(|| {
            nodes.iter().find(|n| {
                n.clickable
                    && (n.desc.to_lowercase().contains("add")
                        || n.text.trim().eq_ignore_ascii_case("add")
                        || n.text.trim() == "添加")
            })
        })
        .ok_or_else(|| anyhow!("未能在端口控制界面中找到「添加」按钮。"))?;
    uia::tap_node(adb, add_btn).await?;
    tokio::time::sleep(Duration::from_millis(900)).await;

    let dialog_nodes = uia::dump(adb).await?;
    let field = uia::find_by_id(&dialog_nodes, "settings_port_forwarding_inactive_add_dialog_text")
        .or_else(|| {
            dialog_nodes
                .iter()
                .find(|n| n.class.contains("EditText") && n.enabled)
        })
        .ok_or_else(|| anyhow!("未能打开端口输入框。"))?;
    uia::tap_node(adb, field).await?;
    // 等软键盘弹出，之后对话框位置会变，保存按钮必须重新定位
    tokio::time::sleep(Duration::from_millis(900)).await;
    uia::type_text(adb, &port_str).await?;
    tokio::time::sleep(Duration::from_millis(600)).await;

    let after_typing = dump_retry(adb, 3).await?;
    match find_dialog_save(&after_typing) {
        Some(btn) => uia::tap_node(adb, btn).await?,
        None => uia::press_enter(adb).await?,
    }
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let after = uia::dump(adb).await?;
    let added = in_saved_list(&after, port)
        || switch_in_row(&after, &port_str).is_some()
        || after.iter().any(|n| n.text.trim() == port_str);
    if !added {
        bail!("端口 {} 添加失败。", port);
    }

    Ok(PortResult {
        port,
        ok: true,
        action: "added".to_string(),
        message: format!("已把端口 {} 加入允许列表。", port),
        active: false,
    })
}

/// 放行一个端口：新版走「设置 → 安全」，失败再尝试通知栏与旧版界面。
pub async fn enable_port(adb: &Adb, port: u16) -> Result<PortResult> {
    // 1) 新版（new2）Terminal：⋮ → Settings → Security
    match enable_port_via_security(adb, port).await {
        Ok(r) => Ok(r),
        Err(e_new) => {
            // 2) 通知栏（新老版本通用）
            if let Ok(true) = enable_port_via_notification(adb, port).await {
                let active = device_listens_on(adb, port).await;
                return Ok(PortResult {
                    port,
                    ok: true,
                    action: "notification".to_string(),
                    message: if active {
                        format!("已在通知栏确认放行端口 {}，手机端已开始转发。", port)
                    } else {
                        format!(
                            "已在通知栏确认放行端口 {}。若稍后仍连不上，请确认 Terminal 应用保持运行。",
                            port
                        )
                    },
                    active,
                });
            }

            // 3) 老版本的独立端口控制 Activity
            match enable_port_via_settings(adb, port).await {
                Ok(r) => Ok(r),
                Err(e_old) => Ok(PortResult {
                    port,
                    ok: false,
                    action: "manual".to_string(),
                    message: format!(
                        "自动放行端口 {} 失败：{}\n（旧版端口控制界面：{}）\n\n\
                         请手动处理：打开手机上的 Terminal 应用 → 右上角 ⋮ → Settings → Security，\
                         在「Listening ports」里打开端口 {} 的开关；\
                         若列表中没有该端口，点右下角「+」输入 {} 保存即可。",
                        port, e_new, e_old, port, port
                    ),
                    active: false,
                }),
            }
        }
    }
}
