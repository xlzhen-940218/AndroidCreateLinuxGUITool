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
  vnc: boolean;
  ssh: boolean;
  message: string;
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
