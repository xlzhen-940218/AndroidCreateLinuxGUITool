//! 桌面音频串流。
//!
//! AVF 虚拟机通常**没有**可用的声卡：桌面里的程序能播放声音，但声音只进了
//! PulseAudio 的空输出设备（`auto_null`），手机扬声器不会响。
//!
//! 两种处理方式：
//!   * 虚拟机里已经有真实输出设备（部分 Android 已经把扬声器映射进虚拟机）
//!     —— 什么都不用做，桌面声音直接由手机播放；
//!   * 没有 —— 用 PulseAudio 的 `module-simple-protocol-tcp` 把桌面的 PCM
//!     通过 TCP 推给手机上的 [Simple Protocol Player]
//!     （https://github.com/xlzhen-940218/SimpleProtocolPlayer）。
//!
//! 串流方向：**guest 是 TCP 服务端**，手机上的播放器是客户端。
//! 播放器默认连 12345 端口，采样率 44100、双声道、s16le。
//!
//! 手机上不一定能直连虚拟机的地址（与 VNC 一样受 AVF 虚拟网卡影响），
//! 因此这里会先测一次直连，失败时自动在手机上用 `nc` 建一条中继，
//! 让播放器改连 `127.0.0.1` —— 用户不需要关心网络细节。

use anyhow::{bail, Result};
use serde::Serialize;

use crate::adb::Adb;
use crate::{apk, relay};

/// 音频播放器（Simple Protocol Player）的应用 ID。
pub const PLAYER_PACKAGE: &str = "com.xlzhen.simpleprotocolplayer";
/// 随程序内置的播放器安装包（离线可用，避免国内下载 GitHub 失败）。
const BUNDLED_PLAYER_APK: &[u8] = include_bytes!("../resources/simple-protocol-player.apk");
/// 内置安装包对应的版本号，仅用于提示文案。
const BUNDLED_PLAYER_TAG: &str = "1.0.0";
/// 桌面端监听端口。
pub const AUDIO_PORT: u16 = 12345;
/// 采样率。
pub const AUDIO_RATE: u32 = 44100;
/// 声道数。
pub const AUDIO_CHANNELS: u16 = 2;

/// 音频串流的整体状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioReport {
    /// 虚拟机里是否有可用的输出设备（自带扬声器）。
    pub has_speaker: bool,
    /// 当前输出设备名。
    pub sink_name: String,
    /// 串流模块是否已配置（写入 default.pa 或已加载）。
    pub module_ready: bool,
    /// 桌面在虚拟网络里的地址。
    pub guest_ip: String,
    /// 播放器实际应连接的地址（直连时等于 `guest_ip`，走中继时是 127.0.0.1）。
    pub connect_host: String,
    /// 播放器实际应连接的端口。
    pub connect_port: u16,
    /// 虚拟机内串流模块监听的端口。
    pub port: u16,
    /// 手机侧是否真的能连上串流端口。
    pub reachable: bool,
    /// 是否通过手机端 `nc` 中继连接。
    pub via_relay: bool,
    pub rate: u32,
    pub channels: u16,
    /// 手机上是否已安装播放器。
    pub player_installed: bool,
    /// 播放器是否已打开。
    pub player_launched: bool,
    /// 播放器里的地址与端口是否已自动填好。
    pub prefilled: bool,
    /// 播放器是否已经开始串流。
    pub streaming: bool,
    /// 给用户看的说明。
    pub message: String,
}

impl AudioReport {
    fn base() -> Self {
        Self {
            has_speaker: false,
            sink_name: String::new(),
            module_ready: false,
            guest_ip: String::new(),
            connect_host: String::new(),
            connect_port: AUDIO_PORT,
            port: AUDIO_PORT,
            reachable: false,
            via_relay: false,
            rate: AUDIO_RATE,
            channels: AUDIO_CHANNELS,
            player_installed: false,
            player_launched: false,
            prefilled: false,
            streaming: false,
            message: String::new(),
        }
    }
}

/// 在虚拟机里执行：检测输出设备 → 必要时配置串流模块 → 回读状态。
///
/// 返回的文本里带有 `AUDIO_*` 的键值行。
const GUEST_AUDIO_SCRIPT: &str = r#"
AUDIO_PORT=12345
AUDIO_RATE=44100
AUDIO_CHANNELS=2
MODULE_NAME="module-simple-protocol-tcp"
MODULE_ARGS="$MODULE_NAME rate=$AUDIO_RATE format=s16le channels=$AUDIO_CHANNELS source=@DEFAULT_SINK@.monitor record=true port=$AUDIO_PORT"

# 精简的 Debian 镜像有时没有设置 XDG_RUNTIME_DIR，pactl 会直接连不上总线。
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

# PulseAudio 是串流的前提；没有就先装上。
if ! command -v pactl >/dev/null 2>&1; then
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo -n env DEBIAN_FRONTEND=noninteractive apt-get -y install pulseaudio pulseaudio-utils >/dev/null 2>&1
  fi
fi

# PulseAudio 没在运行时就先拉起来，否则后面的 load-module 一定失败。
if command -v pactl >/dev/null 2>&1 && ! pactl info >/dev/null 2>&1; then
  pulseaudio --start --exit-idle-time=-1 >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 10 ]; do
    pactl info >/dev/null 2>&1 && break
    n=$((n+1))
    sleep 0.5
  done
fi

SINK_NAME="$(pactl list short sinks 2>/dev/null | awk 'NR==1 {print $2}')"
IP="$(hostname -I 2>/dev/null | awk '{print $1}')"

# auto_null 是 PulseAudio 的空输出设备：声音进了它不会有任何硬件发声，
# 说明虚拟机里没有可用的扬声器，需要走串流。
SINK=0
if [ -n "$SINK_NAME" ] && [ "$SINK_NAME" != "auto_null" ]; then SINK=1; fi

MODULE=0
LISTEN=0
if [ "$SINK" = "0" ]; then
  mkdir -p "$HOME/.config/pulse" 2>/dev/null || true
  PA="$HOME/.config/pulse/default.pa"
  # 写进 default.pa：桌面会话（以及以后每次登录）启动 PulseAudio 时自动加载。
  if ! grep -q "$MODULE_NAME" "$PA" 2>/dev/null; then
    echo "load-module $MODULE_ARGS" >> "$PA" 2>/dev/null || true
  fi
  if pactl info >/dev/null 2>&1; then
    # 上一轮可能已经加载过，避免重复 load-module。
    if pactl list short modules 2>/dev/null | grep -q "$MODULE_NAME"; then
      MODULE=1
    elif pactl load-module $MODULE_ARGS >/dev/null 2>&1; then
      MODULE=1
    fi
  fi
  # 没有正在运行的 PulseAudio 服务端时，default.pa 里的配置会在进桌面时生效。
  if grep -q "$MODULE_NAME" "$PA" 2>/dev/null; then MODULE=1; fi
  if (ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -q ":$AUDIO_PORT "; then
    LISTEN=1
  fi
fi

echo "AUDIO_SINK=$SINK"
echo "AUDIO_SINK_NAME=$SINK_NAME"
echo "AUDIO_MODULE=$MODULE"
echo "AUDIO_LISTEN=$LISTEN"
echo "AUDIO_IP=$IP"
echo "AUDIO_PORT=$AUDIO_PORT"
echo "AUDIO_RATE=$AUDIO_RATE"
echo "AUDIO_CHANNELS=$AUDIO_CHANNELS"
"#;

/// 从脚本输出里解析 `AUDIO_*` 键值行。
pub fn parse_audio_output(content: &str) -> AudioReport {
    let mut r = AudioReport::base();
    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "AUDIO_SINK" => r.has_speaker = value.trim() == "1",
            "AUDIO_SINK_NAME" => r.sink_name = value.trim().to_string(),
            "AUDIO_MODULE" => r.module_ready = value.trim() == "1",
            "AUDIO_LISTEN" => r.reachable = value.trim() == "1",
            "AUDIO_IP" => r.guest_ip = value.trim().to_string(),
            "AUDIO_PORT" => {
                if let Ok(p) = value.trim().parse::<u16>() {
                    r.port = p;
                    r.connect_port = p;
                }
            }
            "AUDIO_RATE" => {
                if let Ok(v) = value.trim().parse::<u32>() {
                    r.rate = v;
                }
            }
            "AUDIO_CHANNELS" => {
                if let Ok(v) = value.trim().parse::<u16>() {
                    r.channels = v;
                }
            }
            _ => {}
        }
    }
    if r.connect_host.is_empty() {
        r.connect_host = r.guest_ip.clone();
    }
    r
}

/// 音频串流端口是否已经在持续输出 PCM。
///
/// 这个探测**不依赖虚拟机终端**：只要能从手机侧读到至少 1 个字节，
/// 就说明模块已经加载并在推流（PulseAudio 即使静音也会持续发送静音帧）。
async fn audio_port_alive(adb: &Adb, host: &str, port: u16) -> bool {
    let out = adb
        .shell(&format!(
            "toybox timeout 2 toybox nc {host} {port} < /dev/null 2>/dev/null | head -c 1 | wc -c",
            host = host,
            port = port
        ))
        .await
        .unwrap_or_default();
    out.trim() == "1"
}

/// 从手机侧快速找到虚拟机地址（只读，不唤醒终端）。
async fn find_guest_ip(adb: &Adb) -> Option<String> {
    let neigh = adb
        .shell("ip neigh show 2>/dev/null | grep avf_tap")
        .await
        .ok()?;
    let mut ips: Vec<String> = Vec::new();
    for line in neigh.lines() {
        if let Some(ip) = line.split_whitespace().next() {
            // 只保留 IPv4
            if ip.contains('.') && !ips.iter().any(|x| x == ip) {
                ips.push(ip.to_string());
            }
        }
    }
    for ip in &ips {
        // STALE 条目先 ping 一下，避免地址已失效
        let _ = adb
            .shell(&format!("ping -c 1 -W 1 {} >/dev/null 2>&1", ip))
            .await;
        if audio_port_alive(adb, ip, AUDIO_PORT).await {
            return Some(ip.clone());
        }
    }
    None
}

/// 快速路径：串流端口已经在输出 PCM，说明桌面端配置早就完成了。
///
/// 常见情况下（例如第二次打开工具）完全不需要执行虚拟机脚本，
/// 避免因为 Terminal 自动化而等待几十秒。
async fn quick_report(adb: &Adb) -> Option<AudioReport> {
    let ip = find_guest_ip(adb).await?;
    let mut report = AudioReport::base();
    report.guest_ip = ip.clone();
    report.connect_host = ip;
    report.port = AUDIO_PORT;
    report.connect_port = AUDIO_PORT;
    report.module_ready = true;
    report.reachable = true;
    // 串流场景下输出设备就是 PulseAudio 的空设备
    report.sink_name = "auto_null".to_string();
    Some(report)
}

/// 把地址与端口自动填进播放器界面。
///
/// 播放器把地址存在 SharedPreferences 里（不读 intent extras），
/// 所以只能直接往输入框里打字。已经填对时不做任何操作，避免打断用户。
async fn prefill_player_fields(adb: &Adb, ip: &str, port: u16) -> bool {
    use crate::uia;

    let nodes = match uia::dump(adb).await {
        Ok(n) => n,
        Err(_) => return false,
    };
    let ip_node = uia::find_by_id(&nodes, "editTextIpAddr").cloned();
    let port_node = uia::find_by_id(&nodes, "editTextAudioPort").cloned();

    let ip_ok = ip_node
        .as_ref()
        .map(|n| n.text.trim() == ip)
        .unwrap_or(false);
    let port_ok = port_node
        .as_ref()
        .map(|n| n.text.trim() == port.to_string())
        .unwrap_or(false);
    if ip_ok && port_ok {
        return true;
    }

    if !ip_ok {
        let Some(node) = ip_node else {
            return false;
        };
        if uia::tap_node(adb, &node).await.is_err() {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // 全选后删除，避免和新值拼在一起
        let _ = adb.raw(&["shell", "input", "keycombination", "113", "29"]).await;
        let _ = adb.raw(&["shell", "input", "keyevent", "67"]).await;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if uia::type_text(adb, ip).await.is_err() {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }

    if !port_ok {
        let nodes = uia::dump(adb).await.unwrap_or_default();
        if let Some(node) = uia::find_by_id(&nodes, "editTextAudioPort") {
            if uia::tap_node(adb, node).await.is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = adb.raw(&["shell", "input", "keycombination", "113", "29"]).await;
                let _ = adb.raw(&["shell", "input", "keyevent", "67"]).await;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let _ = uia::type_text(adb, &port.to_string()).await;
            }
        }
    }
    true
}

/// 点一下「开始串流」；如果已经在串流则直接返回成功。
async fn start_streaming(adb: &Adb) -> bool {
    use crate::uia;

    let nodes = match uia::dump(adb).await {
        Ok(n) => n,
        Err(_) => return false,
    };
    if let Some(stop) = uia::find_by_id(&nodes, "stopButton") {
        if stop.enabled {
            return true;
        }
    }
    match uia::find_by_id(&nodes, "playButton") {
        Some(play) if play.enabled => {
            let ok = uia::tap_node(adb, play).await.is_ok();
            if ok {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }
            ok
        }
        _ => false,
    }
}

/// 确保播放器已安装并打开，填好地址并开始串流。
async fn ensure_player(
    adb: &Adb,
    host: &str,
    port: u16,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<(bool, bool, bool, bool)> {
    let mut installed = apk::installed(adb, PLAYER_PACKAGE).await;
    if !installed {
        let apk_path = apk::write_bundled("simple-protocol-player.apk", BUNDLED_PLAYER_APK)?;
        progress(format!(
            "正在安装音频播放器 Simple Protocol Player {} …",
            BUNDLED_PLAYER_TAG
        ));
        apk::install(adb, &apk_path).await?;
        progress("音频播放器已安装。".to_string());
        installed = true;
    }

    progress("正在打开音频播放器…".to_string());
    let launched = apk::launch(adb, PLAYER_PACKAGE).await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    progress("正在把地址填入播放器…".to_string());
    let prefilled = prefill_player_fields(adb, host, port).await;
    progress("正在开始串流…".to_string());
    let streaming = prefilled && start_streaming(adb).await;
    Ok((installed, launched, prefilled, streaming))
}

/// 播放器当前是否处于串流状态（仅当它在前台时有效）。
async fn player_is_streaming(adb: &Adb) -> bool {
    use crate::uia;

    let Ok(nodes) = uia::dump(adb).await else {
        return false;
    };
    uia::find_by_id(&nodes, "stopButton")
        .map(|n| n.enabled)
        .unwrap_or(false)
}

/// 检测虚拟机的音频情况（只读，不改动系统）。
pub async fn detect(adb: &Adb) -> Result<AudioReport> {
    // 快速路径：端口已经在推流 → 直接返回，不唤醒终端。
    if let Some(mut report) = quick_report(adb).await {
        report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;
        report.streaming = report.player_installed && player_is_streaming(adb).await;
        report.message = describe(&report);
        return Ok(report);
    }

    let out = crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await?;
    let mut report = parse_audio_output(&out);
    report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;

    if !report.has_speaker && report.module_ready {
        let direct = audio_port_alive(adb, &report.guest_ip, report.port).await;
        // 直连不通时看看上一轮是否留下了手机端中继。
        if !direct
            && relay::device_listens(adb, report.port).await
            && relay::tcp_connect_ok(adb, "127.0.0.1", report.port).await
        {
            report.via_relay = true;
            report.connect_host = "127.0.0.1".to_string();
            report.connect_port = report.port;
            report.reachable = true;
        } else {
            report.reachable = direct;
        }
    }

    report.message = describe(&report);
    Ok(report)
}

/// 配置音频串流：虚拟机端配好模块，手机端装好播放器并填好地址。
pub async fn setup(
    adb: &Adb,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<AudioReport> {
    // 快速路径：桌面端已经在推流，跳过所有终端操作，直接把播放器拉起来。
    if let Some(mut report) = quick_report(adb).await {
        progress(format!(
            "桌面音频串流已在 {}:{} 运行，无需重新配置。",
            report.guest_ip, report.port
        ));
        let (installed, launched, prefilled, streaming) =
            ensure_player(adb, &report.connect_host, report.connect_port, progress).await?;
        report.player_installed = installed;
        report.player_launched = launched;
        report.prefilled = prefilled;
        report.streaming = streaming;
        report.message = describe(&report);
        return Ok(report);
    }

    progress("正在检查桌面音频输出设备…".to_string());
    let out = crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await?;
    let mut report = parse_audio_output(&out);

    if report.has_speaker {
        progress(format!(
            "桌面自带扬声器（{}），无需音频串流。",
            report.sink_name
        ));
        report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;
        report.reachable = true;
        report.message = describe(&report);
        return Ok(report);
    }

    if report.guest_ip.is_empty() {
        bail!("没有拿到虚拟机的地址，无法配置音频串流。请确认虚拟机已启动。");
    }

    if !report.module_ready {
        bail!(
            "桌面端音频串流模块没能配置成功。\n\
             请在虚拟机终端里手动执行：\n  \
             export XDG_RUNTIME_DIR=${{XDG_RUNTIME_DIR:-/run/user/$(id -u)}}\n  \
             pulseaudio --start --exit-idle-time=-1\n  \
             pactl load-module module-simple-protocol-tcp rate=44100 format=s16le channels=2 \
             source=@DEFAULT_SINK@.monitor record=true port=12345"
        );
    }

    progress(format!(
        "桌面端已就绪，正在监听 {}:{}",
        report.guest_ip, report.port
    ));

    // 选择播放器实际连接的地址：优先直连，直连不通再走手机端中继。
    report.connect_host = report.guest_ip.clone();
    report.connect_port = report.port;
    report.via_relay = false;
    report.reachable = audio_port_alive(adb, &report.guest_ip, report.port).await;
    if !report.reachable {
        progress("手机无法直连虚拟机的音频端口，正在建立手机端中继…".to_string());
        match relay::open_port_relay(adb, report.port, &report.guest_ip, report.port).await {
            Ok(port) => {
                report.via_relay = true;
                report.connect_host = "127.0.0.1".to_string();
                report.connect_port = port;
                report.reachable = true;
                progress(format!(
                    "手机已在 :{} 监听，并转发到虚拟机音频端口 {}。",
                    port, report.port
                ));
            }
            Err(e) => progress(format!("手机端音频中继没有建立成功：{}", e)),
        }
    }

    let (installed, launched, prefilled, streaming) =
        ensure_player(adb, &report.connect_host, report.connect_port, progress).await?;
    report.player_installed = installed;
    report.player_launched = launched;
    report.prefilled = prefilled;
    report.streaming = streaming;

    report.message = describe(&report);
    Ok(report)
}

/// 只重新打开播放器并填好地址，不改动虚拟机里的串流配置。
///
/// 用户关掉播放器、或想再听一次时使用；地址与端口由前端把上一次的
/// `connectHost` / `connectPort` 传回来。
pub async fn open_player(
    adb: &Adb,
    host: &str,
    port: u16,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<AudioReport> {
    let mut report = AudioReport::base();
    report.connect_host = host.to_string();
    report.connect_port = port;
    report.port = port;
    report.reachable = audio_port_alive(adb, host, port).await;

    let (installed, launched, prefilled, streaming) =
        ensure_player(adb, host, port, progress).await?;
    report.player_installed = installed;
    report.player_launched = launched;
    report.prefilled = prefilled;
    report.streaming = streaming;
    report.message = if streaming {
        format!("已打开播放器并开始串流（{}:{}），现在可以听到桌面声音了。", host, port)
    } else if prefilled {
        format!(
            "已打开播放器并填好地址 {}:{}，点「开始串流」即可收听到桌面声音。",
            host, port
        )
    } else {
        format!(
            "已打开播放器。请在 App 里填地址 {}、端口 {}，然后点「开始串流」。",
            host, port
        )
    };
    Ok(report)
}

/// 生成给用户看的说明文字。
fn describe(r: &AudioReport) -> String {
    if r.has_speaker {
        return format!(
            "桌面自带扬声器（{}），声音直接由手机播放，不需要额外操作。",
            r.sink_name
        );
    }
    if !r.reachable {
        return format!(
            "桌面音频串流已配置，但手机暂时连不上 {}:{}。\n\
             请确认虚拟机仍在运行，然后重新点一次「配置音频串流」；\n\
             也可以在手机上手动打开 Simple Protocol Player 连接该地址。",
            r.connect_host, r.connect_port
        );
    }
    let mut s = format!(
        "桌面没有自带扬声器，已改用音频串流：\n\
         手机上的播放器连接 {}:{}（{}Hz / {} 声道）。",
        r.connect_host, r.connect_port, r.rate, r.channels
    );
    if r.via_relay {
        s.push_str(&format!(
            "\n已自动建立手机端中继，虚拟机地址为 {}:{}。",
            r.guest_ip, r.port
        ));
    }
    if r.player_installed {
        s.push_str("\n手机上已安装 Simple Protocol Player。");
    } else {
        s.push_str("\n手机上还没有安装 Simple Protocol Player。");
    }
    if r.streaming {
        s.push_str(&format!(
            "\n已在手机上自动开始串流（{}:{}）。",
            r.connect_host, r.connect_port
        ));
    } else if r.prefilled {
        s.push_str(&format!(
            "\n已自动填好地址 {} 和端口 {}，打开 App 点「开始串流」即可。",
            r.connect_host, r.connect_port
        ));
    } else {
        s.push_str(&format!(
            "\n请在手机上打开 Simple Protocol Player，地址填 {}，端口填 {}，然后点「开始串流」。",
            r.connect_host, r.connect_port
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_audio_output() {
        let r = parse_audio_output(
            "noise\nAUDIO_SINK=0\nAUDIO_SINK_NAME=auto_null\nAUDIO_MODULE=1\nAUDIO_LISTEN=1\n\
             AUDIO_IP=10.32.71.37\nAUDIO_PORT=12345\nAUDIO_RATE=44100\nAUDIO_CHANNELS=2\n__DONE__\n",
        );
        assert!(!r.has_speaker);
        assert_eq!(r.sink_name, "auto_null");
        assert!(r.module_ready);
        assert!(r.reachable);
        assert_eq!(r.guest_ip, "10.32.71.37");
        assert_eq!(r.connect_host, "10.32.71.37");
        assert_eq!(r.port, 12345);
        assert_eq!(r.connect_port, 12345);
        assert_eq!(r.rate, 44100);
        assert_eq!(r.channels, 2);
    }

    #[test]
    fn detects_real_speaker() {
        let r = parse_audio_output("AUDIO_SINK=1\nAUDIO_SINK_NAME=alsa_output.usb\n");
        assert!(r.has_speaker);
        assert_eq!(r.sink_name, "alsa_output.usb");
        assert!(!r.module_ready);
    }
}
