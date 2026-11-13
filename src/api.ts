import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AvfReport,
  DeviceEntry,
  DevOptionReport,
  EnvInfo,
  InstallOptions,
  InstallStatus,
  InstalledReport,
  PortResult,
  SecurityIssue,
  TerminalPhase,
  VncSetup,
} from "./types";

export const api = {
  checkAdb: (adbPath?: string) => invoke<EnvInfo>("check_adb", { adbPath }),
  setAdbPath: (adbPath: string) => invoke<void>("set_adb_path", { adbPath }),
  listDevices: (adbPath?: string) => invoke<DeviceEntry[]>("list_devices", { adbPath }),
  checkAvf: (serial: string, adbPath?: string) =>
    invoke<AvfReport>("check_avf", { serial, adbPath }),
  devOptions: (serial: string, adbPath?: string) =>
    invoke<DevOptionReport>("dev_options", { serial, adbPath }),
  openDevOptions: (serial: string, adbPath?: string) =>
    invoke<void>("open_dev_options", { serial, adbPath }),
  enableLinuxEnv: (serial: string, adbPath?: string) =>
    invoke<DevOptionReport>("enable_linux_env", { serial, adbPath }),
  scanSecurity: (serial: string, adbPath?: string) =>
    invoke<SecurityIssue | null>("scan_security", { serial, adbPath }),
  clickAllow: (serial: string, adbPath?: string) =>
    invoke<boolean>("click_allow", { serial, adbPath }),
  terminalPhase: (serial: string, adbPath?: string) =>
    invoke<TerminalPhase>("terminal_phase", { serial, adbPath }),
  openTerminal: (serial: string, adbPath?: string) =>
    invoke<void>("open_terminal", { serial, adbPath }),
  detectInstalled: (serial: string, adbPath?: string) =>
    invoke<InstalledReport>("detect_installed", { serial, adbPath }),
  runInstall: (serial: string, options: InstallOptions, adbPath?: string) =>
    invoke<InstallStatus>("run_install", { serial, options, adbPath }),
  installStatus: () => invoke<InstallStatus>("install_status"),
  cancelInstall: () => invoke<void>("cancel_install"),
  enablePort: (serial: string, port: number, adbPath?: string) =>
    invoke<PortResult>("enable_port", { serial, port, adbPath }),
  setupVnc: (serial: string, port: number, adbPath?: string, password?: string) =>
    invoke<VncSetup>("setup_vnc", { serial, port, adbPath, password }),
  installAvnc: (serial: string, adbPath?: string, password?: string) =>
    invoke<string>("install_avnc", { serial, adbPath, password }),
  captureScreen: (serial: string, adbPath?: string) =>
    invoke<string>("capture_screen", { serial, adbPath }),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
};

export function onInstallLog(cb: (line: string) => void): Promise<UnlistenFn> {
  return listen<string>("install-log", (e) => cb(e.payload));
}

export function onInstallStep(cb: (status: InstallStatus) => void): Promise<UnlistenFn> {
  return listen<InstallStatus>("install-step", (e) => cb(e.payload));
}

export function onInstallStatus(cb: (status: InstallStatus) => void): Promise<UnlistenFn> {
  return listen<InstallStatus>("install-status", (e) => cb(e.payload));
}

export function onVncProgress(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>("vnc-progress", (e) => cb(e.payload));
}
