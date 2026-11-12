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

export interface InstallOptions {
  desktop: string;
  cnMirror: boolean;
  locale: string;
  vncPassword: string;
  vncDisplay: string;
  vncGeometry: string;
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
  guestDir: string;
  phoneDir: string;
  scriptPath: string;
}

export interface PortResult {
  port: number;
  ok: boolean;
  action: string;
  message: string;
  active: boolean;
}

export interface VncSetup {
  port: number;
  forwardOk: boolean;
  forwardDetail: string;
  viewerFound: boolean;
  /** 客户端是本次自动下载得到的 */
  viewerDownloaded: boolean;
  viewerPath: string;
  viewerLaunched: boolean;
  connectHint: string;
  message: string;
}

export interface DesktopPreset {
  id: string;
  name: string;
  size: string;
  desc: string;
}

export const DESKTOP_PRESETS: DesktopPreset[] = [
  { id: "xfce", name: "XFCE", size: "约 1.2 GB", desc: "轻量省内存，手机上体验最流畅（推荐）" },
  { id: "gnome", name: "GNOME", size: "约 2.5 GB", desc: "官方风格，功能完整，资源占用较高" },
  { id: "kde", name: "KDE Plasma", size: "约 3.5 GB", desc: "最接近 Windows 的桌面体验" },
  { id: "mate", name: "MATE", size: "约 1.5 GB", desc: "经典 GNOME 衍生，简洁稳定" },
  { id: "cinnamon", name: "Cinnamon", size: "约 2.0 GB", desc: "布局类似 Windows，动画顺滑" },
  { id: "lxqt", name: "LXQt", size: "约 0.8 GB", desc: "超轻量，适合低配设备" },
  { id: "lxde", name: "LXDE", size: "约 0.8 GB", desc: "老牌轻量桌面" },
  { id: "flashback", name: "GNOME Flashback", size: "约 1.2 GB", desc: "GNOME 3 经典模式" },
];
