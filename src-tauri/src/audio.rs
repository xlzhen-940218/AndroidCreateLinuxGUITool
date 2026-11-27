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

/// 在虚拟机里执行：检测声卡 → 必要时配置虚拟输出/串流 → 回读状态。
///
/// 返回的文本里带有 `AUDIO_*` 的键值行。
///
/// 关键点：AVF 虚拟机里其实是**有**声卡（`virtio-snd`，`/proc/asound/cards`
/// 里的 `VirtIO SoundCard`），但 `/dev/snd/*` 默认是 `root:audio 0660`，
/// 桌面用户不在 `audio` 组时打不开设备，WirePlumber 就不会创建 sink，
/// 于是在 VNC 桌面里“一个音频设备都检测不到”。所以这里先修权限，再找声卡。
const GUEST_AUDIO_SCRIPT: &str = r#"
AUDIO_PORT=12345
AUDIO_RATE=44100
AUDIO_CHANNELS=2
MODULE_NAME="module-simple-protocol-tcp"
VIRTUAL_SINK="avf_speaker"
MODULE_ARGS="$MODULE_NAME rate=$AUDIO_RATE format=s16le channels=$AUDIO_CHANNELS source=$VIRTUAL_SINK.monitor record=true port=$AUDIO_PORT"

# 精简的 Debian 镜像有时没有设置 XDG_RUNTIME_DIR，pactl 会直接连不上总线。
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"

# 真实声卡：既不是 auto_null，也不是本工具创建的虚拟输出。
real_sink() {
  pactl list short sinks 2>/dev/null \
    | awk '{print $2}' \
    | grep -v -E '^(auto_null|avf_speaker)$' \
    | head -n1
}

# 让桌面用户能读写 /dev/snd：加 udev 规则（重启后依然有效）+ 立刻改一次权限。
ensure_snd_access() {
  [ -d /dev/snd ] || return 1
  RULE=/etc/udev/rules.d/60-avf-audio.rules
  if [ ! -f "$RULE" ] && command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then
    printf 'SUBSYSTEM=="sound", MODE="0666"\n' | sudo -n tee "$RULE" >/dev/null 2>&1
    sudo -n udevadm control --reload-rules >/dev/null 2>&1
    sudo -n udevadm trigger --subsystem-match=sound --action=change >/dev/null 2>&1
  fi
  [ -r /dev/snd/controlC0 ] && [ -w /dev/snd/controlC0 ] && return 0
  command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1 || return 1
  sudo -n chmod 666 /dev/snd/* >/dev/null 2>&1
  [ -r /dev/snd/controlC0 ]
}

# 音频服务：新版镜像用 PipeWire（pactl 连的是 pipewire-pulse），旧镜像用 PulseAudio。
start_audio_server() {
  pactl info >/dev/null 2>&1 && return 0
  systemctl --user start pipewire wireplumber pipewire-pulse >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 8 ]; do
    pactl info >/dev/null 2>&1 && return 0
    n=$((n+1))
    sleep 0.5
  done
  pulseaudio --start --exit-idle-time=-1 >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 10 ]; do
    pactl info >/dev/null 2>&1 && return 0
    n=$((n+1))
    sleep 0.5
  done
  return 1
}

# pactl 是检测和配置的基础；没有就先装上。
if ! command -v pactl >/dev/null 2>&1; then
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo -n env DEBIAN_FRONTEND=noninteractive apt-get -y install pulseaudio pulseaudio-utils >/dev/null 2>&1
  fi
fi

ensure_snd_access || true
start_audio_server || true

IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
REAL="$(real_sink)"

# 权限刚放开时 WirePlumber 已经错过了设备，让它重新扫描一次。
if [ -z "$REAL" ] && grep -qv 'no soundcards' /proc/asound/cards 2>/dev/null; then
  systemctl --user restart wireplumber >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 12 ]; do
    REAL="$(real_sink)"
    [ -n "$REAL" ] && break
    n=$((n+1))
    sleep 0.5
  done
fi

SINK=0
MODULE=0
LISTEN=0
SINK_NAME="$REAL"

if [ -n "$REAL" ]; then
  # 有真实声卡：直接用，声音由 Terminal 应用（virtio-snd 后端）从手机扬声器放出来。
  SINK=1
  # 用更友好的设备描述（如 "Virtio 1.0 sound Stereo"）展示给用户
  DESC="$(pactl list sinks 2>/dev/null | grep -F -A1 "Name: $REAL" \
    | sed -n 's/^[^D]*Description: //p' | head -n1)"
  [ -n "$DESC" ] && SINK_NAME="$DESC"
  pactl set-default-sink "$REAL" >/dev/null 2>&1 || true
  pactl set-sink-mute "$REAL" 0 >/dev/null 2>&1 || true
  pactl set-sink-volume "$REAL" 100% >/dev/null 2>&1 || true

  # 之前为串流准备的虚拟输出和模块已经没用了，卸掉，免得桌面上多一个假设备。
  for id in $(pactl list short modules 2>/dev/null | grep "$MODULE_NAME" | awk '{print $1}'); do
    pactl unload-module "$id" >/dev/null 2>&1 || true
  done
  for id in $(pactl list short modules 2>/dev/null | grep "sink_name=$VIRTUAL_SINK" | awk '{print $1}'); do
    pactl unload-module "$id" >/dev/null 2>&1 || true
  done
  PA="$HOME/.config/pulse/default.pa"
  if [ -f "$PA" ]; then
    grep -v -E "module-simple-protocol-tcp|sink_name=$VIRTUAL_SINK|set-default-sink $VIRTUAL_SINK" \
      "$PA" > "$PA.acl.tmp" 2>/dev/null && mv "$PA.acl.tmp" "$PA" 2>/dev/null
  fi
else
  if pactl info >/dev/null 2>&1; then
    # 桌面里必须有一个看得见的输出设备，否则音频设置里是空的。
    # 没有真实声卡时创建 "AVF Audio" 虚拟输出，并把默认输出切过去。
    if ! pactl list short sinks 2>/dev/null | awk '{print $2}' | grep -qx "$VIRTUAL_SINK"; then
      pactl load-module module-null-sink sink_name=$VIRTUAL_SINK \
        sink_properties=device.description=AVF_Audio >/dev/null 2>&1 || true
    fi
    pactl set-default-sink "$VIRTUAL_SINK" >/dev/null 2>&1 || true
    pactl set-sink-volume "$VIRTUAL_SINK" 100% >/dev/null 2>&1 || true
    pactl set-sink-mute "$VIRTUAL_SINK" 0 >/dev/null 2>&1 || true
    SINK_NAME="$VIRTUAL_SINK"

    # 已加载的模块如果还指向旧的 auto_null，就换成虚拟输出的 monitor。
    if pactl list short modules 2>/dev/null | grep "$MODULE_NAME" | grep -q "$VIRTUAL_SINK.monitor"; then
      MODULE=1
    else
      for id in $(pactl list short modules 2>/dev/null | grep "$MODULE_NAME" | awk '{print $1}'); do
        pactl unload-module "$id" >/dev/null 2>&1 || true
      done
      pactl load-module $MODULE_ARGS >/dev/null 2>&1 && MODULE=1
    fi
  fi

  if [ "$MODULE" = "1" ] && (ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -q ":$AUDIO_PORT "; then
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

/// 在手机上找到一个正在监听的音频中继端口。
async fn find_relay_port(adb: &Adb) -> Option<u16> {
    for offset in 0..10u16 {
        let port = AUDIO_PORT.saturating_add(offset);
        if relay::device_listens(adb, port).await {
            return Some(port);
        }
    }
    None
}

/// 播放器是否已经通过中继建立了连接。
///
/// 纯被动检查（读 `netstat`），不会像端口探测那样额外建连接；
/// 音频模块同一时间只服务一个客户端，主动探测会打扰正在播放的播放器。
async fn player_connected(adb: &Adb, port: u16) -> bool {
    let out = adb
        .shell(&format!(
            "toybox netstat -tn 2>/dev/null | grep ESTABLISHED | grep -q ':{port} '; echo $?",
            port = port
        ))
        .await
        .unwrap_or_default();
    out.trim() == "0"
}

/// 从手机侧读取虚拟机地址（只读，不探测端口）。
async fn find_guest_ip(adb: &Adb) -> Option<String> {
    let neigh = adb
        .shell("ip neigh show 2>/dev/null | grep avf_tap")
        .await
        .ok()?;
    neigh.lines().find_map(|line| {
        let ip = line.split_whitespace().next()?;
        if ip.contains('.') {
            Some(ip.to_string())
        } else {
            None
        }
    })
}

/// 快速路径：手机上已经有音频中继在监听，说明之前已经配置过。
///
/// 只做被动检查：看中继端口是否在监听、播放器是否已连上，
/// 绝不额外建立探测连接，避免把正在串流的播放器挤掉。
async fn quick_report(adb: &Adb) -> Option<AudioReport> {
    let port = find_relay_port(adb).await?;
    let mut report = AudioReport::base();
    report.guest_ip = find_guest_ip(adb).await.unwrap_or_default();
    report.connect_host = "127.0.0.1".to_string();
    report.connect_port = port;
    report.port = AUDIO_PORT;
    report.module_ready = true;
    report.reachable = true;
    report.via_relay = true;
    report.streaming = player_connected(adb, port).await;
    // 串流场景下输出设备是本工具创建的虚拟声卡
    report.sink_name = "avf_speaker".to_string();
    Some(report)
}

/// 把地址与端口自动填进播放器界面，返回是否**真的**填对。
///
/// 播放器把地址存在 SharedPreferences 里（不读 intent extras），只能往输入框打字。
/// 输入框是 AutoCompleteTextView，Ctrl+A 在部分输入法下不生效，
/// 所以这里用「移到行尾 + 连续退格」清空，写完再 dump 一次校验真实值。
async fn prefill_player_fields(adb: &Adb, ip: &str, port: u16) -> bool {
    use crate::uia;

    async fn set_field(adb: &Adb, id: &str, value: &str) -> bool {
        let Ok(nodes) = uia::dump(adb).await else {
            return false;
        };
        let Some(node) = uia::find_by_id(&nodes, id) else {
            return false;
        };
        if node.text.trim() == value {
            return true;
        }
        if uia::tap_node(adb, node).await.is_err() {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        // MOVE_END 后连按退格清空：IP 最长 15 位、端口 5 位，24 次足够。
        let _ = adb
            .shell("input keyevent 123; for i in $(seq 1 24); do input keyevent 67; done")
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if uia::type_text(adb, value).await.is_err() {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let Ok(nodes) = uia::dump(adb).await else {
            return false;
        };
        uia::find_by_id(&nodes, id)
            .map(|n| n.text.trim() == value)
            .unwrap_or(false)
    }

    let ip_ok = set_field(adb, "editTextIpAddr", ip).await;
    // 收起软键盘，避免它挡住下面的端口输入框
    if ip_ok {
        let _ = adb.shell("input keyevent 4").await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    let port_ok = set_field(adb, "editTextAudioPort", &port.to_string()).await;
    let _ = adb.shell("input keyevent 4").await;
    ip_ok && port_ok
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

/// 检测虚拟机的音频情况（只读，不改动系统）。
pub async fn detect(adb: &Adb) -> Result<AudioReport> {
    // 快速路径：中继在监听且播放器真的连上了 → 确实在串流，不必唤醒终端。
    // 只监听、没有播放器连接的中继可能是旧版本留下的，继续往下问虚拟机。
    if let Some(mut report) = quick_report(adb).await {
        if report.streaming {
            report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;
            report.message = describe(&report);
            return Ok(report);
        }
    }

    let out = crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await?;
    let mut report = parse_audio_output(&out);
    report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;

    if report.has_speaker {
        // 真实声卡可用；把旧版本留下的串流中继清掉。
        stop_audio_relays(adb).await;
        report.reachable = true;
        report.message = describe(&report);
        return Ok(report);
    }

    if report.module_ready {
        // 优先复用手机端中继；没有中继时按直连处理（模块已在监听）。
        if let Some(port) = find_relay_port(adb).await {
            report.via_relay = true;
            report.connect_host = "127.0.0.1".to_string();
            report.connect_port = port;
            report.streaming = player_connected(adb, port).await;
        } else {
            report.connect_host = report.guest_ip.clone();
            report.connect_port = report.port;
        }
        report.reachable = true;
    }

    report.message = describe(&report);
    Ok(report)
}

/// 关掉本工具之前为音频串流拉起的手机端中继。
///
/// 真实声卡可用后这些中继就没用了，留着会让「检测」误以为还在串流。
async fn stop_audio_relays(adb: &Adb) {
    for offset in 0..10u16 {
        let port = AUDIO_PORT.saturating_add(offset);
        if relay::device_listens(adb, port).await {
            relay::stop_relay(adb, port).await;
        }
    }
}

/// 配置桌面音频：优先启用虚拟机自带的声卡，没有才退回串流。
pub async fn setup(
    adb: &Adb,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<AudioReport> {
    progress("正在检查桌面音频输出设备…".to_string());
    // 以虚拟机里的真实状态为准：之前的中继可能已经过时（例如声卡权限修好了）。
    let out = match crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await {
        Ok(o) => o,
        Err(e) => {
            // 终端不可用时，沿用上一次已经建好的中继，至少能把播放器拉起来。
            if let Some(mut report) = quick_report(adb).await {
                progress(format!("无法读取虚拟机状态（{}），沿用已有中继。", e));
                let (installed, launched, prefilled, streaming) =
                    ensure_player(adb, &report.connect_host, report.connect_port, progress).await?;
                report.player_installed = installed;
                report.player_launched = launched;
                report.prefilled = prefilled;
                report.streaming = streaming;
                report.message = describe(&report);
                return Ok(report);
            }
            return Err(e);
        }
    };
    let mut report = parse_audio_output(&out);

    if report.has_speaker {
        progress(format!(
            "桌面已检测到声卡（{}），声音由手机直接播放。",
            report.sink_name
        ));
        report.reachable = true;
        // 之前为串流准备的虚拟输出已经卸载，顺手把手机上残留的中继也关掉。
        stop_audio_relays(adb).await;
        report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;
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
             pactl load-module module-null-sink sink_name=avf_speaker \
             sink_properties=device.description=AVF_Audio\n  \
             pactl set-default-sink avf_speaker\n  \
             pactl load-module module-simple-protocol-tcp rate=44100 format=s16le channels=2 \
             source=avf_speaker.monitor record=true port=12345"
        );
    }

    progress(format!(
        "桌面端已就绪，正在监听 {}:{}",
        report.guest_ip, report.port
    ));

    // 音频模块同一时间只服务一个客户端，统一走手机端中继：
    // 播放器连 127.0.0.1，中继再连虚拟机，避免直连探测和播放器抢连接。
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
        Err(e) => {
            // 中继失败时退回直连；模块本身已经在监听。
            report.via_relay = false;
            report.connect_host = report.guest_ip.clone();
            report.connect_port = report.port;
            report.reachable = true;
            progress(format!(
                "手机端中继没有建立成功，将直接连接虚拟机：{}",
                e
            ));
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
    // 地址来自上一次成功的配置，直接信任，避免再建探测连接。
    report.reachable = true;

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
            "桌面已检测到声卡（{}），声音由手机扬声器直接播放，不需要串流。\n\
             如果没声音，看一下 VNC 桌面右下角的音量图标，确认输出设备选的是它且没有静音。",
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
    if r.via_relay && !r.guest_ip.is_empty() {
        s.push_str(&format!(
            "\n已通过手机端中继连接，虚拟机地址为 {}:{}。",
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
