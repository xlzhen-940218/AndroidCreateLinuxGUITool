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

use anyhow::{bail, Result};
use serde::Serialize;

use crate::adb::Adb;
use crate::apk;

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
    /// 桌面在虚拟网络里的地址（手机上的播放器连这个地址）。
    pub guest_ip: String,
    pub port: u16,
    pub rate: u32,
    pub channels: u16,
    /// 手机上是否已安装播放器。
    pub player_installed: bool,
    /// 播放器是否已打开。
    pub player_launched: bool,
    /// 播放器里的地址与端口是否已自动填好。
    pub prefilled: bool,
    /// 给用户看的说明。
    pub message: String,
}

/// 在虚拟机里执行：检测输出设备 → 必要时配置串流模块 → 回读状态。
///
/// 返回的文本里带有 `AUDIO_*` 的键值行。
const GUEST_AUDIO_SCRIPT: &str = r#"
AUDIO_PORT=12345
AUDIO_RATE=44100
AUDIO_CHANNELS=2
MODULE_ARGS="module-simple-protocol-tcp rate=$AUDIO_RATE format=s16le channels=$AUDIO_CHANNELS source=@DEFAULT_SINK@.monitor record=true port=$AUDIO_PORT"

# PulseAudio 是串流的前提；没有就先装上。
if ! command -v pactl >/dev/null 2>&1; then
  if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    sudo -n env DEBIAN_FRONTEND=noninteractive apt-get -y install pulseaudio pulseaudio-utils >/dev/null 2>&1
  fi
fi

SINK_NAME="$(pactl list short sinks 2>/dev/null | awk 'NR==1 {print $2}')"
IP="$(hostname -I 2>/dev/null | awk '{print $1}')"

# auto_null 是 PulseAudio 的空输出设备：声音进了它不会有任何硬件发声，
# 说明虚拟机里没有可用的扬声器，需要走串流。
SINK=0
if [ -n "$SINK_NAME" ] && [ "$SINK_NAME" != "auto_null" ]; then SINK=1; fi

MODULE=0
if [ "$SINK" = "0" ]; then
  mkdir -p "$HOME/.config/pulse" 2>/dev/null || true
  PA="$HOME/.config/pulse/default.pa"
  # 写进 default.pa：桌面会话（以及以后每次登录）启动 PulseAudio 时自动加载。
  if ! grep -q "module-simple-protocol-tcp" "$PA" 2>/dev/null; then
    echo "load-module $MODULE_ARGS" >> "$PA" 2>/dev/null || true
  fi
  if pactl info >/dev/null 2>&1; then
    pactl unload-module module-simple-protocol-tcp 2>/dev/null || true
    if pactl load-module $MODULE_ARGS >/dev/null 2>&1; then MODULE=1; fi
  fi
  # 没有正在运行的 PulseAudio 服务端时，default.pa 里的配置会在进桌面时生效。
  if grep -q "module-simple-protocol-tcp" "$PA" 2>/dev/null; then MODULE=1; fi
fi

echo "AUDIO_SINK=$SINK"
echo "AUDIO_SINK_NAME=$SINK_NAME"
echo "AUDIO_MODULE=$MODULE"
echo "AUDIO_IP=$IP"
echo "AUDIO_PORT=$AUDIO_PORT"
echo "AUDIO_RATE=$AUDIO_RATE"
echo "AUDIO_CHANNELS=$AUDIO_CHANNELS"
"#;

/// 从脚本输出里解析 `AUDIO_*` 键值行。
pub fn parse_audio_output(content: &str) -> AudioReport {
    let mut r = AudioReport {
        has_speaker: false,
        sink_name: String::new(),
        module_ready: false,
        guest_ip: String::new(),
        port: AUDIO_PORT,
        rate: AUDIO_RATE,
        channels: AUDIO_CHANNELS,
        player_installed: false,
        player_launched: false,
        prefilled: false,
        message: String::new(),
    };
    for line in content.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "AUDIO_SINK" => r.has_speaker = value.trim() == "1",
            "AUDIO_SINK_NAME" => r.sink_name = value.trim().to_string(),
            "AUDIO_MODULE" => r.module_ready = value.trim() == "1",
            "AUDIO_IP" => r.guest_ip = value.trim().to_string(),
            "AUDIO_PORT" => {
                if let Ok(p) = value.trim().parse::<u16>() {
                    r.port = p;
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
    r
}

/// 把地址与端口自动填进播放器界面。
///
/// 播放器把地址存在 SharedPreferences 里（不读 intent extras），
/// 所以只能直接往输入框里打字。填好后用户只要点一下「开始串流」。
async fn prefill_player_fields(adb: &Adb, ip: &str, port: u16) -> bool {
    use crate::uia;

    let nodes = match uia::dump(adb).await {
        Ok(n) => n,
        Err(_) => return false,
    };
    let ip_node = match uia::find_by_id(&nodes, "editTextIpAddr") {
        Some(n) => n.clone(),
        None => return false,
    };
    if uia::tap_node(adb, &ip_node).await.is_err() {
        return false;
    }
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    // 先清空再输入
    let _ = adb.raw(&["shell", "input", "keycombination", "113", "29"]).await;
    let _ = adb.raw(&["shell", "input", "keyevent", "67"]).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    if uia::type_text(adb, ip).await.is_err() {
        return false;
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 端口输入框（有默认值，一般也是对的；不一致才改）
    let nodes = uia::dump(adb).await.unwrap_or_default();
    if let Some(port_node) = uia::find_by_id(&nodes, "editTextAudioPort") {
        if uia::tap_node(adb, port_node).await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = adb.raw(&["shell", "input", "keycombination", "113", "29"]).await;
            let _ = adb.raw(&["shell", "input", "keyevent", "67"]).await;
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let _ = uia::type_text(adb, &port.to_string()).await;
        }
    }
    true
}

/// 检测虚拟机的音频情况（只读，不改动系统）。
pub async fn detect(adb: &Adb) -> Result<AudioReport> {
    let out = crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await?;
    let mut report = parse_audio_output(&out);
    report.player_installed = apk::installed(adb, PLAYER_PACKAGE).await;
    report.message = describe(&report);
    Ok(report)
}

/// 配置音频串流：虚拟机端配好模块，手机端装好播放器并填好地址。
pub async fn setup(
    adb: &Adb,
    progress: &mut (dyn FnMut(String) + Send),
) -> Result<AudioReport> {
    progress("正在检查桌面音频输出设备…".to_string());
    let out = crate::installer::guest_exec(adb, GUEST_AUDIO_SCRIPT).await?;
    let mut report = parse_audio_output(&out);

    if report.has_speaker {
        progress(format!(
            "桌面自带扬声器（{}），无需音频串流。",
            report.sink_name
        ));
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
             pactl load-module module-simple-protocol-tcp rate=44100 format=s16le channels=2 \
             source=@DEFAULT_SINK@.monitor record=true port=12345"
        );
    }

    progress(format!(
        "桌面端已就绪，正在监听 {}:{}",
        report.guest_ip, report.port
    ));

    if !apk::installed(adb, PLAYER_PACKAGE).await {
        let apk_path = apk::write_bundled("simple-protocol-player.apk", BUNDLED_PLAYER_APK)?;
        progress(format!(
            "正在安装音频播放器 Simple Protocol Player {} …",
            BUNDLED_PLAYER_TAG
        ));
        apk::install(adb, &apk_path).await?;
        progress("音频播放器已安装。".to_string());
    }
    report.player_installed = true;

    progress("正在打开音频播放器…".to_string());
    report.player_launched = apk::launch(adb, PLAYER_PACKAGE).await;
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    progress("正在把地址填入播放器…".to_string());
    report.prefilled = prefill_player_fields(adb, &report.guest_ip, report.port).await;

    report.message = describe(&report);
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
    let mut s = format!(
        "桌面没有自带扬声器，已改用音频串流：\n\
         虚拟机在 {}:{} 上提供 {}Hz / {} 声道音频。\n",
        r.guest_ip, r.port, r.rate, r.channels
    );
    if r.player_installed {
        s.push_str("手机上已安装 Simple Protocol Player。");
    } else {
        s.push_str("手机上还没有安装 Simple Protocol Player。");
    }
    if r.prefilled {
        s.push_str(&format!(
            "\n已自动填好地址 {} 和端口 {}，打开 App 点「开始串流」即可。",
            r.guest_ip, r.port
        ));
    } else {
        s.push_str(&format!(
            "\n请在手机上打开 Simple Protocol Player，地址填 {}，端口填 {}，然后点「开始串流」。",
            r.guest_ip, r.port
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
            "noise\nAUDIO_SINK=0\nAUDIO_SINK_NAME=auto_null\nAUDIO_MODULE=1\n\
             AUDIO_IP=10.32.71.37\nAUDIO_PORT=12345\nAUDIO_RATE=44100\nAUDIO_CHANNELS=2\n__DONE__\n",
        );
        assert!(!r.has_speaker);
        assert_eq!(r.sink_name, "auto_null");
        assert!(r.module_ready);
        assert_eq!(r.guest_ip, "10.32.71.37");
        assert_eq!(r.port, 12345);
        assert_eq!(r.rate, 44100);
        assert_eq!(r.channels, 2);
    }

    #[test]
    fn detects_real_speaker() {
        let r = parse_audio_output("AUDIO_SINK=1\nAUDIO_SINK_NAME=alsa_output.usb\n");
        assert!(r.has_speaker);
        assert_eq!(r.sink_name, "alsa_output.usb");
    }
}
