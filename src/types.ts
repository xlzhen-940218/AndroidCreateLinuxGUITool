export interface EnvInfo {
  adbPath: string;
  adbFound: boolean;
  adbVersion: string;
  error: string | null;
}

export interface DeviceEntry {
  serial: string;
  state: string;
  model: string;
  product: string;
  device: string;
  transportId: string;
  fingerprint: string;
}

export interface AvfReport {
  serial: string;
  model: string;
  manufacturer: string;
  androidRelease: string;
  sdkInt: number;
  isPixel: boolean;
  featureLine: string;
  featureSupported: boolean;
  vmInfoOk: boolean;
  vmInfo: string;
  virtApexPresent: boolean;
  terminalPkgPresent: boolean;
  terminalPkgEnabled: boolean;
  supported: boolean;
  reasons: string[];
}

export interface SecurityIssue {
  kind: string;
  title: string;
  detail: string;
  autoClickable: boolean;
}

export interface DevOptionReport {
  devOptionsEnabled: boolean;
  adbEnabled: boolean;
  linuxEnvEnabled: boolean;
  terminalPkgPresent: boolean;
  linuxEntryVisible: boolean;
  linuxEntryLabel: string;
  linuxSwitchChecked: boolean | null;
  isPixel: boolean;
  devicePolicyPresent: boolean;
  devicePolicyDetail: string;
  securityIssue: SecurityIssue | null;
  notes: string[];
}

export type TerminalPhase =
  | "notInstalled"
  | "needsInstall"
  | "installing"
  | "booting"
  | "ready"
  | "unknown";

/** 「安装 Linux 系统」步骤的进度。 */
export interface ImageStatus {
  phase: TerminalPhase;
  ready: boolean;
  message: string;
}

export interface InstallOptions {
  desktop: string;
  cnMirror: boolean;
  locale: string;
  vncPassword: string;
  vncGeometry: string;
  /** 虚拟机磁盘目标大小（GB）；0 表示不调整。仅 Android 16 及以下生效。 */
  diskSizeGb: number;
  installApps: boolean;
  installIme: boolean;
  sshPort: number;
}

export type InstallState =
  | "idle"
  | "preparing"
  | "running"
  | "success"
  | "failed"
  | "cancelled";

export interface InstallStatus {
  state: InstallState;
  message: string;
  step: string;
  /** 本次执行的安装阶段：base / desktop / apps / vncsvc / all */
  stage: string;
  guestDir: string;
  phoneDir: string;
  scriptPath: string;
}

/** 虚拟机内已安装环境的检测结果。 */
export interface InstalledReport {
  checked: boolean;
  desktop: boolean;
  desktopName: string;
  desktops: string[];
  apps: boolean;
  appsInstalled: number;
  appsTotal: number;
  ime: boolean;
  /** 输入法会话环境是否配置好（未配置时 Ctrl+Space 不生效） */
  imeEnv: boolean;
  vnc: boolean;
  /** 虚拟机里 5901 端口是否真的在监听（装了包 ≠ 服务在跑） */
  vncPort: boolean;
  /** VNC 服务单元状态：优先 acl-vnc.service，其次发行版单元 */
  vncUnit: string;
  ssh: boolean;
  /** 虚拟机里是否有可用的输出设备（自带扬声器）。 */
  audioSink: boolean;
  /** 当前输出设备名。 */
  audioSinkName: string;
  /** 音频串流模块是否已配置。 */
  audioModule: boolean;
  /** 桌面在虚拟网络里的地址。 */
  audioIp: string;
  message: string;
}

export interface VncSetup {
  port: number;
  forwardOk: boolean;
  forwardDetail: string;
  /** 手机端把端口接到虚拟机的说明（走的是哪条通道） */
  relayDetail: string;
  /** 是否真的能连上 VNC 服务（完成 RFB 握手） */
  reachable: boolean;
  /** 探测详情（成功或失败原因） */
  probeDetail: string;
  viewerFound: boolean;
  viewerPath: string;
  viewerLaunched: boolean;
  connectHint: string;
  message: string;
}

/** 桌面音频串流的状态。 */
export interface AudioReport {
  /** 虚拟机里是否有可用的输出设备（自带扬声器）。 */
  hasSpeaker: boolean;
  /** 当前输出设备名。 */
  sinkName: string;
  /** 串流模块是否已配置。 */
  moduleReady: boolean;
  /** 桌面在虚拟网络里的地址。 */
  guestIp: string;
  /** 播放器实际应连接的地址（直连时等于 guestIp，走中继时是 127.0.0.1）。 */
  connectHost: string;
  /** 播放器实际应连接的端口。 */
  connectPort: number;
  /** 虚拟机内串流模块监听的端口。 */
  port: number;
  /** 手机侧是否真的能连上串流端口。 */
  reachable: boolean;
  /** 是否通过手机端中继连接。 */
  viaRelay: boolean;
  rate: number;
  channels: number;
  /** 手机上是否已安装音频播放器。 */
  playerInstalled: boolean;
  /** 播放器是否已打开。 */
  playerLaunched: boolean;
  /** 播放器里的地址与端口是否已自动填好。 */
  prefilled: boolean;
  /** 播放器是否已经开始串流。 */
  streaming: boolean;
  /** 给用户看的说明。 */
  message: string;
}

export interface DesktopPreset {
  id: string;
  name: string;
  size: string;
}
export const DESKTOP_PRESETS: DesktopPreset[] = [
  { id: "xfce", name: "XFCE", size: "~1.2 GB" },
  { id: "gnome", name: "GNOME", size: "~2.5 GB" },
  { id: "kde", name: "KDE Plasma", size: "~3.5 GB" },
  { id: "mate", name: "MATE", size: "~1.5 GB" },
  { id: "cinnamon", name: "Cinnamon", size: "~2.0 GB" },
  { id: "lxqt", name: "LXQt", size: "~0.8 GB" },
  { id: "lxde", name: "LXDE", size: "~0.8 GB" },
  { id: "flashback", name: "GNOME Flashback", size: "~1.2 GB" },
];
