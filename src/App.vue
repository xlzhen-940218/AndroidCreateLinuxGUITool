<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, onImageProgress, onInstallLog, onInstallStep, onInstallStatus, onVncProgress } from "./api";
import LogPanel from "./components/LogPanel.vue";
import ModalDialog, { type DialogButton } from "./components/ModalDialog.vue";
import { LOCALES, locale, setLocale, t, type Locale } from "./i18n";
import { DESKTOP_PRESETS } from "./types";
import type {
  AvfReport,
  DeviceEntry,
  DevOptionReport,
  EnvInfo,
  ImageStatus,
  InstallOptions,
  InstallStatus,
  InstalledReport,
  PortResult,
  TerminalPhase,
  VncSetup,
} from "./types";

type StepId =
  | "env"
  | "device"
  | "avf"
  | "devenv"
  | "image"
  | "base"
  | "desktop"
  | "apps"
  | "vncsvc"
  | "vnc";

/** 「安装桌面环境」拆出的 4 个独立安装阶段。 */
type StageId = "base" | "desktop" | "apps" | "vncsvc";

type ThemeId = "blue" | "pink" | "dark";

interface DialogSpec {
  title: string;
  message?: string;
  detail?: string;
  tone?: "info" | "warn" | "error" | "success";
  buttons: DialogButton[];
}

const stepIds: StepId[] = [
  "env",
  "device",
  "avf",
  "devenv",
  "image",
  "base",
  "desktop",
  "apps",
  "vncsvc",
  "vnc",
];
const stageIds: StageId[] = ["base", "desktop", "apps", "vncsvc"];
const themeIds: ThemeId[] = ["blue", "pink", "dark"];

const THEME_KEY = "acl.theme";
const theme = ref<ThemeId>((localStorage.getItem(THEME_KEY) as ThemeId) || "blue");

function setTheme(id: ThemeId) {
  theme.value = id;
  localStorage.setItem(THEME_KEY, id);
  document.documentElement.dataset.theme = id;
}

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
const portResult = ref<PortResult | null>(null);
const vnc = ref<VncSetup | null>(null);
const vncProgress = ref("");
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
  vnc: false,
});

const options = reactive<InstallOptions>({
  desktop: "xfce",
  cnMirror: true,
  locale: "zh_CN.UTF-8",
  vncPassword: "linux",
  vncDisplay: ":1",
  vncGeometry: "1920x1080",
  installApps: true,
  installIme: true,
  sshPort: 10022,
});

const unlisteners: Array<() => void> = [];

const selectedDevice = computed(() => devices.value.find((d) => d.serial === serial.value));
const readyDevice = computed(() => devices.value.find((d) => d.state === "device"));
const phaseLabel = computed(() => t(`phase.${phase.value}`));
const installRunning = computed(
  () => install.value.state === "running" || install.value.state === "preparing",
);
const currentPreset = computed(() => DESKTOP_PRESETS.find((p) => p.id === options.desktop));
/** 当前步骤对应的安装阶段（不在 4 个安装步骤内时为 null）。 */
const stageCurrent = computed<StageId | null>(() => {
  const id = stepIds[current.value];
  return stageIds.includes(id as StageId) ? (id as StageId) : null;
});
const stageIndex = (id: StageId) => stepIds.indexOf(id);

function applyTitle() {
  void getCurrentWindow()
    .setTitle(t("app.title"))
    .catch(() => {});
}

function onLocaleChange(e: Event) {
  setLocale((e.target as HTMLSelectElement).value as Locale);
  applyTitle();
}

function showDialog(spec: DialogSpec) {
  dialog.value = spec;
}

function onDialogAction(id: string) {
  const d = dialog.value;
  dialog.value = null;
  if (!d) return;
  handleDialogAction(id);
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
      current.value = Math.min(stageIndex(st) + 1, stepIds.length - 1);
      break;
    }
    case "goto-install":
      current.value = stageIndex("base");
      break;
    case "goto-next":
      current.value = Math.min(stageIndex(activeStage.value) + 1, stepIds.length - 1);
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
    current.value = Math.max(current.value, 1);
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
    current.value = Math.max(current.value, 2);
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
      current.value = Math.max(current.value, 3);
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
    current.value = Math.max(current.value, 4);
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
    current.value = Math.max(current.value, 4);
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
    const r = await api.detectLinuxImage(
      serial.value,
      adbPathInput.value.trim() || undefined,
    );
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
    current.value = Math.max(current.value, 5);
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

/** 检测到已装好桌面时，跳过安装直接进入远程桌面。 */
/** 跳过当前安装阶段，直接进入下一步（只标记当前阶段完成）。 */
function skipStage() {
  const st = stageCurrent.value;
  if (!st) return;
  done[st] = true;
  failed[st] = false;
  current.value = Math.min(stageIndex(st) + 1, stepIds.length - 1);
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
      dialog.value = {
        title: t("inst.cnTitle"),
        message: t("inst.cnMsg"),
        tone: "info",
        buttons: [
          { id: "cn", label: t("inst.cnYes"), primary: true },
          { id: "official", label: t("inst.cnNo") },
          { id: "cancel", label: t("btn.cancel") },
        ],
      };
      const off = watchDialog(
        (id) => {
          off();
          if (id === "cn") resolve(true);
          else if (id === "official") resolve(false);
          else resolve(null);
        },
      );
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

// 弹窗动作回调（用于 startInstall 内的同步等待）
let dialogHandler: ((id: string) => void) | null = null;
function watchDialog(cb: (id: string) => void) {
  dialogHandler = cb;
  return () => {
    dialogHandler = null;
  };
}

function onDialogActionWithHandler(id: string) {
  const h = dialogHandler;
  dialogHandler = null;
  dialog.value = null;
  if (h) {
    h(id);
    return;
  }
  onDialogAction(id);
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
  try {
    const r = await api.enablePort(serial.value, 5901, adbPathInput.value.trim() || undefined);
    portResult.value = r;
    if (!r.ok) {
      showDialog({
        title: t("vnc.portFailTitle"),
        message: r.message,
        tone: "error",
        buttons: [{ id: "retry-port", label: t("btn.retry"), primary: true }],
      });
      return;
    }
    await setupVnc();
  } catch (e) {
    showDialog({
      title: t("vnc.portFailTitle"),
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-port", label: t("btn.retry"), primary: true }],
    });
  } finally {
    busy.value = false;
  }
}

async function setupVnc() {
  busy.value = true;
  vncProgress.value = "";
  try {
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
    showDialog({
      title: t("vnc.readyTitle"),
      message: t("vnc.readyMsg", {
        msg: r.message,
        host: r.connectHint,
        password: options.vncPassword,
      }),
      detail: r.viewerDownloaded
        ? t("vnc.bundledDetail", { version: "1.16.0" })
        : "",
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

/* ------------------------------ 事件 ------------------------------ */

onMounted(async () => {
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
    await onInstallStep((s) => {
      install.value = s;
      if (s.message) logs.value.push(`▶ ${s.message}`);
    }),
  );
  unlisteners.push(
    await onInstallStatus((s) => {
      install.value = s;
      busy.value = false;
      const st = stageIds.includes(s.stage as StageId)
        ? (s.stage as StageId)
        : activeStage.value;
      if (s.state === "success") {
        activeStage.value = st;
        done[st] = true;
        failed[st] = false;
        // 该阶段完成，自动进入下一个步骤
        current.value = Math.min(stageIndex(st) + 1, stepIds.length - 1);
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
});

onUnmounted(() => {
  unlisteners.forEach((f) => f());
});

function goTo(i: number) {
  // 自由跳转：每个步骤都可以单独查看/重跑，不再受"上一步是否完成"限制。
  // 安装阶段彼此独立，用户需要能直接回到没装好的那一步。
  current.value = Math.max(0, Math.min(i, stepIds.length - 1));
  // 进入「安装 Linux 系统」步骤时自动检测是否已安装
  if (i === 4 && serial.value) void detectImage();
  // 进入任一安装阶段时，先看看虚拟机里已经装了什么
  if (i >= 5 && i <= 8 && serial.value && !installed.value) void detectInstalled(true);
}

function stepState(id: StepId): "pending" | "active" | "done" | "failed" {
  if (done[id]) return "done";
  if (failed[id]) return "failed";
  if (stepIds[current.value] === id) return "active";
  return "pending";
}
</script>

<template>
  <div class="app">
    <header class="topbar">
      <div class="brand">
        <div class="logo">AVF</div>
        <div>
          <h1>{{ t("app.title") }}</h1>
          <p>{{ t("app.subtitle") }}</p>
        </div>
      </div>
      <div class="topbar-right">
        <div class="theme-switch" :title="t('theme.label')">
          <button
            v-for="id in themeIds"
            :key="id"
            :class="{ on: theme === id }"
            @click="setTheme(id)"
          >
            <span class="dot" :class="id"></span>{{ t(`theme.${id}`) }}
          </button>
        </div>
        <select class="select" :value="locale" @change="onLocaleChange">
          <option v-for="l in LOCALES" :key="l.id" :value="l.id">{{ l.label }}</option>
        </select>
        <div v-if="env" class="pill" :class="env.adbFound ? 'ok' : 'bad'">
          {{
            env.adbFound
              ? t("app.adbReady", { version: env.adbVersion || "OK" })
              : t("app.adbNotReady")
          }}
        </div>
        <select v-if="devices.length" v-model="serial" class="select" @change="runAvfCheck()">
          <option v-for="d in devices" :key="d.serial" :value="d.serial">
            {{ d.model || d.product || d.serial }} · {{ d.serial }} ({{ d.state }})
          </option>
        </select>
        <button class="btn ghost" :disabled="busy" @click="refreshDevices()">
          {{ t("app.redetect") }}
        </button>
      </div>
    </header>

    <div class="body">
      <aside class="rail">
        <ol class="steps">
          <li
            v-for="(id, i) in stepIds"
            :key="id"
            class="step"
            :class="stepState(id)"
            @click="goTo(i)"
          >
            <span class="step-dot">
              <template v-if="stepState(id) === 'done'">✓</template>
              <template v-else-if="stepState(id) === 'failed'">!</template>
              <template v-else>{{ i + 1 }}</template>
            </span>
            <span class="step-text">
              <strong>{{ t(`step.${id}`) }}</strong>
              <em>{{ t(`step.${id}.desc`) }}</em>
            </span>
          </li>
        </ol>
      </aside>

      <main class="panel">
        <!-- 步骤 1：环境 -->
        <section v-if="current === 0" class="card">
          <h2>{{ t("env.title") }}</h2>
          <p class="lead">{{ t("env.lead") }}</p>
          <div class="row">
            <input v-model="adbPathInput" class="input" :placeholder="t('env.placeholder')" />
            <button class="btn" @click="saveAdbPath()">{{ t("btn.saveDetect") }}</button>
            <button class="btn ghost" @click="showAdbSettings = !showAdbSettings">
              {{ showAdbSettings ? t("btn.collapse") : t("btn.manualPath") }}
            </button>
          </div>
          <div v-if="env" class="kv">
            <div><span>{{ t("env.adbPath") }}</span><code>{{ env.adbPath || t("common.notFound") }}</code></div>
            <div><span>{{ t("env.version") }}</span><code>{{ env.adbVersion || env.error || "-" }}</code></div>
          </div>
          <div class="actions">
            <button class="btn primary" :disabled="busy || !env?.adbFound" @click="refreshDevices()">
              {{ t("btn.nextDevice") }}
            </button>
          </div>
        </section>

        <!-- 步骤 2：设备 -->
        <section v-if="current === 1" class="card">
          <h2>{{ t("device.title") }}</h2>
          <p class="lead">{{ t("device.connected") }}</p>
          <div v-if="devices.length === 0" class="empty">{{ t("device.none") }}</div>
          <table v-else class="table">
            <thead>
              <tr>
                <th>{{ t("device.colSerial") }}</th>
                <th>{{ t("device.colModel") }}</th>
                <th>{{ t("device.colState") }}</th>
                <th>{{ t("device.colProduct") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="d in devices" :key="d.serial" :class="{ picked: d.serial === serial }">
                <td><code>{{ d.serial }}</code></td>
                <td>{{ d.model || "-" }}</td>
                <td><span class="tag" :class="d.state === 'device' ? 'ok' : 'bad'">{{ d.state }}</span></td>
                <td>{{ d.product || "-" }}</td>
              </tr>
            </tbody>
          </table>
          <div class="actions">
            <button class="btn ghost" :disabled="busy" @click="refreshDevices()">{{ t("app.redetect") }}</button>
            <button class="btn primary" :disabled="busy || !readyDevice" @click="runAvfCheck()">
              {{ t("btn.nextAvf") }}
            </button>
          </div>
        </section>

        <!-- 步骤 3：AVF -->
        <section v-if="current === 2" class="card">
          <h2>{{ t("avf.title") }}</h2>
          <p class="lead">
            {{ t("avf.lead") }}<code>adb shell /apex/com.android.virt/bin/vm info</code>
            <code>adb shell pm list features | grep virtualization_framework</code>
          </p>
          <div v-if="avf" class="kv">
            <div><span>{{ t("avf.device") }}</span><code>{{ avf.manufacturer }} {{ avf.model }}</code></div>
            <div><span>{{ t("avf.system") }}</span><code>Android {{ avf.androidRelease }} (API {{ avf.sdkInt }})</code></div>
            <div><span>{{ t("avf.feature") }}</span><code>{{ avf.featureLine || t("avf.featureMiss") }}</code></div>
            <div><span>{{ t("avf.runtime") }}</span><code>{{ avf.virtApexPresent ? t("avf.apexOk") : t("avf.missing") }}</code></div>
            <div><span>{{ t("avf.vmInfo") }}</span><code>{{ avf.vmInfoOk ? t("avf.vmOk") : t("avf.vmFail") }}</code></div>
            <div>
              <span>{{ t("avf.terminal") }}</span>
              <code>{{ avf.terminalPkgPresent ? (avf.terminalPkgEnabled ? t("avf.termOk") : t("avf.termDisabled")) : t("common.notInstalled") }}</code>
            </div>
          </div>
          <pre v-if="avf?.vmInfo" class="codeblock">{{ avf.vmInfo }}</pre>
          <div class="actions">
            <button class="btn ghost" :disabled="busy" @click="runAvfCheck()">{{ t("app.redetect") }}</button>
            <button class="btn primary" :disabled="busy || !avf?.supported" @click="runDevOptions()">
              {{ t("btn.nextDevenv") }}
            </button>
          </div>
        </section>

        <!-- 步骤 4：开发者选项 -->
        <section v-if="current === 3" class="card">
          <h2>{{ t("devenv.title") }}</h2>
          <p class="lead">{{ t("devenv.lead") }}</p>
          <div v-if="devopts" class="kv">
            <div><span>{{ t("devenv.devOptions") }}</span><code>{{ devopts.devOptionsEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
            <div><span>{{ t("devenv.usb") }}</span><code>{{ devopts.adbEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
            <div><span>{{ t("devenv.linuxEnv") }}</span><code>{{ devopts.linuxEnvEnabled ? t("devenv.on") : t("devenv.off") }}</code></div>
            <div v-if="devopts.linuxEntryVisible">
              <span>{{ t("devenv.entry") }}</span>
              <code>{{ devopts.linuxEntryLabel }}<template v-if="devopts.linuxSwitchChecked !== null"> · {{ devopts.linuxSwitchChecked ? t("devenv.switchOn") : t("devenv.switchOff") }}</template></code>
            </div>
            <div><span>{{ t("devenv.model") }}</span><code>{{ devopts.isPixel ? t("devenv.pixel") : t("devenv.nonPixel") }}</code></div>
            <div v-if="devopts.devicePolicyPresent"><span>{{ t("devenv.policy") }}</span><code>{{ devopts.devicePolicyDetail }}</code></div>
          </div>
          <ul v-if="devopts?.notes.length" class="notes">
            <li v-for="(n, i) in devopts.notes" :key="i">{{ n }}</li>
          </ul>
          <div class="actions">
            <button class="btn ghost" :disabled="busy" @click="api.openDevOptions(serial)">
              {{ t("btn.openDevOnPhone") }}
            </button>
            <button class="btn ghost" :disabled="busy || !serial" @click="captureScreen()">
              {{ t("btn.capture") }}
            </button>
            <button class="btn ghost" :disabled="busy" @click="runDevOptions()">{{ t("app.redetect") }}</button>
            <button
              class="btn primary"
              :disabled="busy || !serial || devopts?.linuxEnvEnabled"
              @click="runEnableLinuxEnv()"
            >
              {{ devopts?.linuxEnvEnabled ? t("btn.enabled") : t("btn.enableLinux") }}
            </button>
          </div>
        </section>

        <!-- 步骤 5：安装 Linux 系统 -->
        <section v-if="current === 4" class="card">
          <h2>{{ t("image.title") }}</h2>
          <p class="lead">{{ t("image.lead") }}</p>
          <div class="kv">
            <div><span>{{ t("image.phase") }}</span><code>{{ phaseLabel }}</code></div>
            <div v-if="imageChecking">
              <span>{{ t("image.checkLabel") }}</span><code>{{ t("image.detecting") }}</code>
            </div>
            <div v-else-if="imageStatus?.message">
              <span>{{ t("image.checkLabel") }}</span><code>{{ imageStatus.message }}</code>
            </div>
          </div>
          <ul class="notes">
            <li>{{ t("image.note1") }}</li>
            <li>{{ t("image.note2") }}</li>
          </ul>
          <div class="actions">
            <button class="btn primary" :disabled="busy || imageChecking || !serial" @click="installImage()">
              {{ busy ? t("image.working") : t("image.start") }}
            </button>
            <button class="btn ghost" :disabled="busy || imageChecking || !serial" @click="detectImage()">
              {{ imageChecking ? t("image.detecting") : t("image.detect") }}
            </button>
            <button
              class="btn ghost"
              :disabled="busy || imageStatus?.phase !== 'ready'"
              @click="current = 5"
            >
              {{ t("btn.continueInstall") }}
            </button>
          </div>
        </section>

        <!-- 步骤 6-9：安装阶段（系统基础 / 桌面环境 / 常用软件 / TigerVNC 服务） -->
        <section v-if="stageCurrent" class="card">
          <h2>{{ t(`step.${stageCurrent}`) }}</h2>
          <p class="lead">{{ t(`stage.${stageCurrent}.lead`) }}</p>

          <div v-if="stageCurrent === 'base'" class="grid">
            <div class="field">
              <label>{{ t("inst.desktopParams") }}</label>
              <div class="row">
                <div class="sub">
                  <span>{{ t("inst.sshPort") }}</span>
                  <input v-model.number="options.sshPort" class="input" type="number" />
                </div>
              </div>
            </div>
          </div>

          <div v-if="stageCurrent === 'desktop'" class="grid">
            <div class="field">
              <label>{{ t("inst.desktop") }}</label>
              <div class="presets">
                <button
                  v-for="p in DESKTOP_PRESETS"
                  :key="p.id"
                  class="preset"
                  :class="{ on: options.desktop === p.id }"
                  @click="options.desktop = p.id"
                >
                  <strong>{{ p.name }}</strong>
                  <em>{{ p.size }}</em>
                  <span>{{ t(`preset.${p.id}`) }}</span>
                </button>
              </div>
            </div>
          </div>

          <div v-if="stageCurrent === 'apps'" class="grid">
            <div class="field">
              <label class="check">
                <input v-model="options.installApps" type="checkbox" />
                {{ t("inst.installApps") }}
              </label>
              <label class="check">
                <input v-model="options.installIme" type="checkbox" />
                {{ t("inst.installIme") }}
              </label>
            </div>
          </div>

          <div v-if="stageCurrent === 'vncsvc'" class="grid">
            <div class="field">
              <label>{{ t("inst.desktopParams") }}</label>
              <div class="row">
                <div class="sub">
                  <span>{{ t("inst.vncPassword") }}</span>
                  <input v-model="options.vncPassword" class="input" />
                </div>
                <div class="sub">
                  <span>{{ t("inst.resolution") }}</span>
                  <select v-model="options.vncGeometry" class="input">
                    <option>1280x720</option>
                    <option>1600x900</option>
                    <option>1920x1080</option>
                  </select>
                </div>
              </div>
            </div>
          </div>

          <div v-if="install.state !== 'idle'" class="statusbar" :class="install.state">
            <div class="status-main">
              <strong>{{ install.message || install.state }}</strong>
              <span v-if="install.step">{{ t("inst.step", { step: install.step }) }}</span>
            </div>
            <button v-if="installRunning" class="btn ghost" @click="cancelInstall()">
              {{ t("btn.cancel") }}
            </button>
          </div>

          <div class="inline-note">
            {{ t("inst.terminalStatus", { phase: phaseLabel }) }}
            <button class="link" @click="refreshPhase()">{{ t("btn.refresh") }}</button>
          </div>

          <div v-if="stageCurrent === 'base'" class="detect">
            <div class="detect-head">
              <span>{{ t("inst.detectTitle") }}</span>
              <button
                class="link"
                :disabled="detecting || installRunning || !serial"
                @click="detectInstalled(false)"
              >
                {{ detecting ? t("btn.detecting") : installed ? t("app.redetect") : t("btn.detectNow") }}
              </button>
            </div>
            <div v-if="detecting" class="detect-line">
              {{ t("inst.detectingHint") }}
            </div>
            <div v-else-if="installed" class="detect-grid">
              <span :class="installed.desktop ? 'ok' : 'miss'">
                {{ t("inst.desktopLabel") }}：{{ installed.desktop ? installed.desktopName : t("common.notDetected") }}
              </span>
              <span :class="installed.apps ? 'ok' : 'miss'">
                {{ t("inst.appsLabel") }}：{{ installed.appsInstalled }}/{{ installed.appsTotal }}
              </span>
              <span :class="installed.ime ? 'ok' : 'miss'">
                {{ t("inst.imeLabel") }}：{{ installed.ime ? t("common.installed") : t("common.notInstalled") }}
              </span>
              <span :class="installed.vncPort ? 'ok' : 'miss'">
                {{ t("inst.vncLabel") }}：{{
                  !installed.vnc
                    ? t("common.notInstalled")
                    : installed.vncPort
                      ? t("common.running")
                      : t("common.notRunning")
                }}
              </span>
            </div>
            <div v-else class="detect-line">
              {{ t("inst.detectHint") }}
            </div>
          </div>

          <div
            v-if="stageCurrent === 'base' && installed?.desktop && !done.desktop"
            class="detect-tip"
          >
            {{ t("inst.alreadyInstalled", { name: installed.desktopName }) }}
            <button class="link" @click="skipStage()">{{ t("btn.skipStage") }}</button>
          </div>

          <div class="actions">
            <button
              class="btn primary"
              :disabled="busy || installRunning || !serial"
              @click="startInstall(stageCurrent ?? 'base')"
            >
              {{
                installRunning
                  ? t("btn.installing")
                  : done[stageCurrent ?? "base"]
                    ? t("btn.reinstall")
                    : t("btn.startInstall")
              }}
            </button>
          </div>
        </section>

        <!-- 步骤 10：远程桌面 -->
        <section v-if="stepIds[current] === 'vnc'" class="card">
          <h2>{{ t("vnc.title") }}</h2>
          <p class="lead">{{ t("vnc.lead") }}</p>
          <div v-if="portResult" class="kv">
            <div><span>{{ t("vnc.port") }}</span><code>{{ portResult.message }} ({{ portResult.action }})</code></div>
            <div><span>{{ t("vnc.listen") }}</span><code>{{ portResult.active ? t("vnc.detected") : t("vnc.pending") }}</code></div>
          </div>
          <div v-if="vnc" class="kv">
            <div><span>{{ t("vnc.forward") }}</span><code>{{ vnc.forwardDetail }}</code></div>
            <div>
              <span>{{ t("vnc.probe") }}</span>
              <code>{{ vnc.reachable ? `${t("vnc.probeOk")} — ${vnc.probeDetail}` : `${t("vnc.probeFail")} — ${vnc.probeDetail}` }}</code>
            </div>
            <div><span>{{ t("vnc.client") }}</span><code>{{ vnc.viewerPath || t("vnc.viewerMissing") }}</code></div>
            <div><span>{{ t("vnc.address") }}</span><code>{{ vnc.connectHint }}</code></div>
          </div>
          <p v-if="vncProgress" class="lead">{{ vncProgress }}</p>
          <div class="hintbox">
            <strong>{{ t("vnc.phoneBox") }}</strong>
            <ul>
              <li>{{ t("vnc.addrLabel") }}<code>127.0.0.1</code>　{{ t("vnc.portLabel") }}<code>5901</code></li>
              <li>{{ t("vnc.userLabel") }}<code>droid</code>（{{ t("vnc.passLabel") }}<code>{{ options.vncPassword }}</code>）</li>
              <li>{{ t("vnc.phoneHint") }}</li>
            </ul>
          </div>
          <div class="actions">
            <button class="btn primary" :disabled="busy || !serial" @click="enableVncPort()">
              {{ t("btn.enablePort") }}
            </button>
            <button class="btn" :disabled="busy || !serial" @click="installAvnc()">
              {{ busy ? t("btn.installingAvnc") : t("btn.installAvnc") }}
            </button>
            <button class="btn ghost" @click="api.openExternal('https://tigervnc.org/download/')">
              {{ t("btn.openTigerVnc") }}
            </button>
          </div>
        </section>

        <LogPanel :lines="logs" />
      </main>
    </div>

    <ModalDialog
      v-if="dialog"
      :title="dialog.title"
      :message="dialog.message"
      :detail="dialog.detail"
      :tone="dialog.tone"
      :buttons="dialog.buttons"
      @action="onDialogActionWithHandler"
    />
  </div>
</template>
