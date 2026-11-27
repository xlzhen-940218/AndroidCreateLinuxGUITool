import { computed, reactive, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  api,
  onAudioProgress,
  onImageProgress,
  onInstallLog,
  onInstallStep,
  onInstallStatus,
  onVncProgress,
} from "./api";
import { setLocale, t, type Locale } from "./i18n";
import type {
  AudioReport,
  AvfReport,
  DeviceEntry,
  DevOptionReport,
  EnvInfo,
  ImageStatus,
  InstallOptions,
  InstallStatus,
  InstalledReport,
  TerminalPhase,
  VncSetup,
} from "./types";

export type StepId =
  | "env"
  | "device"
  | "avf"
  | "devenv"
  | "image"
  | "base"
  | "desktop"
  | "apps"
  | "vncsvc"
  | "audio"
  | "vnc";

/** 「安装桌面环境」拆出的 4 个独立安装阶段。 */
export type StageId = "base" | "desktop" | "apps" | "vncsvc";

export type ThemeId = "blue" | "pink" | "dark";

export interface DialogSpec {
  title: string;
  message?: string;
  detail?: string;
  tone?: "info" | "warn" | "error" | "success";
  buttons: Array<{ id: string; label: string; primary?: boolean; danger?: boolean }>;
  /** 需要同步等待用户选择时（例如安装前的镜像选择）使用。 */
  onAction?: (id: string) => void;
}

export interface StepGroup {
  id: string;
  titleKey: string;
  steps: StepId[];
}

/** 步骤顺序：准备工作 → 安装阶段 → 使用（音频 → 远程桌面）。 */
export const STEP_ORDER: StepId[] = [
  "env",
  "device",
  "avf",
  "devenv",
  "image",
  "base",
  "desktop",
  "apps",
  "vncsvc",
  "audio",
  "vnc",
];

/** 侧栏分组：功能模块按阶段归类，VNC 固定在最后。 */
export const STEP_GROUPS: StepGroup[] = [
  { id: "prepare", titleKey: "group.prepare", steps: ["env", "device", "avf", "devenv", "image"] },
  { id: "install", titleKey: "group.install", steps: ["base", "desktop", "apps", "vncsvc"] },
  { id: "use", titleKey: "group.use", steps: ["audio", "vnc"] },
];

export const STAGE_IDS: StageId[] = ["base", "desktop", "apps", "vncsvc"];
export const THEME_IDS: ThemeId[] = ["blue", "pink", "dark"];

export function isStage(id: StepId): id is StageId {
  return (STAGE_IDS as string[]).includes(id);
}

/* ------------------------------ 状态 ------------------------------ */

const THEME_KEY = "acl.theme";
const theme = ref<ThemeId>((localStorage.getItem(THEME_KEY) as ThemeId) || "blue");

const current = ref(0);
const env = ref<EnvInfo | null>(null);
const devices = ref<DeviceEntry[]>([]);
const serial = ref("");
const avf = ref<AvfReport | null>(null);
const devopts = ref<DevOptionReport | null>(null);
const phase = ref<TerminalPhase>("unknown");
const imageStatus = ref<ImageStatus | null>(null);
const installed = ref<InstalledReport | null>(null);
const detecting = ref(false);
const imageChecking = ref(false);
const install = ref<InstallStatus>({
  state: "idle",
  message: "",
  step: "",
  stage: "",
  guestDir: "",
  phoneDir: "",
  scriptPath: "",
});
const activeStage = ref<StageId>("base");
const logs = ref<string[]>([]);
const dialog = ref<DialogSpec | null>(null);
const vnc = ref<VncSetup | null>(null);
const vncProgress = ref("");
const audio = ref<AudioReport | null>(null);
const audioProgress = ref("");
const busy = ref(false);
const adbPathInput = ref("");
const showAdbSettings = ref(false);
const cnChoice = ref<boolean | null>(null);
const lastScreenshot = ref("");
const done = reactive<Record<StepId, boolean>>({
  env: false,
  device: false,
  avf: false,
  devenv: false,
  image: false,
  base: false,
  desktop: false,
  apps: false,
  vncsvc: false,
  audio: false,
  vnc: false,
});
const failed = reactive<Record<StepId, boolean>>({
  env: false,
  device: false,
  avf: false,
  devenv: false,
  image: false,
  base: false,
  desktop: false,
  apps: false,
  vncsvc: false,
  audio: false,
  vnc: false,
});

const options = reactive<InstallOptions>({
  desktop: "xfce",
  cnMirror: true,
  locale: "zh_CN.UTF-8",
  vncPassword: "linux",
  vncGeometry: "1920x1080",
  diskSizeGb: 32,
  installApps: true,
  installIme: true,
  sshPort: 10022,
});

const unlisteners: Array<() => void> = [];
let initialized = false;

/* ------------------------------ 计算属性 ------------------------------ */

const selectedDevice = computed(() => devices.value.find((d) => d.serial === serial.value));
const readyDevice = computed(() => devices.value.find((d) => d.state === "device"));
const phaseLabel = computed(() => t(`phase.${phase.value}`));
const installRunning = computed(
  () => install.value.state === "running" || install.value.state === "preparing",
);
const currentStepId = computed(() => STEP_ORDER[current.value]);
/** 当前步骤对应的安装阶段（不在 4 个安装步骤内时为 null）。 */
const stageCurrent = computed<StageId | null>(() => {
  const id = currentStepId.value;
  return isStage(id) ? id : null;
});

export function stepIndex(id: StepId): number {
  return STEP_ORDER.indexOf(id);
}

export function stepState(id: StepId): "pending" | "active" | "done" | "failed" {
  if (done[id]) return "done";
  if (failed[id]) return "failed";
  if (currentStepId.value === id) return "active";
  return "pending";
}

function applyTitle() {
  void getCurrentWindow()
    .setTitle(t("app.title"))
    .catch(() => {});
}

function setTheme(id: ThemeId) {
  theme.value = id;
  localStorage.setItem(THEME_KEY, id);
  document.documentElement.dataset.theme = id;
}

function onLocaleChange(e: Event) {
  setLocale((e.target as HTMLSelectElement).value as Locale);
  applyTitle();
}

/* ------------------------------ 弹窗 ------------------------------ */

function showDialog(spec: DialogSpec) {
  dialog.value = spec;
}

function onDialogAction(id: string) {
  const d = dialog.value;
  dialog.value = null;
  if (!d) return;
  if (d.onAction) {
    d.onAction(id);
    return;
  }
  void handleDialogAction(id);
}

async function handleDialogAction(id: string) {
  switch (id) {
    case "open-settings":
      showAdbSettings.value = true;
      break;
    case "open-dev-options":
      if (serial.value) await api.openDevOptions(serial.value);
      break;
    case "retry-device":
      await refreshDevices();
      break;
    case "retry-avf":
      await runAvfCheck();
      break;
    case "retry-devenv":
      await runEnableLinuxEnv();
      break;
    case "retry-install":
      await startInstall();
      break;
    case "retry-image":
      await installImage();
      break;
    case "continue-install": {
      const st = activeStage.value;
      done[st] = true;
      failed[st] = false;
      current.value = Math.min(stepIndex(st) + 1, STEP_ORDER.length - 1);
      break;
    }
    case "goto-install":
      current.value = stepIndex("base");
      break;
    case "goto-next":
      current.value = Math.min(stepIndex(activeStage.value) + 1, STEP_ORDER.length - 1);
      break;
    case "goto-device":
      current.value = stepIndex("device");
      break;
    case "goto-vnc":
      current.value = stepIndex("vnc");
      break;
    case "open-avnc":
      await installAvnc();
      break;
    case "open-tigervnc-site":
      await api.openExternal("https://tigervnc.org/download/");
      break;
    case "open-screenshot":
      if (lastScreenshot.value) {
        await api.openExternal("file:///" + lastScreenshot.value.replace(/\\/g, "/"));
      }
      break;
    case "retry-port":
      await enableVncPort();
      break;
    case "open-audio-player":
      await openAudioPlayer();
      break;
    default:
      break;
  }
}

/* ------------------------------ 环境与设备 ------------------------------ */

async function initEnv() {
  busy.value = true;
  try {
    const info = await api.checkAdb(adbPathInput.value.trim() || undefined);
    env.value = info;
    if (info.adbFound) {
      done.env = true;
      failed.env = false;
      await refreshDevices();
    } else {
      done.env = false;
      failed.env = true;
      showAdbSettings.value = true;
      showDialog({
        title: t("env.noAdbTitle"),
        message: info.error ?? t("env.noAdbMsg"),
        tone: "error",
        buttons: [
          { id: "open-settings", label: t("btn.settingsAdb"), primary: true },
          { id: "dismiss", label: t("btn.dismiss") },
        ],
      });
    }
  } catch (e) {
    failed.env = true;
    showDialog({
      title: t("env.checkFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  } finally {
    busy.value = false;
  }
}

async function saveAdbPath() {
  const p = adbPathInput.value.trim();
  await api.setAdbPath(p);
  showAdbSettings.value = false;
  await initEnv();
}

async function refreshDevices() {
  busy.value = true;
  try {
    devices.value = await api.listDevices(adbPathInput.value.trim() || undefined);
    const ready = readyDevice.value;
    if (!devices.value.length) {
      done.device = false;
      failed.device = true;
      showDialog({
        title: t("device.noneTitle"),
        message: t("device.noneMsg"),
        tone: "warn",
        buttons: [{ id: "retry-device", label: t("app.redetect"), primary: true }],
      });
      return;
    }
    const unauthorized = devices.value.find((d) => d.state === "unauthorized");
    if (unauthorized) {
      failed.device = true;
      showDialog({
        title: t("device.unauthTitle"),
        message: t("device.unauthMsg"),
        tone: "warn",
        buttons: [{ id: "retry-device", label: t("app.redetect"), primary: true }],
      });
      return;
    }
    if (!ready) {
      failed.device = true;
      showDialog({
        title: t("device.abnormalTitle"),
        message: t("device.abnormalMsg", {
          states: devices.value.map((d) => d.state).join(", "),
        }),
        tone: "error",
        buttons: [{ id: "retry-device", label: t("app.redetect"), primary: true }],
      });
      return;
    }
    serial.value = ready.serial;
    done.device = true;
    failed.device = false;
    // 关闭幽灵进程限制：否则 Linux 虚拟机里的进程会被系统在后台清理
    void api
      .disablePhantomKiller(ready.serial, adbPathInput.value.trim() || undefined)
      .catch(() => {});
    current.value = Math.max(current.value, stepIndex("device"));
    await runAvfCheck();
  } catch (e) {
    failed.device = true;
    showDialog({
      title: t("device.listFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-device", label: t("app.redetect"), primary: true }],
    });
  } finally {
    busy.value = false;
  }
}

/* ------------------------------ AVF 检测 ------------------------------ */

async function runAvfCheck() {
  if (!serial.value) return;
  busy.value = true;
  try {
    const r = await api.checkAvf(serial.value, adbPathInput.value.trim() || undefined);
    avf.value = r;
    if (!r.supported) {
      done.avf = false;
      failed.avf = true;
      showDialog({
        title: t("avf.unsupportedTitle"),
        message: t("avf.unsupportedMsg"),
        detail: r.reasons.join("\n"),
        tone: "error",
        buttons: [
          { id: "retry-avf", label: t("app.redetect"), primary: true },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
      return;
    }
    done.avf = true;
    failed.avf = false;
    current.value = Math.max(current.value, stepIndex("avf"));
    if (!r.terminalPkgPresent) {
      showDialog({
        title: t("avf.noTerminalTitle"),
        message: t("avf.noTerminalMsg"),
        tone: "warn",
        buttons: [{ id: "retry-avf", label: t("app.redetect"), primary: true }],
      });
      return;
    }
    await runDevOptions();
  } catch (e) {
    failed.avf = true;
    showDialog({
      title: t("avf.checkFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-avf", label: t("app.redetect"), primary: true }],
    });
  } finally {
    busy.value = false;
  }
}

/* --------------------------- 开发者选项 --------------------------- */

async function runDevOptions() {
  if (!serial.value) return;
  busy.value = true;
  try {
    const r = await api.devOptions(serial.value, adbPathInput.value.trim() || undefined);
    devopts.value = r;
    if (!r.linuxEnvEnabled) {
      done.devenv = false;
      failed.devenv = false;
      current.value = Math.max(current.value, stepIndex("devenv"));
      if (!r.devOptionsEnabled) {
        showDialog({
          title: t("devenv.notEnabledTitle"),
          message: t("devenv.notEnabledMsg"),
          tone: "warn",
          buttons: [
            { id: "open-dev-options", label: t("btn.openDevOptions"), primary: true },
            { id: "retry-devenv", label: t("app.redetect") },
          ],
        });
      }
      return;
    }
    done.devenv = true;
    failed.devenv = false;
    current.value = Math.max(current.value, stepIndex("image"));
  } catch (e) {
    showDialog({
      title: t("devenv.readFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-devenv", label: t("app.redetect"), primary: true }],
    });
  } finally {
    busy.value = false;
  }
}

async function runEnableLinuxEnv() {
  if (!serial.value) return;
  busy.value = true;
  try {
    const r = await api.enableLinuxEnv(serial.value, adbPathInput.value.trim() || undefined);
    devopts.value = r;

    const issue = await api.scanSecurity(serial.value, adbPathInput.value.trim() || undefined);
    if (issue) {
      failed.devenv = true;
      showDialog({
        title: issue.title,
        message: issue.detail,
        tone: "error",
        buttons: [
          { id: "retry-devenv", label: t("btn.handledRetry"), primary: true },
          { id: "dismiss", label: t("btn.dismiss") },
        ],
      });
      return;
    }

    if (!r.linuxEnvEnabled) {
      failed.devenv = true;
      showDialog({
        title: t("devenv.enableFailTitle"),
        message: t("devenv.enableFailMsg"),
        tone: "error",
        buttons: [
          { id: "open-dev-options", label: t("btn.openDevOptions"), primary: true },
          { id: "retry-devenv", label: t("btn.retry") },
        ],
      });
      return;
    }

    done.devenv = true;
    failed.devenv = false;
    current.value = Math.max(current.value, stepIndex("image"));
    showDialog({
      title: t("devenv.enabledTitle"),
      message: t("devenv.enabledMsg"),
      tone: "success",
      buttons: [{ id: "goto-install", label: t("btn.continueInstall"), primary: true }],
    });
  } catch (e) {
    failed.devenv = true;
    showDialog({
      title: t("devenv.enableErrorTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-devenv", label: t("btn.retry"), primary: true }],
    });
  } finally {
    busy.value = false;
  }
}

/* ------------------------------ 安装 ------------------------------ */

async function refreshPhase() {
  if (!serial.value) return;
  try {
    phase.value = await api.terminalPhase(serial.value, adbPathInput.value.trim() || undefined);
    if (phase.value === "ready") {
      done.image = true;
      failed.image = false;
    }
  } catch {
    phase.value = "unknown";
  }
}

/** 只读检测 Linux 系统是否已经安装（进入步骤时自动执行，不触发下载）。 */
async function detectImage() {
  if (!serial.value) return;
  imageChecking.value = true;
  try {
    const r = await api.detectLinuxImage(serial.value, adbPathInput.value.trim() || undefined);
    imageStatus.value = { ...r, message: t(`image.check.${r.phase}`) };
    phase.value = r.phase;
    if (r.ready) {
      done.image = true;
      failed.image = false;
    }
  } catch (e) {
    imageStatus.value = {
      phase: "unknown",
      ready: false,
      message: `${t("image.check.unknown")}\n${String(e)}`,
    };
    phase.value = "unknown";
  } finally {
    imageChecking.value = false;
  }
}

/** 安装/启动 Linux 系统镜像（位于「Linux 开发环境」与「安装桌面环境」之间）。 */
async function installImage() {
  if (!serial.value) {
    showDialog({
      title: t("inst.noDeviceTitle"),
      message: t("inst.noDeviceMsg"),
      tone: "warn",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
    return;
  }
  busy.value = true;
  imageStatus.value = { phase: "unknown", ready: false, message: t("image.preparing") };
  try {
    const r = await api.installLinuxImage(
      serial.value,
      options.diskSizeGb,
      adbPathInput.value.trim() || undefined,
    );
    imageStatus.value = r;
    phase.value = r.phase;
    if (!r.ready) {
      failed.image = true;
      showDialog({
        title: t("image.incompleteTitle"),
        message: r.message,
        tone: "warn",
        buttons: [
          { id: "retry-image", label: t("btn.retry"), primary: true },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
      return;
    }
    done.image = true;
    failed.image = false;
    imageStatus.value = { ...r, message: t("image.check.ready") };
    current.value = Math.max(current.value, stepIndex("base"));
    showDialog({
      title: t("image.doneTitle"),
      message: t("image.doneMsg"),
      tone: "success",
      buttons: [{ id: "dismiss", label: t("btn.continueInstall"), primary: true }],
    });
  } catch (e) {
    failed.image = true;
    showDialog({
      title: t("image.failTitle"),
      message: String(e),
      tone: "error",
      buttons: [
        { id: "retry-image", label: t("btn.retry"), primary: true },
        { id: "dismiss", label: t("btn.close") },
      ],
    });
  } finally {
    busy.value = false;
  }
}

/** 检测虚拟机内已安装的桌面 / 常用软件 / 输入法。 */
async function detectInstalled(silent = false) {
  if (!serial.value || detecting.value || installRunning.value) return;
  detecting.value = true;
  try {
    installed.value = await api.detectInstalled(
      serial.value,
      adbPathInput.value.trim() || undefined,
    );
  } catch (e) {
    installed.value = null;
    if (!silent) {
      showDialog({
        title: t("inst.detectFailTitle"),
        message: String(e),
        tone: "warn",
        buttons: [{ id: "dismiss", label: t("btn.close") }],
      });
    }
  } finally {
    detecting.value = false;
  }
}

/** 跳过当前安装阶段，直接进入下一步（只标记当前阶段完成）。 */
function skipStage() {
  const st = stageCurrent.value;
  if (!st) return;
  done[st] = true;
  failed[st] = false;
  current.value = Math.min(stepIndex(st) + 1, STEP_ORDER.length - 1);
}

async function startInstall(stage: StageId = activeStage.value) {
  if (!serial.value) {
    showDialog({
      title: t("inst.noDeviceTitle"),
      message: t("inst.noDeviceMsg"),
      tone: "warn",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
    return;
  }

  // 国内镜像 / 语言只在「系统基础」阶段询问一次（重试时沿用上次选择）
  if (stage === "base" && cnChoice.value === null) {
    const picked = await new Promise<boolean | null>((resolve) => {
      showDialog({
        title: t("inst.cnTitle"),
        message: t("inst.cnMsg"),
        tone: "info",
        buttons: [
          { id: "cn", label: t("inst.cnYes"), primary: true },
          { id: "official", label: t("inst.cnNo") },
          { id: "cancel", label: t("btn.cancel") },
        ],
        onAction: (id) => resolve(id === "cn" ? true : id === "official" ? false : null),
      });
    });
    if (picked === null) return;
    cnChoice.value = picked;
  }

  const useCn = cnChoice.value ?? true;
  options.cnMirror = useCn;
  options.locale = useCn ? "zh_CN.UTF-8" : "en_US.UTF-8";
  if (!useCn) options.installIme = false;

  activeStage.value = stage;
  logs.value = [];
  busy.value = true;
  try {
    await api.runInstall(
      serial.value,
      { ...options },
      stage,
      adbPathInput.value.trim() || undefined,
    );
    busy.value = false;
  } catch (e) {
    busy.value = false;
    failed[stage] = true;
    showDialog({
      title: t("inst.startFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [
        { id: "retry-install", label: t("btn.retry"), primary: true },
        { id: "dismiss", label: t("btn.close") },
      ],
    });
  }
}

async function cancelInstall() {
  await api.cancelInstall();
}

async function captureScreen() {
  try {
    const p = await api.captureScreen(serial.value, adbPathInput.value.trim() || undefined);
    lastScreenshot.value = p;
    showDialog({
      title: t("inst.shotTitle"),
      message: t("inst.shotMsg", { path: p }),
      tone: "success",
      buttons: [
        { id: "open-screenshot", label: t("btn.openFolder"), primary: true },
        { id: "dismiss", label: t("btn.close") },
      ],
    });
  } catch (e) {
    showDialog({
      title: t("inst.shotFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  }
}

/* ------------------------------ VNC ------------------------------ */

async function enableVncPort() {
  busy.value = true;
  vncProgress.value = "";
  try {
    // 一次调用把整条链路都走完：
    // 手机端端口中继 → adb forward → RFB 握手探测 → 拉起 PC 端客户端。
    const r = await api.setupVnc(
      serial.value,
      5901,
      adbPathInput.value.trim() || undefined,
      options.vncPassword,
    );
    vnc.value = r;
    const ok = r.forwardOk && r.reachable;
    done.vnc = ok;
    failed.vnc = !ok;
    if (!ok) {
      showDialog({
        title: t("vnc.unreachableTitle"),
        message: r.message,
        tone: "error",
        buttons: [
          { id: "retry-port", label: t("btn.retry"), primary: true },
          { id: "open-avnc", label: t("btn.installAvncPhone") },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
      return;
    }
    if (!r.viewerFound) {
      done.vnc = false;
      failed.vnc = true;
      showDialog({
        title: t("vnc.viewerFailTitle"),
        message: t("vnc.viewerFailMsg"),
        detail: r.message,
        tone: "warn",
        buttons: [
          { id: "open-tigervnc-site", label: t("btn.openDownload"), primary: true },
          { id: "open-avnc", label: t("btn.installAvncPhone") },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
      return;
    }
    if (!r.viewerLaunched) {
      done.vnc = false;
      failed.vnc = true;
      showDialog({
        title: t("vnc.startFailTitle"),
        message: r.message,
        tone: "error",
        buttons: [
          { id: "retry-port", label: t("btn.retry"), primary: true },
          { id: "open-avnc", label: t("btn.installAvncPhone") },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
      return;
    }
    showDialog({
      title: t("vnc.readyTitle"),
      message: t("vnc.readyMsg", {
        msg: r.message,
        host: r.connectHint,
        password: options.vncPassword,
      }),
      tone: "success",
      buttons: [
        { id: "open-avnc", label: t("btn.installAvncPhone"), primary: true },
        { id: "dismiss", label: t("btn.close") },
      ],
    });
  } catch (e) {
    showDialog({
      title: t("vnc.startFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-port", label: t("btn.retry"), primary: true }],
    });
  } finally {
    busy.value = false;
    vncProgress.value = "";
  }
}

/** 一键直达远程桌面：切到 VNC 步骤并立即连接。 */
async function openRemoteDesktop() {
  current.value = stepIndex("vnc");
  if (!serial.value && env.value?.adbFound) {
    await refreshDevices();
  }
  if (!serial.value) {
    // refreshDevices 失败时已经给出了具体原因（未连接 / 未授权），不要覆盖它。
    if (!dialog.value) {
      showDialog({
        title: t("vnc.noDeviceTitle"),
        message: t("vnc.noDeviceMsg"),
        tone: "warn",
        buttons: [
          { id: "goto-device", label: t("vnc.gotoDevice"), primary: true },
          { id: "dismiss", label: t("btn.close") },
        ],
      });
    }
    return;
  }
  if (busy.value) return;
  await enableVncPort();
}

async function installAvnc() {
  busy.value = true;
  vncProgress.value = "";
  try {
    const msg = await api.installAvnc(
      serial.value,
      adbPathInput.value.trim() || undefined,
      options.vncPassword,
    );
    showDialog({
      title: t("vnc.avncTitle"),
      message: msg,
      tone: "info",
      buttons: [{ id: "dismiss", label: t("btn.gotIt"), primary: true }],
    });
  } catch (e) {
    showDialog({
      title: t("vnc.avncFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  } finally {
    busy.value = false;
    vncProgress.value = "";
  }
}

/* ------------------------------ 音频 ------------------------------ */

/**
 * 配置桌面音频。
 *
 * 优先修好 /dev/snd 权限，让虚拟机自带的 virtio-snd 声卡出现在桌面里；
 * 确实没有声卡时才退回 PulseAudio 串流 + 手机上的 Simple Protocol Player。
 */
async function setupAudio() {
  busy.value = true;
  audioProgress.value = "";
  try {
    const r = await api.setupAudio(serial.value, adbPathInput.value.trim() || undefined);
    audio.value = r;
    const ok = r.hasSpeaker || r.reachable;
    done.audio = ok;
    failed.audio = !ok;
    showDialog({
      title: r.hasSpeaker ? t("audio.speakerTitle") : t("audio.readyTitle"),
      message: r.message,
      detail: r.hasSpeaker ? "" : t("audio.playHint"),
      tone: r.hasSpeaker || r.streaming ? "success" : r.reachable ? "info" : "warn",
      buttons:
        r.hasSpeaker || r.streaming
          ? [{ id: "dismiss", label: t("btn.gotIt"), primary: true }]
          : [
              { id: "open-audio-player", label: t("audio.openPlayer"), primary: true },
              { id: "dismiss", label: t("btn.gotIt") },
            ],
    });
  } catch (e) {
    failed.audio = true;
    showDialog({
      title: t("audio.failTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  } finally {
    busy.value = false;
    audioProgress.value = "";
  }
}

/** 只检测音频现状，不改动任何东西。 */
async function detectAudio() {
  busy.value = true;
  try {
    const r = await api.detectAudio(serial.value, adbPathInput.value.trim() || undefined);
    audio.value = r;
  } catch (e) {
    showDialog({
      title: t("audio.failTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  } finally {
    busy.value = false;
  }
}

/** 重新打开播放器并复用上一次的地址，不重复改动虚拟机配置。 */
async function openAudioPlayer() {
  if (!serial.value || !audio.value) return;
  busy.value = true;
  audioProgress.value = "";
  try {
    const r = await api.openAudioPlayer(
      serial.value,
      audio.value.connectHost,
      audio.value.connectPort,
      adbPathInput.value.trim() || undefined,
    );
    audio.value = {
      ...audio.value,
      connectHost: r.connectHost || audio.value.connectHost,
      connectPort: r.connectPort || audio.value.connectPort,
      reachable: r.reachable,
      playerInstalled: r.playerInstalled,
      playerLaunched: r.playerLaunched,
      prefilled: r.prefilled,
      streaming: r.streaming,
      message: r.message,
    };
    done.audio = true;
    failed.audio = false;
    showDialog({
      title: t("audio.playerTitle"),
      message: r.message,
      tone: "success",
      buttons: [{ id: "dismiss", label: t("btn.gotIt"), primary: true }],
    });
  } catch (e) {
    showDialog({
      title: t("audio.failTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: t("btn.close") }],
    });
  } finally {
    busy.value = false;
    audioProgress.value = "";
  }
}

/* ------------------------------ 初始化 ------------------------------ */

async function init() {
  if (initialized) return;
  initialized = true;
  document.documentElement.dataset.theme = theme.value;
  applyTitle();
  unlisteners.push(await onInstallLog((line) => logs.value.push(line)));
  unlisteners.push(
    await onImageProgress((s) => {
      imageStatus.value = s;
      phase.value = s.phase;
      if (s.message) logs.value.push(`▶ ${s.message}`);
    }),
  );
  unlisteners.push(
    await onVncProgress((msg) => {
      vncProgress.value = msg;
      logs.value.push(msg);
    }),
  );
  unlisteners.push(
    await onAudioProgress((msg) => {
      audioProgress.value = msg;
      logs.value.push(msg);
    }),
  );
  unlisteners.push(
    await onInstallStep((s) => {
      install.value = s;
      if (s.message) logs.value.push(`▶ ${s.message}`);
    }),
  );
  unlisteners.push(
    await onInstallStatus((s) => {
      install.value = s;
      busy.value = false;
      const st = isStage(s.stage as StepId) ? (s.stage as StageId) : activeStage.value;
      if (s.state === "success") {
        activeStage.value = st;
        done[st] = true;
        failed[st] = false;
        current.value = Math.min(stepIndex(st) + 1, STEP_ORDER.length - 1);
        showDialog({
          title: t("inst.doneTitle"),
          message: t(`stage.${st}.done`),
          tone: "success",
          buttons: [{ id: "goto-next", label: t("btn.next"), primary: true }],
        });
        // VNC 阶段结束后重新检测，确认 5901 是真的在监听
        if (st === "vncsvc") void detectInstalled(true);
      } else if (s.state === "failed") {
        activeStage.value = st;
        done[st] = false;
        failed[st] = true;
        showDialog({
          title: t("inst.failTitle"),
          message: t("inst.failMsg"),
          detail: s.message,
          tone: "error",
          buttons: [
            { id: "retry-install", label: t("btn.retry"), primary: true },
            { id: "continue-install", label: t("btn.skipContinue") },
            { id: "dismiss", label: t("btn.viewLog") },
          ],
        });
      }
    }),
  );
  await initEnv();
}

function dispose() {
  unlisteners.forEach((f) => f());
  unlisteners.length = 0;
  initialized = false;
}

function goTo(i: number) {
  // 自由跳转：每个步骤都可以单独查看/重跑，不再受"上一步是否完成"限制。
  current.value = Math.max(0, Math.min(i, STEP_ORDER.length - 1));
  const id = STEP_ORDER[current.value];
  // 进入「安装 Linux 系统」步骤时自动检测是否已安装
  if (id === "image" && serial.value) void detectImage();
  // 进入任一安装阶段时，先看看虚拟机里已经装了什么
  if (isStage(id) && serial.value && !installed.value) void detectInstalled(true);
}

export function useApp() {
  return {
    // 主题与语言
    theme,
    themeIds: THEME_IDS,
    setTheme,
    onLocaleChange,
    // 导航
    current,
    currentStepId,
    stepGroups: STEP_GROUPS,
    stepIds: STEP_ORDER,
    stageIds: STAGE_IDS,
    stepIndex,
    stepState,
    goTo,
    stageCurrent,
    // 环境 / 设备
    env,
    devices,
    serial,
    selectedDevice,
    readyDevice,
    avf,
    devopts,
    phase,
    phaseLabel,
    imageStatus,
    installed,
    detecting,
    imageChecking,
    install,
    installRunning,
    activeStage,
    logs,
    dialog,
    vnc,
    vncProgress,
    audio,
    audioProgress,
    busy,
    adbPathInput,
    showAdbSettings,
    options,
    done,
    failed,
    // 操作
    init,
    dispose,
    showDialog,
    onDialogAction,
    initEnv,
    saveAdbPath,
    refreshDevices,
    runAvfCheck,
    runDevOptions,
    runEnableLinuxEnv,
    refreshPhase,
    detectImage,
    installImage,
    detectInstalled,
    skipStage,
    startInstall,
    cancelInstall,
    captureScreen,
    enableVncPort,
    openRemoteDesktop,
    installAvnc,
    setupAudio,
    detectAudio,
    openAudioPlayer,
  };
}
