//! 手机端端口中继：让 PC 能通过 `adb forward` 连到虚拟机里的服务。
//!
//! AVF 虚拟机通过 `avf_tap*` 虚拟网卡挂在手机上，手机自己就是虚拟机的网关
//! （虚拟机的默认路由指向手机），所以**虚拟机能访问手机，但 PC 访问不到虚拟机**。
//!
//! 官方做法是 Terminal 应用的「端口放行」，但它在部分机型（Redmi K Pad /
//! Android 16）上切换开关会让 `DebianServiceImpl.updateListeningPorts()` 卡死
//! 主线程，端口也并没有真的被绑定。因此这里用 Android 自带的 toybox nc 在手机上
//! 跑一个最简中继：手机监听 `<port>`，每来一个连接就转发到虚拟机的 `<port>`。
//!
//! 中继是否可用不靠“端口在监听”猜：启动后会用 `toybox nc 127.0.0.1 <port>`
//! 真正走一遍「手机 → 中继 → 虚拟机 → VNC 服务」，读到 `RFB` 握手串才算成功。
//! 这样既验证了中继进程，也验证了虚拟机地址选得对。

use anyhow::{bail, Result};
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::adb::Adb;

/// AVF 虚拟网卡名前缀（不同版本后缀不同，如 `avf_tap_fixed` / `avf_tap0`）。
const TAP_PREFIX: &str = "avf_tap";

/// 找到 AVF 虚拟网卡名、手机自身地址与子网前缀长度。
async fn tap_interface(adb: &Adb) -> Result<(String, Ipv4Addr, u8)> {
    let out = adb.shell("ip -4 -o addr show").await.unwrap_or_default();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let _index = it.next();
        let Some(name) = it.next() else { continue };
        if !name.starts_with(TAP_PREFIX) {
            continue;
        }
        let Some(cidr) = it.find(|t| t.contains('/')) else {
            continue;
        };
        let (addr, prefix) = cidr.split_once('/').unwrap_or((cidr, "24"));
        let Ok(ip) = addr.parse::<Ipv4Addr>() else {
            continue;
        };
        let prefix_len = prefix.parse::<u8>().unwrap_or(24).min(32);
        return Ok((name.to_string(), ip, prefix_len));
    }
    bail!(
        "未找到 AVF 虚拟网卡（{}*），Linux 虚拟机可能还没有启动或已退出。",
        TAP_PREFIX
    )
}

/// 从手机的邻居表里读出虚拟机的候选地址（排除手机自己）。
async fn neighbour_ips(adb: &Adb, tap: &str, self_ip: Ipv4Addr) -> Vec<String> {
    let out = adb
        .shell(&format!("ip neigh show dev {}", tap))
        .await
        .unwrap_or_default();

    let mut ips: Vec<String> = Vec::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let Some(raw) = it.next() else { continue };
        let Ok(ip) = raw.parse::<Ipv4Addr>() else {
            continue;
        };
        if ip == self_ip || ips.iter().any(|x| x == raw) {
            continue;
        }
        let state = it.last().unwrap_or("");
        if state.contains("FAILED") || state.contains("INCOMPLETE") {
            continue;
        }
        ips.push(ip.to_string());
    }
    ips
}

/// 生成同一子网里最可能分配给虚拟机的地址（最多 64 个）。
fn subnet_candidates(self_ip: Ipv4Addr, prefix_len: u8) -> Vec<String> {
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len)
    };
    let network = u32::from(self_ip) & mask;
    let host_count = (1u64 << (32 - prefix_len.min(31))).saturating_sub(2).min(64);
    let self_text = self_ip.to_string();
    (1..=host_count)
        .map(|offset| Ipv4Addr::from((network + offset as u32) as u32).to_string())
        .filter(|ip| ip != &self_text)
        .collect()
}

/// 对子网做一次并行 ping，促使手机把虚拟机的 MAC/IP 写进邻居表。
async fn ping_sweep(adb: &Adb, candidates: &[String]) {
    if candidates.is_empty() {
        return;
    }
    let mut cmd = String::from("for ip in");
    for ip in candidates {
        cmd.push(' ');
        cmd.push_str(ip);
    }
    cmd.push_str("; do ping -c 1 -W 1 \"$ip\" >/dev/null 2>&1 & done; wait");
    let _ = adb.shell(&cmd).await;
}

/// 手机端是否真的在监听某个 TCP 端口（只认 LISTEN 状态）。
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

/// 指定端口上是否已经有本工具拉起的 nc 中继进程。
async fn relay_process(adb: &Adb, port: u16) -> bool {
    adb.shell(&format!(
        "pgrep -f \"[n]c -L -p {} \" >/dev/null 2>&1 && echo yes || echo no",
        port
    ))
    .await
    .map(|s| s.trim() == "yes")
    .unwrap_or(false)
}

/// 选择设备端的中继监听端口。
///
/// **不能直接用 VNC 端口**：Redmi K Pad / Android 16 上 Terminal 的端口转发会在
/// `127.0.0.1:5901` 留下一个“只监听、不转发”的僵尸 socket，中继无法再绑定 5901。
/// 因此中继改用 `5901 + 10000 = 15901` 这类独立端口，再由
/// `adb forward tcp:5901 tcp:15901` 把 PC 的 5901 指过来。
///
/// 端口选择规则：
///   * 空闲 → 直接用；
///   * 已经有本工具的中继且能读到 RFB → 复用；
///   * 有本工具的中继但已经失效 → 杀掉重启；
///   * 被其它进程占用 → 试下一个。
pub async fn choose_relay_port(adb: &Adb, public_port: u16) -> u16 {
    let base = if public_port <= 55535 {
        public_port + 10000
    } else {
        public_port.saturating_sub(10000)
    };

    for offset in 0..10u16 {
        let port = base.saturating_add(offset);
        if port == 0 {
            continue;
        }
        if !device_listens(adb, port).await {
            return port;
        }
        // 只有本工具拉起的 nc 中继才可以复用/重启；其它进程（哪怕也是 VNC）
        // 一律避开，避免把 PC 接到一个无关的桌面上。
        if relay_process(adb, port).await {
            if device_serves_vnc(adb, "127.0.0.1", port).await {
                return port;
            }
            stop_relay(adb, port).await;
            return port;
        }
    }
    base
}

/// 从手机（或手机对虚拟机的直连）读一次 RFB 握手串，判断该地址是否提供 VNC。
pub async fn device_serves_vnc(adb: &Adb, host: &str, port: u16) -> bool {
    let out = adb
        .shell(&format!(
            "toybox nc -w 2 {} {} < /dev/null 2>/dev/null | head -c 12",
            host, port
        ))
        .await
        .unwrap_or_default();
    out.contains("RFB")
}

/// 从手机侧测试某个 TCP 端口能否建立连接（不要求对端先发数据）。
///
/// `toybox nc` 连接成功后不会自己退出（会一直读对端数据），必须用
/// `toybox timeout` 包一层：退出码 0（正常 EOF）或 124（超时）都表示端口可达；
/// 连接被拒绝时才会立刻返回非 0。
pub async fn tcp_connect_ok(adb: &Adb, host: &str, port: u16) -> bool {
    let out = adb
        .shell(&format!(
            "toybox timeout 3 toybox nc {host} {port} < /dev/null > /dev/null 2>&1; \
             code=$?; if [ \"$code\" = 0 ] || [ \"$code\" = 124 ]; then echo 0; else echo 1; fi",
            host = host,
            port = port
        ))
        .await
        .unwrap_or_default();
    out.trim() == "0"
}

/// 直接在手机上拉起一个监听 `listen_port` 的中继，转发到 `guest_ip:guest_port`。
///
/// 不做协议层面的校验，适合音频串流；VNC 请用 [`start_relay`]。
async fn start_relay_raw(
    adb: &Adb,
    listen_port: u16,
    guest_ip: &str,
    guest_port: u16,
) -> Result<()> {
    stop_relay(adb, listen_port).await;
    let cmd = format!(
        "nohup toybox nc -L -p {listen} toybox nc {ip} {guest} \
         >/dev/null 2>&1 & echo started",
        listen = listen_port,
        ip = guest_ip,
        guest = guest_port
    );
    adb.shell(&cmd).await?;
    Ok(())
}

/// 给音频串流建一条手机端中继，返回手机实际监听（播放器应连接）的端口。
///
/// 优先使用 `base_port`；被占用时依次尝试后面几个端口。每个候选都要
/// 从手机侧真的连一次 `127.0.0.1:<port>` 才算成功。
pub async fn open_port_relay(
    adb: &Adb,
    base_port: u16,
    guest_ip: &str,
    guest_port: u16,
) -> Result<u16> {
    for offset in 0..10u16 {
        let port = base_port.saturating_add(offset);
        if port == 0 {
            continue;
        }
        if device_listens(adb, port).await {
            // 可能上一轮已经建好，能连上就直接复用。
            if tcp_connect_ok(adb, "127.0.0.1", port).await {
                return Ok(port);
            }
            continue;
        }
        if start_relay_raw(adb, port, guest_ip, guest_port).await.is_err() {
            continue;
        }
        for _ in 0..12 {
            if tcp_connect_ok(adb, "127.0.0.1", port).await {
                return Ok(port);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let _ = stop_relay(adb, port).await;
    }
    bail!(
        "手机端的音频中继没有启动成功（{}:{} 之后的端口都被占用或不可用）。",
        base_port,
        guest_port
    )
}


/// 找到真正在监听 `port` 的虚拟机地址。
///
/// 先看邻居表（快），没有再 ping 一遍子网（促使 ARP 更新），最后才逐个试
/// 子网里的常见地址。只返回**已经读到 RFB 握手串**的地址，避免把中继指向
/// 一个没有服务的地址。
pub async fn find_vnc_guest(adb: &Adb, port: u16) -> Result<String> {
    let (tap, self_ip, prefix_len) = tap_interface(adb).await?;
    let candidates = subnet_candidates(self_ip, prefix_len);

    let mut tried: Vec<String> = Vec::new();
    for ip in neighbour_ips(adb, &tap, self_ip).await {
        tried.push(ip.clone());
        if device_serves_vnc(adb, &ip, port).await {
            return Ok(ip);
        }
    }

    // 邻居表里没有可用条目（或地址过期）时，主动 ping 一遍子网。
    ping_sweep(adb, &candidates).await;
    for ip in neighbour_ips(adb, &tap, self_ip).await {
        if tried.iter().any(|x| x == &ip) {
            continue;
        }
        tried.push(ip.clone());
        if device_serves_vnc(adb, &ip, port).await {
            return Ok(ip);
        }
    }

    // 邻居表仍然为空时，直接试最可能的几个地址。
    for ip in candidates.iter().take(8) {
        if !tried.iter().any(|x| x == ip) && device_serves_vnc(adb, ip, port).await {
            return Ok(ip.clone());
        }
    }

    bail!(
        "没有找到正在监听 {} 端口的虚拟机（网卡 {}，手机地址 {}）。\n\
         请确认 Linux 虚拟机已启动，并且「TigerVNC 服务」步骤已经成功完成。",
        port,
        tap,
        self_ip
    )
}

/// 停掉本工具之前拉起的中继。
///
/// `pkill -f` 的模式里故意用 `[n]c`：正则 `[n]c` 能匹配进程参数里的 `nc`，
/// 却不会匹配到执行这条命令的 shell 自己（它的命令行里是字面量 `[n]c`），
/// 否则 pkill 会把当前 adb shell 一起杀掉。
async fn stop_relay(adb: &Adb, port: u16) {
    let _ = adb
        .shell(&format!("pkill -f \"[n]c -L -p {}\" 2>/dev/null", port))
        .await;
    tokio::time::sleep(Duration::from_millis(350)).await;
}

/// 在手机上拉起中继：手机监听 `listen_port`，转发到虚拟机 `guest_ip:guest_port`。
///
/// 启动后必须能通过中继读到 VNC 握手串才算成功；否则杀掉中继并返回错误，
/// 让调用方去尝试 Terminal 的端口放行。
pub async fn start_relay(
    adb: &Adb,
    listen_port: u16,
    guest_ip: &str,
    guest_port: u16,
) -> Result<String> {
    start_relay_raw(adb, listen_port, guest_ip, guest_port).await?;

    for _ in 0..24 {
        if device_serves_vnc(adb, "127.0.0.1", listen_port).await {
            return Ok(format!(
                "手机已在 :{} 监听，并转发到虚拟机 {}:{}",
                listen_port, guest_ip, guest_port
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let _ = stop_relay(adb, listen_port).await;
    bail!(
        "手机端的端口中继没有启动成功（:{} 未监听或收不到 VNC 握手）。\n\
         请确认虚拟机里的服务已经启动，并重试本步骤。",
        listen_port
    )
}
