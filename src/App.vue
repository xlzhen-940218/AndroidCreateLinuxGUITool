<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { api, onInstallLog, onInstallStep, onInstallStatus, onVncProgress } from "./api";
import LogPanel from "./components/LogPanel.vue";
import ModalDialog, { type DialogButton } from "./components/ModalDialog.vue";
import { DESKTOP_PRESETS } from "./types";
import type {
  AvfReport,
  DeviceEntry,
  DevOptionReport,
  EnvInfo,
  InstallOptions,
  InstallStatus,
  InstalledReport,
  PortResult,
  TerminalPhase,
  VncSetup,
} from "./types";

type StepId = "env" | "device" | "avf" | "devenv" | "install" | "vnc";

interface StepDef {
  id: StepId;
  title: string;
  desc: string;
}

interface DialogSpec {
  title: string;
  message?: string;
  detail?: string;
  tone?: "info" | "warn" | "error" | "success";
  buttons: DialogButton[];
}

const steps: StepDef[] = [
  { id: "env", title: "环境准备", desc: "定位 adb 可执行文件" },
  { id: "device", title: "设备检测", desc: "确认 Android 设备已连接" },
  { id: "avf", title: "虚拟化检测", desc: "确认支持 AVF Linux 虚拟机" },
  { id: "devenv", title: "Linux 开发环境", desc: "打开开发者选项中的开关" },
  { id: "install", title: "安装桌面环境", desc: "执行 Debian 桌面一键安装" },
  { id: "vnc", title: "远程桌面", desc: "端口转发与 VNC 连接" },
];

const current = ref(0);
const env = ref<EnvInfo | null>(null);
const devices = ref<DeviceEntry[]>([]);
const serial = ref("");
const avf = ref<AvfReport | null>(null);
const devopts = ref<DevOptionReport | null>(null);
const phase = ref<TerminalPhase>("unknown");
const installed = ref<InstalledReport | null>(null);
const detecting = ref(false);
const install = ref<InstallStatus>({
  state: "idle",
  message: "",
  step: "",
  guestDir: "",
  phoneDir: "",
  scriptPath: "",
});
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
  install: false,
  vnc: false,
});
const failed = reactive<Record<StepId, boolean>>({
  env: false,
  device: false,
  avf: false,
  devenv: false,
  install: false,
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
const phaseLabel = computed(() => {
  const map: Record<TerminalPhase, string> = {
    notInstalled: "未安装 Terminal 应用",
    needsInstall: "等待安装系统镜像",
    installing: "正在安装系统镜像",
    booting: "虚拟机启动中",
    ready: "终端已就绪",
    unknown: "未知",
  };
  return map[phase.value];
});
const installRunning = computed(
  () => install.value.state === "running" || install.value.state === "preparing",
);
const currentPreset = computed(() => DESKTOP_PRESETS.find((p) => p.id === options.desktop));

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
    case "continue-install":
      done.install = true;
      failed.install = false;
      current.value = 4;
      break;
    case "goto-install":
      current.value = 4;
      break;
    case "goto-vnc":
      current.value = 5;
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
        title: "未找到 adb",
        message: info.error ?? "请安装 Android Platform Tools 并指定 adb.exe 路径。",
        tone: "error",
        buttons: [
          { id: "open-settings", label: "设置 adb 路径", primary: true },
          { id: "dismiss", label: "稍后处理" },
        ],
      });
    }
  } catch (e) {
    failed.env = true;
    showDialog({
      title: "检测 adb 失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: "关闭" }],
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
        title: "未检测到设备",
        message:
          "请用 USB 数据线连接 Android 手机，并在手机上开启「开发者选项 → USB 调试」，\n连接后点击「重新检测」。",
        tone: "warn",
        buttons: [{ id: "retry-device", label: "重新检测", primary: true }],
      });
      return;
    }
    const unauthorized = devices.value.find((d) => d.state === "unauthorized");
    if (unauthorized) {
      failed.device = true;
      showDialog({
        title: "设备未授权",
        message:
          "手机上会弹出「允许 USB 调试吗？」的确认框（部分机型还会要求输入本机电脑的 RSA 指纹），\n请勾选「一律允许」并点击「允许」。",
        tone: "warn",
        buttons: [{ id: "retry-device", label: "重新检测", primary: true }],
      });
      return;
    }
    if (!ready) {
      failed.device = true;
      showDialog({
        title: "设备状态异常",
        message: `当前设备状态：${devices.value.map((d) => d.state).join(", ")}。\n请重新插拔数据线或重启 adb 后重试。`,
        tone: "error",
        buttons: [{ id: "retry-device", label: "重新检测", primary: true }],
      });
      return;
    }
    serial.value = ready.serial;
    done.device = true;
    failed.device = false;
    current.value = Math.max(current.value, 1);
    await runAvfCheck();
  } catch (e) {
    failed.device = true;
    showDialog({
      title: "列出设备失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-device", label: "重新检测", primary: true }],
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
        title: "该设备不支持 Linux 虚拟机",
        message:
          "当前手机/系统没有提供 Android 虚拟化框架（AVF）所需的运行时，无法创建 Linux 虚拟机。",
        detail: r.reasons.join("\n"),
        tone: "error",
        buttons: [
          { id: "retry-avf", label: "重新检测", primary: true },
          { id: "dismiss", label: "关闭" },
        ],
      });
      return;
    }
    done.avf = true;
    failed.avf = false;
    current.value = Math.max(current.value, 2);
    if (!r.terminalPkgPresent) {
      showDialog({
        title: "当前系统未包含 Terminal 应用",
        message:
          "Terminal 是 Android 随系统分发的内置应用（应用商店无法搜索、也无法单独安装或更新）。\n它只能通过「开发者选项 → Linux 开发环境」启用，并且需要当前系统版本已经包含它。\n如果此处显示缺失，请先通过 OTA 或官方 ROM 升级到包含 Terminal 的系统版本，然后重新检测。",
        tone: "warn",
        buttons: [{ id: "retry-avf", label: "重新检测", primary: true }],
      });
      return;
    }
    await runDevOptions();
  } catch (e) {
    failed.avf = true;
    showDialog({
      title: "AVF 检测失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-avf", label: "重新检测", primary: true }],
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
          title: "开发者选项尚未开启",
          message:
            "请在手机上打开：设置 → 关于手机 → 连续点击「版本号」7 次，输入锁屏密码后即可开启开发者选项。\n完成后点击「重新检测」。",
          tone: "warn",
          buttons: [
            { id: "open-dev-options", label: "打开开发者选项", primary: true },
            { id: "retry-devenv", label: "重新检测" },
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
      title: "读取开发者选项状态失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-devenv", label: "重新检测", primary: true }],
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
          { id: "retry-devenv", label: "我已处理，重试", primary: true },
          { id: "dismiss", label: "稍后处理" },
        ],
      });
      return;
    }

    if (!r.linuxEnvEnabled) {
      failed.devenv = true;
      showDialog({
        title: "未能开启 Linux 开发环境",
        message:
          "系统拒绝了开启操作。常见原因：设备受企业策略管控、未开启开发者选项、或需要重启手机后再试。",
        tone: "error",
        buttons: [
          { id: "open-dev-options", label: "打开开发者选项", primary: true },
          { id: "retry-devenv", label: "重试" },
        ],
      });
      return;
    }

    done.devenv = true;
    failed.devenv = false;
    current.value = Math.max(current.value, 4);
    showDialog({
      title: "Linux 开发环境已开启",
      message: "接下来将打开手机上的 Terminal 应用并执行桌面环境安装脚本。",
      tone: "success",
      buttons: [{ id: "goto-install", label: "继续安装", primary: true }],
    });
  } catch (e) {
    failed.devenv = true;
    showDialog({
      title: "开启 Linux 开发环境失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-devenv", label: "重试", primary: true }],
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
  } catch {
    phase.value = "unknown";
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
        title: "检测已安装环境失败",
        message: String(e),
        tone: "warn",
        buttons: [{ id: "dismiss", label: "关闭" }],
      });
    }
  } finally {
    detecting.value = false;
  }
}

/** 检测到已装好桌面时，跳过安装直接进入远程桌面。 */
function skipToVnc() {
  done.install = true;
  failed.install = false;
  current.value = 5;
}

async function startInstall() {
  if (!serial.value) {
    showDialog({
      title: "尚未选择设备",
      message: "请先完成设备检测。",
      tone: "warn",
      buttons: [{ id: "dismiss", label: "关闭" }],
    });
    return;
  }

  // 弹窗询问是否中国用户 / 是否切换国内源（重试时沿用上次选择）
  let useCn = cnChoice.value;
  if (useCn === null) {
    useCn = await new Promise<boolean | null>((resolve) => {
      dialog.value = {
        title: "您是否为中国用户？",
        message:
          "选择「是」将同时把系统语言设为简体中文、启用 Fcitx5 中文输入法，并切换 apt 到国内镜像源；\n选择「否」则使用英文界面与 Debian 官方源。",
        tone: "info",
        buttons: [
          { id: "cn", label: "是，使用国内源", primary: true },
          { id: "official", label: "否，使用官方源" },
          { id: "cancel", label: "取消" },
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
    if (useCn === null) return;
    cnChoice.value = useCn;
  }

  options.cnMirror = useCn;
  options.locale = useCn ? "zh_CN.UTF-8" : "en_US.UTF-8";
  options.installIme = useCn;

  logs.value = [];
  busy.value = true;
  try {
    await api.runInstall(serial.value, { ...options }, adbPathInput.value.trim() || undefined);
    busy.value = false;
  } catch (e) {
    busy.value = false;
    failed.install = true;
    showDialog({
      title: "启动安装失败",
      message: String(e),
      tone: "error",
      buttons: [
        { id: "retry-install", label: "重试", primary: true },
        { id: "dismiss", label: "关闭" },
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
      title: "已保存手机截图",
      message: `截图已保存到：\n${p}`,
      tone: "success",
      buttons: [
        { id: "open-screenshot", label: "打开所在文件夹", primary: true },
        { id: "dismiss", label: "关闭" },
      ],
    });
  } catch (e) {
    showDialog({
      title: "截图失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: "关闭" }],
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
        title: "端口放行失败",
        message: r.message,
        tone: "error",
        buttons: [{ id: "retry-port", label: "重试", primary: true }],
      });
      return;
    }
    await setupVnc();
  } catch (e) {
    showDialog({
      title: "端口放行失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-port", label: "重试", primary: true }],
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
    done.vnc = r.forwardOk;
    failed.vnc = !r.forwardOk;
    if (!r.viewerFound) {
      showDialog({
        title: "未能准备 TigerVNC Viewer",
        message:
          "本机没有 TigerVNC 客户端，自动下载也未成功。可以先在手机上使用 AVNC 连接，也可以手动安装 TigerVNC 后再一键打开。",
        detail: r.message,
        tone: "warn",
        buttons: [
          { id: "open-tigervnc-site", label: "打开下载页", primary: true },
          { id: "open-avnc", label: "在手机上安装 AVNC" },
          { id: "dismiss", label: "关闭" },
        ],
      });
      return;
    }
    showDialog({
      title: "远程桌面已就绪",
      message: `${r.message}\n\n在电脑端 TigerVNC 中连接：${r.connectHint}\n在手机端 AVNC 中连接：127.0.0.1:5901`,
      detail: r.viewerDownloaded
        ? "本机原本没有 TigerVNC Viewer，工具已自动下载官方免安装版并直接使用。"
        : "",
      tone: "success",
      buttons: [
        { id: "open-avnc", label: "在手机上安装 AVNC", primary: true },
        { id: "dismiss", label: "关闭" },
      ],
    });
  } catch (e) {
    showDialog({
      title: "启动 VNC 失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "retry-port", label: "重试", primary: true }],
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
    const msg = await api.installAvnc(serial.value, adbPathInput.value.trim() || undefined);
    showDialog({
      title: "AVNC 客户端",
      message: msg,
      tone: "info",
      buttons: [{ id: "dismiss", label: "知道了", primary: true }],
    });
  } catch (e) {
    showDialog({
      title: "安装 AVNC 失败",
      message: String(e),
      tone: "error",
      buttons: [{ id: "dismiss", label: "关闭" }],
    });
  } finally {
    busy.value = false;
    vncProgress.value = "";
  }
}

/* ------------------------------ 事件 ------------------------------ */

onMounted(async () => {
  unlisteners.push(await onInstallLog((line) => logs.value.push(line)));
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
      if (s.state === "success") {
        done.install = true;
        failed.install = false;
        // 安装完成直接进入「远程桌面」，弹窗只是提示
        current.value = 5;
        showDialog({
          title: "桌面环境安装完成",
          message:
            "Debian 桌面环境已经装好，VNC 服务已在 5901 端口监听。\n下一步：放行端口并连接远程桌面。",
          tone: "success",
          buttons: [{ id: "goto-vnc", label: "下一步", primary: true }],
        });
      } else if (s.state === "failed") {
        done.install = false;
        failed.install = true;
        showDialog({
          title: "桌面环境安装失败",
          message:
            "安装过程中出现错误。脚本是幂等的，重试会跳过已完成的步骤，可放心点击重试。",
          detail: s.message,
          tone: "error",
          buttons: [
            { id: "retry-install", label: "重试", primary: true },
            { id: "continue-install", label: "跳过并继续" },
            { id: "dismiss", label: "查看日志" },
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
  // 已完成的步骤可回看；只要前一步已完成，也允许进入下一步
  const prevDone = i > 0 && done[steps[i - 1].id];
  if (i <= current.value || done[steps[i].id] || prevDone) {
    current.value = i;
    // 进入安装步骤时先看看虚拟机里是不是已经装过了
    if (i === 4 && !installed.value && serial.value) void detectInstalled(true);
  }
}

function stepState(id: StepId): "pending" | "active" | "done" | "failed" {
  if (done[id]) return "done";
  if (failed[id]) return "failed";
  if (steps[current.value].id === id) return "active";
  return "pending";
}
</script>

<template>
  <div class="app">
    <header class="topbar">
      <div class="brand">
        <div class="logo">AVF</div>
        <div>
          <h1>Android Linux 桌面一键安装</h1>
          <p>Android 16+ 虚拟化框架（AVF）· Debian 桌面环境 · TigerVNC 远程访问</p>
        </div>
      </div>
      <div class="topbar-right">
        <div v-if="env" class="pill" :class="env.adbFound ? 'ok' : 'bad'">
          {{ env.adbFound ? `adb ${env.adbVersion || "就绪"}` : "adb 未就绪" }}
        </div>
        <select v-if="devices.length" v-model="serial" class="select" @change="runAvfCheck()">
          <option v-for="d in devices" :key="d.serial" :value="d.serial">
            {{ d.model || d.product || d.serial }} · {{ d.serial }} ({{ d.state }})
          </option>
        </select>
        <button class="btn ghost" :disabled="busy" @click="refreshDevices()">重新检测</button>
      </div>
    </header>

    <div class="body">
      <aside class="rail">
        <ol class="steps">
          <li
            v-for="(s, i) in steps"
            :key="s.id"
            class="step"
            :class="stepState(s.id)"
            @click="goTo(i)"
          >
            <span class="step-dot">
              <template v-if="stepState(s.id) === 'done'">✓</template>
              <template v-else-if="stepState(s.id) === 'failed'">!</template>
              <template v-else>{{ i + 1 }}</template>
            </span>
            <span class="step-text">
              <strong>{{ s.title }}</strong>
              <em>{{ s.desc }}</em>
            </span>
          </li>
        </ol>
      </aside>

      <main class="panel">
        <!-- 步骤 1：环境 -->
        <section v-if="current === 0" class="card">
          <h2>环境准备</h2>
          <p class="lead">
            本工具通过 <code>adb</code> 驱动手机完成全部操作。请确认已安装
            Android Platform Tools（adb），否则请手动指定 adb.exe 的完整路径。
          </p>
          <div class="row">
            <input v-model="adbPathInput" class="input" placeholder="例如 C:\Users\xxx\AppData\Local\Android\Sdk\platform-tools\adb.exe" />
            <button class="btn" @click="saveAdbPath()">保存并检测</button>
            <button class="btn ghost" @click="showAdbSettings = !showAdbSettings">
              {{ showAdbSettings ? "收起" : "手动指定路径" }}
            </button>
          </div>
          <div v-if="env" class="kv">
            <div><span>adb 路径</span><code>{{ env.adbPath || "未找到" }}</code></div>
            <div><span>版本</span><code>{{ env.adbVersion || env.error || "-" }}</code></div>
          </div>
          <div class="actions">
            <button class="btn primary" :disabled="busy || !env?.adbFound" @click="refreshDevices()">
              下一步：检测设备
            </button>
          </div>
        </section>

        <!-- 步骤 2：设备 -->
        <section v-if="current === 1" class="card">
          <h2>设备检测</h2>
          <p class="lead">已连接设备：</p>
          <div v-if="devices.length === 0" class="empty">未检测到任何设备</div>
          <table v-else class="table">
            <thead>
              <tr><th>序列号</th><th>型号</th><th>状态</th><th>产品</th></tr>
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
            <button class="btn ghost" :disabled="busy" @click="refreshDevices()">重新检测</button>
            <button class="btn primary" :disabled="busy || !readyDevice" @click="runAvfCheck()">
              下一步：虚拟化检测
            </button>
          </div>
        </section>

        <!-- 步骤 3：AVF -->
        <section v-if="current === 2" class="card">
          <h2>虚拟化能力检测</h2>
          <p class="lead">
            检测命令：<code>adb shell /apex/com.android.virt/bin/vm info</code> 与
            <code>adb shell pm list features | grep virtualization_framework</code>
          </p>
          <div v-if="avf" class="kv">
            <div><span>设备</span><code>{{ avf.manufacturer }} {{ avf.model }}</code></div>
            <div><span>系统</span><code>Android {{ avf.androidRelease }} (API {{ avf.sdkInt }})</code></div>
            <div><span>系统特性</span><code>{{ avf.featureLine || "未检测到 virtualization_framework" }}</code></div>
            <div><span>AVF 运行时</span><code>{{ avf.virtApexPresent ? "com.android.virt 已安装" : "缺失" }}</code></div>
            <div><span>vm info</span><code>{{ avf.vmInfoOk ? "执行成功" : "执行失败" }}</code></div>
            <div><span>Terminal 应用</span><code>{{ avf.terminalPkgPresent ? (avf.terminalPkgEnabled ? "已安装并启用" : "已安装未启用") : "未安装" }}</code></div>
          </div>
          <pre v-if="avf?.vmInfo" class="codeblock">{{ avf.vmInfo }}</pre>
          <div class="actions">
            <button class="btn ghost" :disabled="busy" @click="runAvfCheck()">重新检测</button>
            <button class="btn primary" :disabled="busy || !avf?.supported" @click="runDevOptions()">
              下一步：Linux 开发环境
            </button>
          </div>
        </section>

        <!-- 步骤 4：开发者选项 -->
        <section v-if="current === 3" class="card">
          <h2>Linux 开发环境</h2>
          <p class="lead">
            Terminal 是系统内置应用，只能在「开发者选项 → Linux 开发环境」中启用（应用商店无法安装或单独更新）。
            工具调用的系统接口与该开关完全一致，效果等同，无需手动点击；
            你也可以在手机上开发者选项里手动打开。
          </p>
          <div v-if="devopts" class="kv">
            <div><span>开发者选项</span><code>{{ devopts.devOptionsEnabled ? "已开启" : "未开启" }}</code></div>
            <div><span>USB 调试</span><code>{{ devopts.adbEnabled ? "已开启" : "未开启" }}</code></div>
            <div><span>Linux 开发环境</span><code>{{ devopts.linuxEnvEnabled ? "已开启" : "未开启" }}</code></div>
            <div v-if="devopts.linuxEntryVisible"><span>开发者选项条目</span><code>{{ devopts.linuxEntryLabel }}<template v-if="devopts.linuxSwitchChecked !== null"> · 开关{{ devopts.linuxSwitchChecked ? "已打开" : "未打开" }}</template></code></div>
            <div><span>机型</span><code>{{ devopts.isPixel ? "Pixel（无额外安全限制）" : "非 Pixel（可能有 Knox / USB 安全调试限制）" }}</code></div>
            <div v-if="devopts.devicePolicyPresent"><span>设备策略</span><code>{{ devopts.devicePolicyDetail }}</code></div>
          </div>
          <ul v-if="devopts?.notes.length" class="notes">
            <li v-for="(n, i) in devopts.notes" :key="i">{{ n }}</li>
          </ul>
          <div class="actions">
            <button class="btn ghost" :disabled="busy" @click="api.openDevOptions(serial)">
              在手机上打开开发者选项
            </button>
            <button class="btn ghost" :disabled="busy || !serial" @click="captureScreen()">
              截取手机屏幕
            </button>
            <button class="btn ghost" :disabled="busy" @click="runDevOptions()">重新检测</button>
            <button
              class="btn primary"
              :disabled="busy || !serial || devopts?.linuxEnvEnabled"
              @click="runEnableLinuxEnv()"
            >
              {{ devopts?.linuxEnvEnabled ? "已开启" : "自动开启 Linux 开发环境" }}
            </button>
          </div>
        </section>

        <!-- 步骤 5：安装 -->
        <section v-if="current === 4" class="card">
          <h2>安装 Linux 桌面环境</h2>
          <p class="lead">
            工具会把安装脚本推送到手机共享目录，再在 Terminal 应用中执行。
            桌面环境与 VNC 的安装进度会实时显示在下方日志中。
          </p>

          <div class="grid">
            <div class="field">
              <label>桌面环境</label>
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
                  <span>{{ p.desc }}</span>
                </button>
              </div>
            </div>

            <div class="field">
              <label>虚拟桌面参数</label>
              <div class="row">
                <div class="sub">
                  <span>VNC 密码</span>
                  <input v-model="options.vncPassword" class="input" />
                </div>
                <div class="sub">
                  <span>分辨率</span>
                  <select v-model="options.vncGeometry" class="input">
                    <option>1280x720</option>
                    <option>1600x900</option>
                    <option>1920x1080</option>
                  </select>
                </div>
                <div class="sub">
                  <span>SSH 端口</span>
                  <input v-model.number="options.sshPort" class="input" type="number" />
                </div>
              </div>
              <label class="check">
                <input v-model="options.installApps" type="checkbox" />
                安装常用软件套件（Chromium / LibreOffice / VLC / GIMP 等，耗时较长）
              </label>
              <label class="check">
                <input v-model="options.installIme" type="checkbox" />
                安装中文输入法 Fcitx5
              </label>
            </div>
          </div>

          <div v-if="install.state !== 'idle'" class="statusbar" :class="install.state">
            <div class="status-main">
              <strong>{{ install.message || install.state }}</strong>
              <span v-if="install.step">阶段：{{ install.step }}</span>
            </div>
            <button v-if="installRunning" class="btn ghost" @click="cancelInstall()">取消</button>
          </div>

          <div class="inline-note">
            Terminal 应用状态：{{ phaseLabel }}
            <button class="link" @click="refreshPhase()">刷新</button>
          </div>

          <div class="detect">
            <div class="detect-head">
              <span>已安装环境检测</span>
              <button
                class="link"
                :disabled="detecting || installRunning || !serial"
                @click="detectInstalled(false)"
              >
                {{ detecting ? "检测中…" : installed ? "重新检测" : "立即检测" }}
              </button>
            </div>
            <div v-if="detecting" class="detect-line">
              正在虚拟机内检查已安装的组件，请保持手机亮屏…
            </div>
            <div v-else-if="installed" class="detect-grid">
              <span :class="installed.desktop ? 'ok' : 'miss'">
                桌面环境：{{ installed.desktop ? installed.desktopName : "未检测到" }}
              </span>
              <span :class="installed.apps ? 'ok' : 'miss'">
                常用软件：{{ installed.appsInstalled }}/{{ installed.appsTotal }}
              </span>
              <span :class="installed.ime ? 'ok' : 'miss'">
                中文输入法：{{ installed.ime ? "已安装" : "未安装" }}
              </span>
              <span :class="installed.vnc ? 'ok' : 'miss'">
                TigerVNC：{{ installed.vnc ? "已安装" : "未安装" }}
              </span>
            </div>
            <div v-else class="detect-line">
              已装过桌面可先检测，避免重复安装（只做只读检查，不会改动系统）。
            </div>
          </div>

          <div v-if="installed?.desktop && !done.install" class="detect-tip">
            已检测到 {{ installed.desktopName }} 桌面环境，无需重复安装。
            <button class="link" @click="skipToVnc()">前往远程桌面</button>
          </div>

          <div class="actions">
            <button class="btn primary" :disabled="busy || installRunning || !serial" @click="startInstall()">
              {{
                installRunning
                  ? "安装进行中…"
                  : installed?.desktop
                    ? "重新安装 / 补装组件"
                    : "开始安装桌面环境"
              }}
            </button>
          </div>
        </section>

        <!-- 步骤 6：VNC -->
        <section v-if="current === 5" class="card">
          <h2>远程桌面</h2>
          <p class="lead">
            VNC 服务运行在虚拟机内（5901 端口）。需要先在 Terminal 应用中放行该端口
            （新版：右上角 ⋮ → Port control；旧版：设置 → Port control），
            工具会自动完成这一步，然后建立 <code>adb forward</code> 转发并一键打开 TigerVNC。
          </p>
          <div v-if="portResult" class="kv">
            <div><span>端口放行</span><code>{{ portResult.message }}（{{ portResult.action }}）</code></div>
            <div><span>虚拟机监听</span><code>{{ portResult.active ? "已检测到" : "未检测到，稍后自动生效" }}</code></div>
          </div>
          <div v-if="vnc" class="kv">
            <div><span>端口转发</span><code>{{ vnc.forwardDetail }}</code></div>
            <div><span>客户端</span><code>{{ vnc.viewerPath || "未安装 TigerVNC Viewer" }}</code></div>
            <div><span>连接地址</span><code>{{ vnc.connectHint }}</code></div>
          </div>
          <p v-if="vncProgress" class="lead">{{ vncProgress }}</p>
          <div class="hintbox">
            <strong>手机端连接（AVNC）</strong>
            <ul>
              <li>地址：<code>127.0.0.1</code>　端口：<code>5901</code></li>
              <li>工具会自动下载并安装 AVNC 客户端，安装完成后在手机上打开即可连接。</li>
            </ul>
          </div>
          <div class="actions">
            <button class="btn primary" :disabled="busy || !serial" @click="enableVncPort()">
              放行端口并一键打开 TigerVNC
            </button>
            <button class="btn" :disabled="busy || !serial" @click="installAvnc()">
              {{ busy ? "正在安装 AVNC…" : "安装 AVNC（手机端）" }}
            </button>
            <button class="btn ghost" @click="api.openExternal('https://tigervnc.org/download/')">
              打开 TigerVNC 官网
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
