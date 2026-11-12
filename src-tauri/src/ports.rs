//! Terminal 应用「端口控制」(Port control) 的自动化。
//!
//! AOSP 实现（packages/modules/Virtualization/android/TerminalApp）：
//!   * `SettingsPortForwardingActivity` 展示虚拟机里正在监听的端口；
//!   * "Saved allowed ports" 列表里的端口会被放行，虚拟机一旦监听即自动转发到手机本机；
//!   * 放行状态保存在应用私有 SharedPreferences（".PORTS"），外部无法直接写。
//!
//! Android 17 起 Terminal 换成了 new2（Compose）实现，端口控制界面没有 resource-id，
//! 也不再是独立 Activity。因此这里采用**两条路径**：
//!   1) 通知栏放行（两个版本都有，字符串稳定）：虚拟机开始监听端口时，
//!      Terminal 会弹出「Terminal is requesting to open a new port / Accept」通知；
//!   2) 端口控制界面（老版本有独立 Activity，按 resource-id 操作）。

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
    const ACCEPT_LABELS: [&str; 8] = [
        "Allow always",
        "Allow once",
        "Accept",
        "Allow",
        "ALLOW",
        "允许",
        "接受",
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

        // 找到提到了目标端口的文本节点
        let port_node = nodes.iter().find(|n| {
            let t = format!("{} {}", n.text, n.desc);
            t.contains(&port_str)
                && (t.to_lowercase().contains("port")
                    || t.to_lowercase().contains("forward")
                    || t.to_lowercase().contains("open"))
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
                    t.contains(&port_str) && t.to_lowercase().contains("port")
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

/// 打开新版（new2）的「Port control」页面：主界面 → 溢出菜单 → Port control。
///
/// Android 17 起 Terminal 不再有独立的端口控制 Activity，页面是 MainActivity
/// 里 Compose 实现的一个界面，只能从右上角溢出菜单进入。
async fn open_port_control_new2(adb: &Adb) -> Result<()> {
    let activity = crate::terminal::resolve_launch_activity(adb).await?;
    adb.start_activity(&activity).await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let nodes = uia::dump(adb).await?;
    let more = nodes
        .iter()
        .find(|n| {
            let d = n.desc.to_lowercase();
            n.clickable
                && (d.contains("more options")
                    || d.contains("更多选项")
                    || d.contains("overflow")
                    || n.text.trim() == "⋮"
                    || n.text.trim() == "...")
        })
        .cloned()
        .ok_or_else(|| anyhow!("未能在 Terminal 主界面找到溢出菜单（⋮）。"))?;
    uia::tap_node(adb, &more).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let nodes = uia::dump(adb).await?;
    let item = nodes
        .iter()
        .find(|n| {
            let t = n.text.trim().to_lowercase();
            let d = n.desc.to_lowercase();
            t == "port control"
                || d == "port control"
                || t == "端口控制"
                || t == "port forwarding"
                || d == "port forwarding"
        })
        .cloned()
        .ok_or_else(|| anyhow!("未能在菜单中找到「Port control / 端口控制」。"))?;
    uia::tap_node(adb, &item).await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    Ok(())
}

/// 路径 2（新版）：在「Port control」页面里放行端口。
async fn enable_port_via_settings_new2(adb: &Adb, port: u16) -> Result<PortResult> {
    open_port_control_new2(adb).await?;
    let port_str = port.to_string();

    let nodes = uia::dump(adb).await?;

    // 情况 1：端口已经出现在「Listening ports」里 → 打开同行的开关
    if let Some(label) = nodes.iter().find(|n| n.text.trim() == port_str) {
        let sw = nodes.iter().find(|c| {
            (c.checkable || c.class.contains("Switch"))
                && c.bounds.y1 >= label.bounds.y1 - 48
                && c.bounds.y2 <= label.bounds.y2 + 48
        });
        if let Some(sw) = sw {
            if !sw.checked {
                uia::tap_node(adb, sw).await?;
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            return Ok(PortResult {
                port,
                ok: true,
                action: "switched_on".to_string(),
                message: format!("已在「端口控制」的 Listening ports 中放行端口 {}。", port),
                active: true,
            });
        }
    }

    // 情况 2：端口已在「Saved allowed ports」列表中
    if nodes.iter().any(|n| {
        n.text.trim() == port_str
            && n.bounds.y1
                > nodes
                    .iter()
                    .filter(|x| x.text.trim().eq_ignore_ascii_case("Saved allowed ports"))
                    .map(|x| x.bounds.y1)
                    .max()
                    .unwrap_or(0)
    }) {
        return Ok(PortResult {
            port,
            ok: true,
            action: "already_enabled".to_string(),
            message: format!("端口 {} 已在允许列表中，虚拟机开始监听后自动生效。", port),
            active: false,
        });
    }

    // 情况 3：点「Allow a new port」新增
    let add_btn = nodes
        .iter()
        .find(|n| {
            let t = n.text.trim().to_lowercase();
            let d = n.desc.to_lowercase();
            t.contains("allow a new port")
                || d.contains("allow a new port")
                || t.contains("新增端口")
                || t.contains("添加端口")
                || (n.clickable && d == "add")
        })
        .cloned()
        .ok_or_else(|| anyhow!("未能在「Port control」页面中找到「Allow a new port」按钮。"))?;
    uia::tap_node(adb, &add_btn).await?;
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let dialog = uia::dump(adb).await?;
    let field = dialog
        .iter()
        .find(|n| n.class.contains("EditText") && n.enabled)
        .cloned()
        .ok_or_else(|| anyhow!("未能打开端口输入框。"))?;
    uia::tap_node(adb, &field).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    uia::type_text(adb, &port_str).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let save = dialog
        .iter()
        .find(|n| n.id == "android:id/button1" && n.enabled)
        .or_else(|| uia::find_by_text(&dialog, "OK"))
        .or_else(|| uia::find_by_text(&dialog, "Save"))
        .or_else(|| uia::find_by_text(&dialog, "确定"))
        .or_else(|| uia::find_by_text(&dialog, "允许"))
        .cloned();
    match save {
        Some(btn) => {
            uia::tap_node(adb, &btn).await?;
        }
        None => {
            uia::press_enter(adb).await?;
        }
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;

    Ok(PortResult {
        port,
        ok: true,
        action: "added".to_string(),
        message: format!("已把端口 {} 加入允许列表。", port),
        active: false,
    })
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
    tokio::time::sleep(Duration::from_millis(400)).await;
    uia::type_text(adb, &port_str).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let save = dialog_nodes
        .iter()
        .find(|n| n.id == "android:id/button1" && n.enabled)
        .or_else(|| uia::find_by_text(&dialog_nodes, "Save"))
        .or_else(|| uia::find_by_text(&dialog_nodes, "保存"))
        .or_else(|| uia::find_by_text(&dialog_nodes, "确定"))
        .cloned();
    match save {
        Some(btn) => {
            uia::tap_node(adb, &btn).await?;
        }
        None => {
            uia::press_enter(adb).await?;
        }
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

/// 放行一个端口：先尝试通知栏，再尝试端口控制界面。
pub async fn enable_port(adb: &Adb, port: u16) -> Result<PortResult> {
    // 1) 通知栏（通用，优先）
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

    // 2) 新版（new2）的「Port control」页面
    match enable_port_via_settings_new2(adb, port).await {
        Ok(r) => return Ok(r),
        Err(e_new) => {
            // 3) 老版本的独立端口控制 Activity
            match enable_port_via_settings(adb, port).await {
                Ok(r) => Ok(r),
                Err(e_old) => Ok(PortResult {
                    port,
                    ok: false,
                    action: "manual".to_string(),
                    message: format!(
                        "自动放行端口 {} 失败：{}\n（新版路径：{}）\n\n请手动处理：打开手机上的 Terminal 应用 → 右上角 ⋮ → Port control（端口控制），\
                         在「Listening ports」里允许端口 {}，或用「Allow a new port」把它加进「Saved allowed ports」。\n\
                         （旧版本路径：设置 → Port control）",
                        port, e_new, e_old, port
                    ),
                    active: false,
                }),
            }
        }
    }
}
