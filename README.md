# AndroidCreateLinuxGUITool

在 **Android 16+（支持 AVF 虚拟化框架）** 的手机上，一键装好 Debian 桌面环境（默认 XFCE），
并在 Windows 电脑上通过 TigerVNC 远程访问。全程图形界面引导，不需要在手机上手动敲命令。

- 技术栈：Rust + Tauri v2 + Vue 3 + Vite
- 目标设备：Pixel 等支持 AVF（Android Virtualization Framework）的 Android 16 及以上机型
- 虚拟机：Debian 12 / 13（由手机自带的 Linux Terminal 应用提供）

## 功能

| 步骤 | 内容 |
| --- | --- |
| 1. 环境准备 | 自动定位 `adb`，找不到时可手动指定 `adb.exe` 路径 |
| 2. 设备检测 | 检测已连接且已授权 USB 调试的设备，未连接时弹窗引导 |
| 3. 虚拟化检测 | 检查 `virtualization_framework` 特性与 `vm info`，不支持的机型直接提示 |
| 4. Linux 开发环境 | 在开发者选项中自动找到并打开「Linux 开发环境」开关（中英文界面），遇到企业策略 / 三星 Secure Folder 等限制时给出提示 |
| 5. 安装桌面环境 | 唤起 Linux Terminal，按需自动安装系统镜像，随后执行桌面安装脚本；支持中国用户切换国内镜像源，支持失败重试（脚本幂等） |
| 6. 远程桌面 | 放行 Terminal 的 VNC 端口、建立 `adb forward 5901`、自动下载并打开 TigerVNC；并可为手机自动下载安装 AVNC 客户端 |

安装过程中可以实时看到脚本输出（apt 日志、当前阶段、错误原因）。

## 环境要求

- Windows 10/11（TigerVNC 自动下载仅支持 Windows；其他系统需自行安装查看器）
- Android Platform Tools（`adb`），并确认 `adb devices` 能看到设备
- 手机：Android 16+，支持 AVF；已在开发者选项中开启 USB 调试
- 手机与电脑网络畅通，安装时手机保持**亮屏、解锁**（自动输入依赖软键盘，锁屏会导致输入被系统丢弃）

## 快速开始

直接运行发布版：

```text
src-tauri\target\release\android-create-linux-gui-tool.exe
```

然后按左侧 6 个步骤依次操作即可。

### 第 5 步说明

- 进入本步骤或点「立即检测」时，会先在虚拟机内做一次只读检查，显示已安装的**桌面环境 / 常用软件 / 中文输入法 / TigerVNC**；已装过会提示「无需重复安装」，可直接前往远程桌面，按钮会变成「重新安装 / 补装组件」。
- 首次使用若 Terminal 尚未安装系统镜像（约 600 MB），工具会自动点击安装并等待就绪。
- 会先询问是否为中国用户：选「是」将切换国内镜像源（TUNA）、设置中文语言与 Fcitx5 输入法；选「否」使用 Debian 官方源与英文环境。
- 可选桌面：XFCE（默认，约 1.2 GB）、GNOME、KDE Plasma、MATE、Cinnamon、LXQt、LXDE、GNOME Flashback。
- 进度通过共享目录日志回传（虚拟机内 `/mnt/shared` 对应手机存储根目录），日志文件为 `acl_install.log`。
- 安装脚本幂等，中途失败可以放心重试。

### 第 6 步说明

- 默认 VNC 密码 `linux`，显示号 `:1`（端口 5901），分辨率 `1920x1080`，SSH 端口 `10022`。
- 「放行端口并一键打开 TigerVNC」：自动完成 Terminal 端口放行、`adb forward tcp:5901`、启动本机 TigerVNC 并自动填入地址与密码。
- 本机没有 TigerVNC 时，会自动下载官方免安装版到 `%LOCALAPPDATA%\AndroidCreateLinuxGUITool\vncviewer.exe`。
- 「安装 AVNC（手机端）」：优先使用**程序内置的 AVNC 3.3.1 安装包**（不依赖网络，国内可用）安装到手机；内置包安装失败时才回退到 GitHub 下载最新版。连接参数为地址 `127.0.0.1`、端口 `5901`、用户名可留空（填 `droid` 也可）、密码为第 5 步设置的 VNC 密码（默认 `linux`）。

## 从源码构建

前置：Node.js 18+、Rust（stable）、Windows 上的 WebView2（Win10/11 自带）。
Tauri 在 Windows 上还需要 MSVC 生成工具与 Windows SDK。

```bash
# 安装前端依赖
npm install

# 开发模式（热更新）
npm run tauri:dev

# 发布构建
npm run build
cd src-tauri
cargo build --release --features custom-protocol
```

产物位于 `src-tauri/target/release/android-create-linux-gui-tool.exe`。

## 命令行参数

发布版 exe 同时提供诊断与无界面模式，便于排查问题：

| 参数 | 作用 |
| --- | --- |
| `--diagnose` / `--diagnose-json` | 输出设备、AVF、开发者选项的检测报告 |
| `--selftest` | 在真机上跑一遍核心链路（开启 Linux 环境、检测终端等） |
| `--install` | 无界面完整执行安装流程 |
| `--desktop=xfce` | 配合 `--install` 指定桌面环境 |
| `--official-source` | 配合 `--install` 使用 Debian 官方源与英文环境 |
| `--no-apps` | 配合 `--install` 跳过常用软件套件 |
| `--fetch-viewer` | 只下载 TigerVNC Viewer（免安装版） |
| `--port=5901` | 放行端口 + `adb forward` + 尝试打开 TigerVNC |
| `--exec="<命令>"` | 在虚拟机终端里执行一条命令并取回输出 |

示例：

```powershell
.\android-create-linux-gui-tool.exe --diagnose
.\android-create-linux-gui-tool.exe --exec="uname -a"
```

## 项目结构

```text
├─ src/                        # 前端（Vue 3）
│  ├─ App.vue                  # 6 步工作流主界面
│  ├─ api.ts                   # Tauri command / 事件封装
│  ├─ types.ts                 # 前后端共享类型
│  └─ components/              # 日志面板、模态弹窗
├─ src-tauri/
│  ├─ src/
│  │  ├─ adb.rs                # adb 封装（定位、shell、push、forward）
│  │  ├─ detect.rs             # 设备与 AVF 检测
│  │  ├─ devopts.rs            # 开发者选项 / Linux 开发环境开关
│  │  ├─ terminal.rs           # 驱动 Linux Terminal 应用（聚焦、输入命令）
│  │  ├─ installer.rs          # 安装流程编排与日志轮询
│  │  ├─ ports.rs              # VNC 端口放行
│  │  ├─ vnc.rs                # TigerVNC 下载/启动、AVNC 安装、adb forward
│  │  ├─ commands.rs           # 暴露给前端的 Tauri 命令
│  │  └─ lib.rs / main.rs      # 应用入口与命令行模式
│  └─ resources/
│     └─ install-desktop.sh    # 在虚拟机内执行的桌面安装脚本
└─ shots/shot.ps1              # 截图辅助脚本
```

## 常见问题

**卡在「命令似乎没有在终端中执行，正在重新聚焦后重试」**
自动输入依赖手机当前是否弹出软键盘。请让手机保持亮屏解锁，必要时手动点一下 Terminal 的命令行区域，再点「重试」。

**VNC 连上了但桌面黑屏**
脚本已通过 `~/.xsessionrc` 强制 X11（虚拟机内的 Wayland 合成器不支持 layer-shell）。若仍是黑屏，在终端执行 `bash /mnt/shared/acl_install.sh` 重跑一次即可（脚本幂等）。

**点击左侧「远程桌面」没有反应**
已修复：只要上一步完成即可进入下一步。请使用最新构建的 exe。

**TigerVNC 打不开 / 未检测到**
可点「打开 TigerVNC 官网」手动安装；或运行 `--fetch-viewer` 下载免安装版。

**端口转发失败**
确认手机与电脑的 adb 连接正常，重新插拔 USB 后再试；也可在手机上打开 Terminal → 右上角 ⋮ → Settings → Security，在「Listening ports」打开 5901 的开关（列表中没有时点右下角「+」添加）。

## 免责声明

本工具会通过 adb 修改开发者选项并重启虚拟机内的服务，请在了解风险的前提下使用；
安装脚本会占用约 2 GB 以上手机存储，并需要较长时间下载依赖包。
