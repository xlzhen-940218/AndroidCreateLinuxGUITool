import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AudioReport,
  AvfReport,
  DeviceEntry,
  DevOptionReport,
  EnvInfo,
  InstallOptions,
  InstallStatus,
  ImageStatus,
  InstalledReport,
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
  disablePhantomKiller: (serial: string, adbPath?: string) =>
    invoke<boolean>("disable_phantom_killer", { serial, adbPath }),
  scanSecurity: (serial: string, adbPath?: string) =>
    invoke<SecurityIssue | null>("scan_security", { serial, adbPath }),
  clickAllow: (serial: string, adbPath?: string) =>
    invoke<boolean>("click_allow", { serial, adbPath }),
  terminalPhase: (serial: string, adbPath?: string) =>
    invoke<TerminalPhase>("terminal_phase", { serial, adbPath }),
  detectLinuxImage: (serial: string, adbPath?: string) =>
    invoke<ImageStatus>("detect_linux_image", { serial, adbPath }),
  installLinuxImage: (serial: string, diskSizeGb: number, adbPath?: string) =>
    invoke<ImageStatus>("install_linux_image", { serial, diskSizeGb, adbPath }),
  openTerminal: (serial: string, adbPath?: string) =>
    invoke<void>("open_terminal", { serial, adbPath }),
  detectInstalled: (serial: string, adbPath?: string) =>
    invoke<InstalledReport>("detect_installed", { serial, adbPath }),
  runInstall: (serial: string, options: InstallOptions, stage: string, adbPath?: string) =>
    invoke<InstallStatus>("run_install", { serial, options, stage, adbPath }),
  installStatus: () => invoke<InstallStatus>("install_status"),
  cancelInstall: () => invoke<void>("cancel_install"),
  setupVnc: (serial: string, port: number, adbPath?: string, password?: string) =>
    invoke<VncSetup>("setup_vnc", { serial, port, adbPath, password }),
  installAvnc: (serial: string, adbPath?: string, password?: string) =>
    invoke<string>("install_avnc", { serial, adbPath, password }),
  detectAudio: (serial: string, adbPath?: string) =>
    invoke<AudioReport>("detect_audio", { serial, adbPath }),
  setupAudio: (serial: string, adbPath?: string) =>
    invoke<AudioReport>("setup_audio", { serial, adbPath }),
  openAudioPlayer: (serial: string, host: string, port: number, adbPath?: string) =>
    invoke<AudioReport>("open_audio_player", { serial, host, port, adbPath }),
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

export function onAudioProgress(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>("audio-progress", (e) => cb(e.payload));
}

export function onImageProgress(cb: (status: ImageStatus) => void): Promise<UnlistenFn> {
  return listen<ImageStatus>("image-progress", (e) => cb(e.payload));
}
