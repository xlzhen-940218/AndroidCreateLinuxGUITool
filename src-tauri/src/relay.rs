//! 手机端端口中继：让 PC 能通过 `adb forward` 连到虚拟机里的服务。
//!
//! AVF 虚拟机通过 `avf_tap*` 虚拟网卡挂在手机上，手机自己就是虚拟机的网关
//! （虚拟机的默认路由指向手机），所以**虚拟机能访问手机，但 PC 访问不到虚拟机**。
//!
//! Terminal 应用自带的「端口放行」是官方做法，但它依赖应用内的原生转发实现：
//! 实测在部分机型（Redmi K Pad / Android 16）上切换端口开关会让
//! `DebianServiceImpl.updateListeningPorts()` 这个 JNI 调用**卡死主线程**，
//! 最终 ANR，端口也并没有真的被绑定。
//!
//! 因此这里改用 Android 自带的 toybox nc 在手机上跑一个最简中继：
//! 手机监听 `<port>`，每来一个连接就转发到虚拟机的 `<guest_ip>:<port>`。
//! 再配合 `adb forward tcp:<port> tcp:<port>`，PC 的 `127.0.0.1:<port>`
//! 就等价于虚拟机里的 `<port>`。
//!
//! 虚拟机地址不需要问虚拟机：手机自己的邻居表（ARP）里就有。

use anyhow::{bail, Result};
use std::time::Duration;

use crate::adb::Adb;

/// AVF 虚拟网卡名前缀（不同版本后缀不同，如 `avf_tap_fixed` / `avf_tap0`）。
const TAP_PREFIX: &str = "avf_tap";

/// 找到 AVF 虚拟网卡名，以及手机自己在这张网卡上的地址。
async fn tap_interface(adb: &Adb) -> Result<(String, String)> {
    let out = adb.shell("ip -4 -o addr show").await.unwrap_or_default();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let _index = it.next();
        let Some(name) = it.next() else { continue };
        if !name.starts_with(TAP_PREFIX) {
            continue;
        }
        let addr = it
            .find(|t| t.contains('/'))
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        return Ok((name.to_string(), addr));
    }
    bail!(
        "未找到 AVF 虚拟网卡（{}*），Linux 虚拟机可能还没有启动或已退出。",
        TAP_PREFIX
    );
}

/// 从手机的邻居表里列出虚拟机的候选 IPv4 地址。
///
/// 虚拟机和手机之间是直连的二层网络，手机会定期和虚拟机交换 ARP，
/// 所以邻居表里通常能直接看到虚拟机的地址。
pub async fn discover_guest_ips(adb: &Adb) -> Result<Vec<String>> {
    let (tap, self_ip) = tap_interface(adb).await?;
    let out = adb
        .shell(&format!("ip neigh show dev {}", tap))
        .await
        .unwrap_or_default();

    let mut ips: Vec<String> = Vec::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let ip = it.next().unwrap_or("");
        if ip.parse::<std::net::Ipv4Addr>().is_err() {
            continue;
        }
        if ip == self_ip || ips.iter().any(|x| x == ip) {
            continue;
        }
        // 只认可达的条目；FAILED 的条目说明虚拟机已经不在了
        let state = it.last().unwrap_or("");
        if !(state.contains("REACHABLE") || state.contains("STALE") || state.contains("DELAY")) {
            continue;
        }
        ips.push(ip.to_string());
    }
    Ok(ips)
}

/// 手机端是否真的在监听某个 TCP 端口。
///
/// 直接读 `/proc/net/tcp{,6}`，并要求状态是 `0A`（LISTEN），
/// 避免把「连出去的连接」误判成监听。
pub async fn device_listens(adb: &Adb, port: u16) -> bool {
    let hex = format!("{:04X}", port);
    let out = adb
        .shell(&format!(
            "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null \
             | awk '$4 == \"0A\" && $2 ~ /:{hex}$/' | wc -l",
            hex = hex
        ))
        .await
        .unwrap_or_default();
    out.trim().parse::<u32>().unwrap_or(0) > 0
}

/// 从手机试连虚拟机的某个端口，看它是不是 VNC 服务（返回 RFB 握手串）。
async fn guest_serves_vnc(adb: &Adb, ip: &str, port: u16) -> bool {
    let out = adb
        .shell(&format!(
            "toybox nc -w 2 {} {} < /dev/null 2>/dev/null | head -c 12",
            ip, port
        ))
        .await
        .unwrap_or_default();
    out.contains("RFB")
}

/// 停掉可能残留的旧中继。
///
/// 虚拟机重启后地址会变，旧中继还占着端口，必须先杀掉再重新拉起。
async fn stop_relay(adb: &Adb, port: u16) {
    let _ = adb
        .shell(&format!("pkill -f 'nc -L -p {}' 2>/dev/null", port))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// 在手机上拉起中继：手机监听 `port`，转发到虚拟机 `guest_ip:port`。
pub async fn start_relay(adb: &Adb, port: u16, guest_ip: &str) -> Result<String> {
    stop_relay(adb, port).await;

    // toybox nc 的 `-L` 是服务模式：监听并为每个进来的连接启动一条命令。
    // `nohup ... &` 让它脱离 adb shell 继续运行，不随 shell 退出而结束。
    let cmd = format!(
        "nohup toybox nc -L -p {port} toybox nc {ip} {port} >/dev/null 2>&1 & echo started",
        port = port,
        ip = guest_ip
    );
    adb.shell(&cmd).await?;

    for _ in 0..20 {
        if device_listens(adb, port).await {
            return Ok(format!(
                "手机已在 :{} 监听，并转发到虚拟机 {}:{}",
                port, guest_ip, port
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    bail!(
        "手机端的端口中继没有启动成功（:{} 未监听）。\n\
         请确认虚拟机里的服务已经启动，并重试本步骤。",
        port
    )
}

/// 建立「手机端口 → 虚拟机端口」的通道，返回一段可展示的说明。
///
/// 优先挑真正提供 VNC 服务的那个虚拟机地址；探测不出来时退回第一个候选。
pub async fn ensure_relay(adb: &Adb, port: u16) -> Result<String> {
    let ips = discover_guest_ips(adb).await?;
    if ips.is_empty() {
        bail!(
            "没有在 AVF 虚拟网卡上找到虚拟机的地址。\n\
             请确认 Linux 虚拟机已启动（打开 Terminal 应用看一眼即可）。"
        );
    }

    let mut chosen = ips[0].clone();
    for ip in &ips {
        if guest_serves_vnc(adb, ip, port).await {
            chosen = ip.clone();
            break;
        }
    }

    start_relay(adb, port, &chosen).await
}
