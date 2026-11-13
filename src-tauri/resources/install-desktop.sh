#!/bin/bash
# =============================================================================
#  AndroidCreateLinuxGUITool —— Android 16 (AVF) Linux 桌面环境一键安装
#  ---------------------------------------------------------------------
#  本文件由桌面工具生成（配置区由工具注入），推送到手机共享存储后，
#  在 AVF 虚拟机（Debian）内通过 `bash <path>` 非交互执行。
#
#  与原始交互版脚本的差异：
#    * 全部交互项改为配置区变量；
#    * 输出同时写入共享存储日志文件，供 PC 端实时读取进度；
#    * 支持 Debian 12 (bookworm) / Debian 13 (trixie)；
#    * 支持切换到国内镜像源；
#    * 失败时输出 @@RESULT:FAIL:<原因> 供工具弹窗提示。
# =============================================================================

set -uo pipefail

# ============================== 配置区 ==============================
DESKTOP="xfce"
CN_MIRROR="1"
SYS_LOCALE="zh_CN.UTF-8"
VNC_PASSWORD="linux"
VNC_DISPLAY=":1"
VNC_GEOMETRY="1920x1080"
INSTALL_APPS="1"
INSTALL_IME="1"
SSH_PORT="10022"
ACL_LOG_FILE="/mnt/shared/acl_install.log"
# ============================ 配置区结束 ============================

# ---------------------------- 日志初始化 ----------------------------
LOG_FILE="$ACL_LOG_FILE"
if ! { mkdir -p "$(dirname "$LOG_FILE")" 2>/dev/null && touch "$LOG_FILE" 2>/dev/null; }; then
    LOG_FILE="/tmp/acl_install.log"
    : > "$LOG_FILE" 2>/dev/null
fi
# 每次运行都重新开始写日志：否则上一轮的 @@RESULT:OK 会被 PC 端误读成本轮的成功。
: > "$LOG_FILE" 2>/dev/null || true
export ACL_LOG_FILE="$LOG_FILE"
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

# 所有 stdout/stderr 同时进入日志文件
exec > >(tee -a "$LOG_FILE") 2>&1

marker() { printf '@@%s\n' "$*"; }
step()   { marker "STEP:$1"; }
# 退出前留一点时间，确保 tee 把最后几行刷进日志文件
fail()   { marker "RESULT:FAIL:$1"; sleep 0.4; exit 1; }

trap 'marker "RESULT:FAIL:脚本被中断（终端可能被关闭）"; exit 130' INT TERM

# ---------------------------- 环境准备 ----------------------------
TARGET_USER="$(id -un 2>/dev/null || echo droid)"
# sudo 默认 env_reset 会清空环境变量，脚本里 export 的 DEBIAN_FRONTEND
# 传不进 apt/dpkg，于是 debconf 又会弹出「Package configuration」对话框
# （典型是 keyboard-configuration 选键盘布局），整个安装看起来就"死"在
# 那里、按回车也没反应。这里用 env 显式透传，保证全程非交互。
SUDO_ENV="env DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a APT_LISTCHANGES_FRONTEND=none"
if [ "$(id -u)" -eq 0 ]; then
    SUDO=""
elif sudo -n true 2>/dev/null; then
    SUDO="sudo -n $SUDO_ENV"
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo $SUDO_ENV"
else
    fail "虚拟机内没有 sudo，无法自动安装。"
fi
if [ -n "$SUDO" ] && ! $SUDO true 2>/dev/null; then
    fail "当前账户 ($TARGET_USER) 的 sudo 需要交互输入密码，无法自动安装。请先在终端中执行 sudo -v 设置好免密或缓存凭据后重试。"
fi
# 预置 debconf 答案，双保险：即使某个 ROM/发行版的 debconf 仍然冒出来，
# 也直接采用默认值，不会停下来等人按键。
printf '%s\n' \
    'keyboard-configuration keyboard-configuration/layoutcode string us' \
    'keyboard-configuration keyboard-configuration/modelcode string pc105' \
    'keyboard-configuration keyboard-configuration/xkb-keymap select us' \
    'console-setup console-setup/charmap47 select UTF-8' \
    'console-setup console-setup/codeset47 select Guess optimal character set' \
    | $SUDO debconf-set-selections 2>/dev/null || true
# 以普通用户身份启动 VNC（root 启动会生成错误的权限文件）
if [ "$(id -u)" -eq 0 ] && command -v runuser > /dev/null 2>&1; then
    RUNAS="runuser -u $TARGET_USER --"
elif [ "$(id -u)" -eq 0 ] && command -v su > /dev/null 2>&1; then
    RUNAS="su $TARGET_USER -s /bin/bash -c"
else
    RUNAS=""
fi
VNC_CMD="vncserver"
command -v vncserver > /dev/null 2>&1 || VNC_CMD="tigervncserver"

# ---------------------------- 阶段选择 ----------------------------
# 安装流程被拆成 4 个独立阶段，由工具分批调用（也便于失败后单独重跑）：
#   base    —— 系统基础：dpkg 修复、镜像源、软件包更新、中文语言、SSH、共享目录
#   desktop —— 安装桌面环境
#   apps    —— 常用软件与中文输入法
#   vncsvc  —— TigerVNC 服务（安装 / 密码 / 启动）
#   all     —— 依次执行全部阶段（未传参数时的旧行为）
STAGE="${1:-all}"
case "$STAGE" in
    all|base|desktop|apps|vncsvc) ;;
    *) fail "未知的安装阶段: $STAGE（可选 base / desktop / apps / vncsvc / all）" ;;
esac
stage_on() {
    [ "$STAGE" = "all" ] || [ "$STAGE" = "$1" ]
}

# 桌面标识在 desktop / vncsvc 两个阶段都要用，提前解析
case "$DESKTOP" in
    kde)       DESKTOP_NAME="KDE Plasma";     SESSION_WANT="plasmax11";               SESSION_KEY="plasma";    TASK_NAME="kde-desktop";              TASK_PACKAGES="kde-plasma-desktop plasma-workspace" ;;
    gnome)     DESKTOP_NAME="GNOME";           SESSION_WANT="gnome-xorg";               SESSION_KEY="gnome";     TASK_NAME="gnome-desktop";            TASK_PACKAGES="gnome-session gnome-session-xsession gnome-shell" ;;
    xfce)      DESKTOP_NAME="XFCE";            SESSION_WANT="xfce";                     SESSION_KEY="xfce";      TASK_NAME="xfce-desktop";             TASK_PACKAGES="xfce4 xfce4-terminal" ;;
    mate)      DESKTOP_NAME="MATE";            SESSION_WANT="mate";                     SESSION_KEY="mate";      TASK_NAME="mate-desktop";             TASK_PACKAGES="mate-desktop-environment mate-session-manager" ;;
    cinnamon)  DESKTOP_NAME="Cinnamon";        SESSION_WANT="cinnamon";                 SESSION_KEY="cinnamon";  TASK_NAME="cinnamon-desktop";         TASK_PACKAGES="cinnamon cinnamon-session cinnamon-common" ;;
    lxqt)      DESKTOP_NAME="LXQt";            SESSION_WANT="lxqt";                     SESSION_KEY="lxqt";      TASK_NAME="lxqt-desktop";             TASK_PACKAGES="lxqt lxqt-session" ;;
    lxde)      DESKTOP_NAME="LXDE";            SESSION_WANT="lxde";                     SESSION_KEY="lxde";      TASK_NAME="lxde-desktop";             TASK_PACKAGES="lxde lxde-session" ;;
    flashback) DESKTOP_NAME="GNOME Flashback"; SESSION_WANT="gnome-flashback-metacity"; SESSION_KEY="flashback"; TASK_NAME="gnome-flashback-desktop";  TASK_PACKAGES="gnome-flashback gnome-session-xsession" ;;
    *)         fail "未知的桌面环境类型: $DESKTOP" ;;
esac
# 兜底直接用 Xtigervnc 启动时，xstartup 里要执行的会话命令
case "$DESKTOP" in
    kde)       SESSION_CMD="startplasma-x11" ;;
    gnome)     SESSION_CMD="gnome-session" ;;
    xfce)      SESSION_CMD="startxfce4" ;;
    mate)      SESSION_CMD="mate-session" ;;
    cinnamon)  SESSION_CMD="cinnamon-session" ;;
    lxqt)      SESSION_CMD="startlxqt" ;;
    lxde)      SESSION_CMD="startlxde" ;;
    flashback) SESSION_CMD="gnome-session --session=gnome-flashback-metacity" ;;
esac
VNC_PORT=$(( 5900 + ${VNC_DISPLAY#:} ))
# apt/dpkg 的通用参数，所有阶段（base/desktop/apps/vncsvc）都会用到，
# 必须放在阶段判断之外，否则单独跑 desktop 等阶段会因 set -u 报
# "APT_OPTS: 未绑定的变量"。
APT_OPTS="-o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold"

# ============================ 阶段：系统基础 ============================
if stage_on base; then
step "environment"
echo "=== Android Linux 桌面环境安装 ==="
echo "日志文件: $LOG_FILE"
echo "用户: $TARGET_USER  (sudo='${SUDO:-none}')"
echo "阶段: $STAGE"

# ---------------------------- 系统识别 ----------------------------
step "detect_system"
DEB_RAW="$(cat /etc/debian_version 2>/dev/null | tr -d '[:space:]')"
DEB_VER="$(printf '%s' "$DEB_RAW" | cut -d. -f1 | tr -dc '0-9')"

# 识别 Debian 套件。不同设备的镜像版本不一样（Pixel 的 AVF 镜像是 Debian 13，
# Redmi K Pad 是 Debian 12），套件选错会让仓库版本和已装软件对不上，apt 直接报
#   E: The package perl-base needs to be reinstalled, but I can't find an archive for it.
# 注意：测试/滚动镜像的 /etc/debian_version 可能写成 `trixie/sid`，里面没有数字，
# 所以不能只看版本号，要按名字兜底。
detect_suite() {
    local name=""
    if [ -r /etc/os-release ]; then
        name="$(. /etc/os-release 2>/dev/null; printf '%s' "${VERSION_CODENAME:-}")"
    fi
    if [ -z "$name" ]; then
        case "$DEB_RAW" in
            12*)                          name=bookworm ;;
            13*)                          name=trixie   ;;
            *bullseye*)                   name=bullseye ;;
            *bookworm*)                   name=bookworm ;;
            *trixie*|*forky*|*sid*|*testing*) name=trixie ;;
        esac
    fi
    if [ -z "$name" ]; then
        name="$(lsb_release -cs 2>/dev/null || true)"
    fi
    if [ -z "$name" ]; then
        case "$DEB_VER" in
            11) name=bullseye ;;
            12) name=bookworm ;;
            13) name=trixie   ;;
        esac
    fi
    printf '%s' "${name:-bookworm}"
}
SUITE="$(detect_suite)"
# bullseye 及更早没有 non-free-firmware 组件，带上会让 apt update 直接失败
case "$SUITE" in
    bullseye|buster|stretch) COMPONENTS="main contrib non-free" ;;
    *)                       COMPONENTS="main contrib non-free non-free-firmware" ;;
esac
echo "系统: Debian ${DEB_VER:-未知} (${DEB_RAW:-?}, suite=$SUITE)"
echo "架构: $(uname -m)   内核: $(uname -r)"

# ---------------------- 修复上次中断留下的 dpkg 状态 ----------------------
# 终端应用闪退 / 安装被取消会把 dpkg 留在"被中断"状态，之后所有 apt 命令
# 都会直接报：
#   E: dpkg was interrupted, you must manually run
#      'sudo dpkg --configure -a' to correct the problem.
# dpkg --configure -a 在状态正常时是空操作，所以这里无条件跑一遍，
# 让"取消后重新执行"能自动续上，而不是卡死在这里。
step "dpkg_repair"
echo "正在检查上次安装是否留下了未配置的软件包..."
# 若上一次的 apt/dpkg 进程还活着，先等它退出，避免抢锁
for _ in $(seq 1 30); do
    if $SUDO pgrep -x dpkg > /dev/null 2>&1 || $SUDO pgrep -x apt-get > /dev/null 2>&1; then
        sleep 1
    else
        break
    fi
done
# 闪退后经常残留锁文件，清掉（此时已确认没有 dpkg/apt-get 在运行）
$SUDO rm -f /var/lib/dpkg/lock-frontend /var/lib/dpkg/lock \
    /var/cache/apt/archives/lock /var/lib/apt/lists/lock 2>/dev/null || true
if ! $SUDO dpkg --configure -a; then
    echo "[提示] dpkg --configure -a 未完全成功，尝试 apt-get -f install 修复..."
    $SUDO apt-get $APT_OPTS -f install -y || true
    $SUDO dpkg --configure -a || true
fi
echo "dpkg 状态检查完成。"

# ---------------------------- 切换国内源 ----------------------------
# 写好 sources.list + deb822 的 debian.sources（套件由 detect_suite 决定）
write_cn_sources() {
    local suite="$1"
    local comps="main contrib non-free"
    case "$COMPONENTS" in
        *non-free-firmware*) comps="main contrib non-free non-free-firmware" ;;
    esac
    DEB_CN="https://mirrors.tuna.tsinghua.edu.cn/debian"
    SEC_CN="https://mirrors.tuna.tsinghua.edu.cn/debian-security"
    $SUDO cp -n /etc/apt/sources.list /etc/apt/sources.list.bak.acl 2>/dev/null || true
    # 备份不要放在 sources.list.d/ 里：apt 会对非 .list/.sources 后缀的文件报
    # "Ignoring file ... as it has an invalid filename extension"
    $SUDO cp -n /etc/apt/sources.list.d/debian.sources /etc/apt/debian.sources.bak.acl 2>/dev/null || true

    # deb822 格式（Debian 12/13 默认）
    $SUDO tee /etc/apt/sources.list.d/debian.sources > /dev/null <<EOF
Types: deb
URIs: $DEB_CN
Suites: $suite $suite-updates
Components: $COMPONENTS
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg

Types: deb
URIs: $SEC_CN
Suites: $suite-security
Components: $COMPONENTS
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
EOF

    # 经典格式（兼容老镜像）
    $SUDO tee /etc/apt/sources.list > /dev/null <<EOF
deb $DEB_CN $suite main contrib non-free${comps##* non-free}
deb $DEB_CN $suite-updates main contrib non-free${comps##* non-free}
deb $SEC_CN $suite-security main contrib non-free${comps##* non-free}
EOF
}

if [ "$CN_MIRROR" = "1" ]; then
    step "mirror"
    echo "正在切换到国内镜像源 (清华 TUNA, $SUITE)..."
    write_cn_sources "$SUITE"
    echo "国内源已生效: $DEB_CN ($SUITE)"
fi

# ---------------------------- 更新软件包 ----------------------------
step "apt_update"
echo "正在更新软件包列表..."
$SUDO apt-get $APT_OPTS update -y || fail "apt-get update 失败。请检查虚拟机网络连通性，或临时关闭「使用国内源」后重试。"

# 套件和镜像不匹配时，已安装软件在仓库里找不到对应版本，apt 会报：
#   E: The package perl-base needs to be reinstalled, but I can't find an archive for it.
# 用 perl-base 当探针：候选版本为空说明套件选错了，换另一个套件再更新一次。
if [ "$CN_MIRROR" = "1" ] && apt-cache policy perl-base 2>/dev/null | grep -q 'Candidate: (none)'; then
    case "$SUITE" in
        trixie) ALT_SUITE=bookworm ;;
        *)      ALT_SUITE=trixie   ;;
    esac
    echo "[提示] $SUITE 仓库里找不到已安装的 perl-base，套件可能选错了，改用 $ALT_SUITE 重试..."
    SUITE="$ALT_SUITE"
    write_cn_sources "$SUITE"
    echo "国内源已切换: $DEB_CN ($SUITE)"
    $SUDO apt-get $APT_OPTS update -y || fail "apt-get update 失败（套件 $SUITE）。"
fi

# dpkg 里被标记为"需要重装"的包（状态 iR）会挡住后续所有 install/upgrade
REINST="$(dpkg -l 2>/dev/null | awk '$1 ~ /^iR/ {print $2}' | tr '\n' ' ')"
if [ -n "$REINST" ]; then
    echo "检测到需要重装的软件包: $REINST"
    $SUDO apt-get $APT_OPTS install -y --reinstall $REINST || echo "[警告] 重装 $REINST 未完全成功，继续执行。"
fi

echo "正在升级已安装的软件包..."
$SUDO apt-get $APT_OPTS upgrade -y || echo "[警告] apt-get upgrade 未完全成功，继续执行。"

# ---------------------------- 语言与字体 ----------------------------
step "locale"
echo "正在配置系统语言 ($SYS_LOCALE) 与中文字体..."
$SUDO apt-get $APT_OPTS install -y locales || fail "安装 locales 失败。"
$SUDO sed -i 's/^# *\(zh_CN.UTF-8 UTF-8\)/\1/' /etc/locale.gen 2>/dev/null || true
$SUDO sed -i 's/^# *\(en_US.UTF-8 UTF-8\)/\1/' /etc/locale.gen 2>/dev/null || true
$SUDO locale-gen || fail "生成 locale 失败。"
$SUDO update-locale LANG="$SYS_LOCALE" 2>/dev/null || true
if ! grep -q "export LANG=" "$HOME/.bashrc" 2>/dev/null; then
    echo "export LANG=$SYS_LOCALE" >> "$HOME/.bashrc"
    echo "export LC_ALL=$SYS_LOCALE" >> "$HOME/.bashrc"
else
    sed -i "s/export LANG=.*/export LANG=$SYS_LOCALE/" "$HOME/.bashrc"
    sed -i "s/export LC_ALL=.*/export LC_ALL=$SYS_LOCALE/" "$HOME/.bashrc"
fi
$SUDO apt-get $APT_OPTS install -y fonts-noto-cjk fonts-wqy-zenhei fonts-wqy-microhei || echo "[警告] 中文字体安装未完全成功。"

# --------------------- Android 虚拟机网络桥接稳定性补丁 ---------------------
step "network_patch"
echo "正在植入 Android 虚拟机网络桥接补丁..."
$SUDO mkdir -p /etc/systemd/system/ssh.service.d /etc/systemd/system/tigervncserver@.service.d
$SUDO tee /etc/systemd/system/ssh.service.d/wait-network.conf > /dev/null <<'EOF'
[Unit]
After=network-online.target
Wants=network-online.target
EOF
$SUDO tee /etc/systemd/system/tigervncserver@.service.d/wait-network.conf > /dev/null <<'EOF'
[Unit]
After=network-online.target
Wants=network-online.target
EOF
$SUDO systemctl daemon-reload 2>/dev/null || true

# ---------------------------- SSH 配置 ----------------------------
step "ssh"
echo "正在配置 SSH 服务 (端口 $SSH_PORT)..."
$SUDO apt-get $APT_OPTS install -y openssh-server || fail "安装 openssh-server 失败。"
$SUDO sed -i -E -e "s/^#?\\s*Port\\s+[0-9]+/Port $SSH_PORT/" -e 's/^#?\s*PasswordAuthentication\s+no/PasswordAuthentication yes/' /etc/ssh/sshd_config 2>/dev/null || true
$SUDO grep -q "^Port $SSH_PORT" /etc/ssh/sshd_config 2>/dev/null || echo "Port $SSH_PORT" | $SUDO tee -a /etc/ssh/sshd_config > /dev/null
$SUDO grep -q "^PasswordAuthentication yes" /etc/ssh/sshd_config 2>/dev/null || echo "PasswordAuthentication yes" | $SUDO tee -a /etc/ssh/sshd_config > /dev/null
$SUDO grep -q "^ListenAddress 0.0.0.0" /etc/ssh/sshd_config 2>/dev/null || echo "ListenAddress 0.0.0.0" | $SUDO tee -a /etc/ssh/sshd_config > /dev/null
if systemctl list-unit-files 2>/dev/null | grep -q "^ssh.service"; then
    $SUDO systemctl enable ssh 2>/dev/null || true
    $SUDO systemctl restart ssh || $SUDO service ssh restart || echo "[警告] SSH 服务重启失败。"
else
    $SUDO systemctl enable sshd 2>/dev/null || true
    $SUDO systemctl restart sshd || $SUDO service sshd restart || echo "[警告] SSH 服务重启失败。"
fi
fi  # 阶段 base 结束

# ============================ 阶段：桌面环境 ============================
if stage_on desktop; then
step "desktop"
# 注意：Debian 12/13 的仓库里**没有** `xfce-desktop` 这类 tasksel 任务元包
# （`apt-cache show xfce-desktop` 查不到），所以必须直接安装真实的软件包；
# tasksel 只在任务包确实存在时才用。
echo "正在安装 $DESKTOP_NAME 桌面环境，这可能需要较长时间..."
# 任务包在仓库里存在时才走 tasksel，否则直接装真实软件包
if apt-cache show "$TASK_NAME" > /dev/null 2>&1; then
    $SUDO apt-get $APT_OPTS install -y tasksel || echo "[警告] 安装 tasksel 失败。"
    if ! $SUDO tasksel install "$TASK_NAME" < /dev/null; then
        echo "[提示] tasksel 安装未完全成功，改用直接安装任务包..."
    fi
fi
$SUDO apt-get $APT_OPTS install -y $TASK_PACKAGES || fail "安装 $DESKTOP_NAME 桌面环境失败（软件包: $TASK_PACKAGES）。"

fi  # 阶段 desktop 结束

# ============================ 阶段：TigerVNC 服务 ============================
if stage_on vncsvc; then
step "vnc"
echo "正在配置 TigerVNC 远程桌面..."
$SUDO apt-get $APT_OPTS install -y tigervnc-standalone-server tigervnc-common dbus-x11 x11-xserver-utils || fail "安装 TigerVNC 失败。"
$SUDO mkdir -p /tmp/.X11-unix
$SUDO chmod 1777 /tmp/.X11-unix 2>/dev/null || true
mkdir -p "$HOME/.vnc"

# 探测实际可用的 X 会话（以系统里真实存在的会话文件为准）
ACTUAL_SESSION=""
if [ -f "/usr/share/xsessions/${SESSION_WANT}.desktop" ]; then
    ACTUAL_SESSION="$SESSION_WANT"
else
    for f in /usr/share/xsessions/*.desktop; do
        [ -e "$f" ] || continue
        b="$(basename "$f" .desktop)"
        case "$b" in
            *"$SESSION_KEY"*) ACTUAL_SESSION="$b"; break ;;
        esac
    done
fi
if [ -z "$ACTUAL_SESSION" ]; then
    # 所选桌面的会话文件不存在时，退回系统里第一个可用的会话。
    # 常见于之前装的是别的桌面（例如已装 Cinnamon，这次选项却是 XFCE），
    # 没必要因为选项不一致就判定失败。
    for f in /usr/share/xsessions/*.desktop; do
        [ -e "$f" ] || continue
        ACTUAL_SESSION="$(basename "$f" .desktop)"
        break
    done
fi
if [ -z "$ACTUAL_SESSION" ]; then
    echo "可用的 X 会话: $(ls /usr/share/xsessions/ 2>/dev/null | tr '\n' ' ')"
    fail "桌面环境已安装，但没有找到可用的 X 会话文件（/usr/share/xsessions）。请在左侧重新选择已安装的桌面类型后重试。"
fi
# 会话文件里的 Exec 才是真正要执行的命令，以实际安装的会话为准
ACTUAL_EXEC="$(sed -n 's/^Exec=//p' "/usr/share/xsessions/$ACTUAL_SESSION.desktop" 2>/dev/null | head -n1 | sed 's/ *%[fFuU]//g')"
if [ -n "${ACTUAL_EXEC:-}" ]; then
    SESSION_CMD="$ACTUAL_EXEC"
fi
if [ "$ACTUAL_SESSION" != "$SESSION_WANT" ]; then
    echo "[提示] 所选桌面（$DESKTOP_NAME）的会话文件不存在，改用系统里已安装的会话: $ACTUAL_SESSION"
fi
echo "使用会话: $ACTUAL_SESSION"

# ---------------------- VNC 会话必须走 X11 后端 ----------------------
# 虚拟机里存在 Terminal 自带的 Wayland 合成器（/run/user/*/wayland-0）。
# GTK 会优先选择 Wayland 后端，但那个合成器不支持 layer-shell 协议，
# 于是 xfdesktop / xfce4-panel 启动即失败 —— 表现就是 VNC 连上后整屏黑色。
# ~/.xsessionrc 会被 Debian 的 /etc/X11/Xsession 读取，在这里强制 X11。
cat > "$HOME/.xsessionrc" <<'EOF'
unset WAYLAND_DISPLAY
export XDG_SESSION_TYPE=x11
export GDK_BACKEND=x11
export QT_QPA_PLATFORM=xcb
export CLUTTER_BACKEND=x11
EOF
chmod 644 "$HOME/.xsessionrc" 2>/dev/null || true
echo "已强制 VNC 会话使用 X11（避免 Wayland 合成器导致黑屏）。"

if [ -n "$VNC_PASSWORD" ]; then
    echo "正在设置 VNC 密码..."
    # Debian 13 的 TigerVNC 使用 XDG 路径（~/.config/tigervnc/），
    # 旧版本用 ~/.vnc/；两个位置都写，避免版本差异。
    mkdir -p "$HOME/.vnc" "$HOME/.config/tigervnc"
    command -v vncpasswd > /dev/null 2>&1 || fail "找不到 vncpasswd，无法设置 VNC 密码。"
    for PW_DIR in "$HOME/.vnc" "$HOME/.config/tigervnc"; do
        PW_FILE="$PW_DIR/passwd"
        # 必须用 -f：从 stdin 读密码，把混淆后的 8 字节直接写到 stdout。
        # 交互模式要求密码 ≥6 字符，还会追问一次 view-only 密码；非交互执行时
        # 会一直重试到 EOF 并退出，旧文件被原样保留 —— 服务端解出的密码就是错的，
        # 表现就是客户端"认证失败"。
        rm -f "$PW_FILE"
        printf '%s\n' "$VNC_PASSWORD" | vncpasswd -f > "$PW_FILE" 2>/dev/null || true
        if [ "$(wc -c < "$PW_FILE" 2>/dev/null | tr -d ' ')" != "8" ]; then
            rm -f "$PW_FILE"
            fail "生成 VNC 密码文件失败（vncpasswd -f 未输出 8 字节）。"
        fi
        # TigerVNC 强制要求密码文件权限为 0600，否则拒绝启动
        chmod 600 "$PW_FILE" 2>/dev/null || true
    done
    echo "VNC 密码已设置。"
fi

mkdir -p "$HOME/.vnc" "$HOME/.config/tigervnc"
cat > "$HOME/.vnc/config" <<EOF
session=$ACTUAL_SESSION
geometry=$VNC_GEOMETRY
localhost=no
alwaysshared
# 只提供 VncAuth：否则服务端还会广播 VeNCrypt(19)，部分客户端会先尝试
# VeNCrypt 并用用户名+密码的方式认证，直接报"认证失败"。
SecurityTypes=VncAuth
# 必须显式指定密码文件：systemd 单元 / vncserver 包装脚本在部分版本里
# 不会自动带 -rfbauth，结果服务端报 "No password configured for VNC Auth"。
passwordfile=$HOME/.vnc/passwd
EOF
cp "$HOME/.vnc/config" "$HOME/.config/tigervnc/config" 2>/dev/null || true

$SUDO mkdir -p /etc/tigervnc
echo "${VNC_DISPLAY}=${TARGET_USER}" | $SUDO tee /etc/tigervnc/vncserver.users > /dev/null

$SUDO systemctl daemon-reload 2>/dev/null || true
# 不同发行版的 unit 名字不一样（tigervncserver@ / vncserver@），逐个尝试
VNC_UNIT=""
for u in "tigervncserver@${VNC_DISPLAY}.service" "vncserver@${VNC_DISPLAY}.service" "tigervncserver@${VNC_DISPLAY#:}.service"; do
    if systemctl list-unit-files 2>/dev/null | grep -q "^${u}"; then
        VNC_UNIT="$u"
        break
    fi
done
if [ -z "$VNC_UNIT" ] && systemctl list-unit-files 2>/dev/null | grep -q "tigervncserver@"; then
    VNC_UNIT="tigervncserver@${VNC_DISPLAY}.service"
fi
echo "VNC 服务单元: ${VNC_UNIT:-（未找到，改用直接启动）}"
[ -n "$VNC_UNIT" ] && $SUDO systemctl enable "$VNC_UNIT" 2>/dev/null || true
VNC_OK=0
VNC_START_LOG="$HOME/.vnc/acl-vnc-start.log"
: > "$VNC_START_LOG" 2>/dev/null || true
vnc_port_listening() {
    (ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -q ":${VNC_PORT}\b"
}
# 窗口管理器是否已接管会话。VNC 端口监听 != 桌面正常：
# 没有 WM 时画面只有 nemo-desktop 的图标，没有面板/任务栏。
vnc_wm_running() {
    pgrep -u "$TARGET_USER" -x xfwm4 > /dev/null 2>&1 && return 0
    pgrep -u "$TARGET_USER" -x cinnamon > /dev/null 2>&1 && return 0
    pgrep -u "$TARGET_USER" -x muffin > /dev/null 2>&1 && return 0
    return 1
}
# 上一轮会话残留的进程不会随 Xvnc 一起退出：
#   * 残留的 X 客户端会让新 Xvnc 报 "X server already running"；
#   * 残留的窗口管理器（Cinnamon 的 cinnamon/muffin 等）仍占着 WM 身份，
#     新会话里的 Cinnamon 会报
#     "Failed to create backend: Could not take control ... EBUSY"，
#     结果是黑屏 + 只有桌面图标 + 没有面板。
# 所以重启前必须把它们真正杀干净，并等到进程退出。
vnc_kill_stale_session() {
    pkill -u "$TARGET_USER" -f 'xfce4-session|xfce4-panel|xfdesktop|xfwm4|xfsettingsd|xfce4-power-manager' 2>/dev/null || true
    pkill -u "$TARGET_USER" -f 'cinnamon-session|muffin|nemo-desktop|cinnamon|csd-|tigervncconfig' 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        pgrep -u "$TARGET_USER" -f 'xfwm4|cinnamon-session|muffin|cinnamon --replace' > /dev/null 2>&1 || break
        sleep 1
    done
    sleep 2
}
# 清掉上一次遗留的 X socket / lock 文件（仅在确认没有 Xtigervnc 运行时）
vnc_clear_x_locks() {
    if ! pgrep -f "Xtigervnc ${VNC_DISPLAY}" > /dev/null 2>&1; then
        $SUDO rm -f "/tmp/.X${VNC_DISPLAY#:}-lock" "/tmp/.X11-unix/X${VNC_DISPLAY#:}" 2>/dev/null || true
    fi
}
vnc_wait_port() {
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
        vnc_port_listening && return 0
        sleep 1
    done
    return 1
}
vnc_wait_wm() {
    for _ in $(seq 1 20); do
        vnc_wm_running && return 0
        sleep 1
    done
    return 1
}
# 服务端是否真的加载了密码文件。没有 -rfbauth / -PasswordFile 时，
# 客户端会直接报 "No password configured for VNC Auth"。
vnc_auth_configured() {
    pgrep -u "$TARGET_USER" -af Xtigervnc 2>/dev/null | grep -qi -- '-rfbauth\|-passwordfile'
}
vnc_start_via_unit() {
    [ -n "$VNC_UNIT" ] || return 1
    vnc_kill_stale_session
    $SUDO systemctl stop "$VNC_UNIT" 2>/dev/null || true
    vnc_clear_x_locks
    if ! $SUDO systemctl start "$VNC_UNIT" 2>/dev/null; then
        echo "[提示] systemd 单元启动失败，单元日志："
        $SUDO journalctl -u "$VNC_UNIT" -n 15 --no-pager 2>/dev/null | sed 's/^/    /'
        return 1
    fi
    vnc_wait_port
}
vnc_start_direct() {
    $RUNAS $VNC_CMD -kill "$VNC_DISPLAY" > /dev/null 2>&1 || true
    vnc_kill_stale_session
    vnc_clear_x_locks
    # 输出不再丢进 /dev/null：启动失败时这些内容就是唯一的线索
    if $RUNAS $VNC_CMD "$VNC_DISPLAY" -geometry "$VNC_GEOMETRY" -localhost no \
        > "$VNC_START_LOG" 2>&1 && vnc_wait_port; then
        return 0
    fi
    echo "[提示] $VNC_CMD 没有监听 ${VNC_PORT}，最近输出："
    tail -15 "$VNC_START_LOG" 2>/dev/null | sed 's/^/    /'
    return 1
}
# 生成兜底用的 xstartup：绕过 vncserver 包装脚本和 systemd，
# 直接让 Xtigervnc 执行桌面会话。
write_xstartup() {
    cat > "$HOME/.vnc/xstartup" <<EOF
#!/bin/sh
unset SESSION_MANAGER
unset DBUS_SESSION_BUS_ADDRESS
unset WAYLAND_DISPLAY
export XDG_SESSION_TYPE=x11
export GDK_BACKEND=x11
export QT_QPA_PLATFORM=xcb
[ -r "\$HOME/.xsessionrc" ] && . "\$HOME/.xsessionrc"
exec dbus-launch --exit-with-session $SESSION_CMD
EOF
    chmod 755 "$HOME/.vnc/xstartup"
}
vnc_start_raw() {
    command -v Xtigervnc > /dev/null 2>&1 || return 1
    write_xstartup
    local auth="$HOME/.config/tigervnc/passwd"
    [ -f "$auth" ] || auth="$HOME/.vnc/passwd"
    [ -f "$auth" ] || return 1
    $RUNAS $VNC_CMD -kill "$VNC_DISPLAY" > /dev/null 2>&1 || true
    pkill -u "$TARGET_USER" -f "Xtigervnc ${VNC_DISPLAY}" 2>/dev/null || true
    vnc_kill_stale_session
    vnc_clear_x_locks
    # setsid + nohup：脱离当前终端会话。否则安装脚本一结束，Xtigervnc
    # 可能随 Terminal 会话被一起清理，表现为"刚启动就掉线"。
    $RUNAS setsid nohup Xtigervnc "$VNC_DISPLAY" -geometry "$VNC_GEOMETRY" -depth 24 \
        -rfbport "$VNC_PORT" -rfbauth "$auth" -SecurityTypes VncAuth \
        -localhost=0 -AlwaysShared -xstartup "$HOME/.vnc/xstartup" \
        > "$VNC_START_LOG" 2>&1 < /dev/null &
    # 等端口就绪后再观察 2 秒，确认进程没有启动后立刻退出
    if vnc_wait_port && sleep 2 && vnc_port_listening; then
        return 0
    fi
    echo "[提示] Xtigervnc 直接启动也没有监听 ${VNC_PORT}，最近输出："
    tail -15 "$VNC_START_LOG" 2>/dev/null | sed 's/^/    /'
    return 1
}
if vnc_start_via_unit; then
    VNC_OK=1
else
    echo "[提示] systemd 方式未能监听 ${VNC_PORT}，改用 $VNC_CMD 直接启动..."
    if vnc_start_direct; then
        VNC_OK=1
    else
        echo "[提示] $VNC_CMD 方式失败，改用 Xtigervnc 直接启动..."
        vnc_start_raw && VNC_OK=1
    fi
fi
# 端口起来了但密码没生效（客户端报 "No password configured for VNC Auth"）
# → 用带 -rfbauth 的方式重启一次，保证密码认证可用。
if [ "$VNC_OK" = "1" ] && [ -n "$VNC_PASSWORD" ] && ! vnc_auth_configured; then
    echo "[提示] 当前 VNC 实例没有加载密码文件，改用 Xtigervnc 直接启动以强制启用 VncAuth..."
    if vnc_start_raw; then
        echo "VNC 密码认证已启用。"
    else
        echo "[警告] 未能启用密码认证，客户端可能报 \"No password configured\"。"
        VNC_OK=0
    fi
fi
# 端口起来了但窗口管理器没接管（典型：旧会话残留导致 EBUSY、黑屏无面板）
# → 彻底清理后重试一次，尽量不让用户遇到"进得去但没桌面"的状态。
if [ "$VNC_OK" = "1" ] && ! vnc_wait_wm; then
    echo "[提示] 桌面会话未接管窗口管理器，清理残留会话后重试..."
    # 设过密码时优先用带 -rfbauth 的直接启动，避免 systemd 单元把
    # 密码配置丢掉（会变成 "No password configured for VNC Auth"）。
    if [ -n "$VNC_PASSWORD" ]; then
        vnc_start_raw || vnc_start_via_unit || true
    elif [ -n "$VNC_UNIT" ]; then
        vnc_start_via_unit || true
    else
        vnc_start_direct || true
    fi
    vnc_wait_wm || true
fi
if [ "$VNC_OK" = "1" ]; then
    echo "VNC 服务已启动，监听 ${VNC_PORT} 端口。"
    if [ -n "$VNC_PASSWORD" ] && vnc_auth_configured; then
        echo "VNC 密码认证已就绪。"
    fi
    if vnc_wm_running; then
        echo "桌面会话已启动（窗口管理器运行中，$DESKTOP_NAME 桌面可用）。"
    else
        echo "[警告] VNC 端口已监听，但桌面会话未启动，连接后可能是黑屏。最近会话日志:"
        tail -5 "$HOME/.xsession-errors" 2>/dev/null | sed 's/^/    /'
    fi
else
    echo "[错误] VNC 服务未能监听 ${VNC_PORT} 端口，远程桌面无法连接。"
    echo "----- $VNC_CMD 启动输出 -----"
    tail -20 "$VNC_START_LOG" 2>/dev/null | sed 's/^/    /'
    echo "----- $HOME/.vnc 会话日志 -----"
    tail -20 "$HOME/.vnc/"*.log 2>/dev/null | sed 's/^/    /'
    if [ "$STAGE" = "vncsvc" ]; then
        fail "TigerVNC 启动失败：没有监听 ${VNC_PORT} 端口。请查看上方日志（常见原因：桌面会话命令不存在、密码文件权限不对、端口被占用）。"
    fi
    echo "[警告] 已跳过远程桌面，请修复后单独重跑「TigerVNC 服务」步骤。"
fi
fi  # 阶段 vncsvc 结束

# ====================== 阶段：系统基础（共享目录） ======================
if stage_on base; then
# ---------------------------- Android 共享目录 ----------------------------
step "shared"
echo "正在配置 Android 共享目录挂载..."
$SUDO apt-get $APT_OPTS install -y xdg-user-dirs > /dev/null 2>&1 || true
command -v xdg-user-dirs-update > /dev/null 2>&1 && xdg-user-dirs-update || true
if [ ! -d "/mnt/shared" ]; then
    $SUDO mkdir -p /mnt/shared 2>/dev/null || true
fi
$SUDO chown -R "$TARGET_USER:$TARGET_USER" "$HOME/.config" 2>/dev/null || true
DESKTOP_DIR=$(xdg-user-dir DESKTOP 2>/dev/null)
[ -z "$DESKTOP_DIR" ] && DESKTOP_DIR="$HOME/Desktop"
[ -d "$DESKTOP_DIR" ] || mkdir -p "$DESKTOP_DIR" 2>/dev/null
$SUDO chown -R "$TARGET_USER:$TARGET_USER" "$DESKTOP_DIR" 2>/dev/null || true
if [ "$SYS_LOCALE" = "zh_CN.UTF-8" ]; then LINK_NAME="Android文件"; else LINK_NAME="Android_Files"; fi
ln -sf /mnt/shared "$DESKTOP_DIR/$LINK_NAME" 2>/dev/null || true
mkdir -p "$HOME/.config/gtk-3.0" 2>/dev/null
touch "$HOME/.config/gtk-3.0/bookmarks" 2>/dev/null
grep -q "file:///mnt/shared" "$HOME/.config/gtk-3.0/bookmarks" 2>/dev/null || \
    echo "file:///mnt/shared 📱Android_共享磁盘" >> "$HOME/.config/gtk-3.0/bookmarks"

fi  # 阶段 base（共享目录）结束

# ============================ 阶段：可选组件 ============================
if stage_on apps; then
# ---------------------------- 可选组件 ----------------------------
step "optional"
if [ "$INSTALL_IME" = "1" ]; then
    echo "正在安装中文输入法 Fcitx5..."
    $SUDO apt-get $APT_OPTS install -y fcitx5 fcitx5-chinese-addons fcitx5-frontend-gtk3 fcitx5-frontend-qt5 im-config || echo "[警告] 输入法安装未完全成功。"
    command -v im-config > /dev/null 2>&1 && im-config -n fcitx5 > /dev/null 2>&1 || true
fi
if [ "$INSTALL_APPS" = "1" ]; then
    echo "正在安装常用桌面与系统软件套件（耗时较长）..."
    $SUDO apt-get $APT_OPTS install -y curl wget git htop unzip p7zip-full vlc gimp remmina libreoffice pulseaudio-utils || echo "[警告] 部分常用软件安装失败，可稍后在虚拟机内补装。"
    $SUDO apt-get $APT_OPTS install -y chromium || echo "[提示] chromium 安装失败（非 Debian 官方仓库），可自行使用其他浏览器。"
    if [ "$SYS_LOCALE" = "zh_CN.UTF-8" ]; then
        $SUDO apt-get $APT_OPTS install -y libreoffice-l10n-zh-cn || true
    fi
fi

fi  # 阶段 apps 结束

# ---------------------------- 完成 ----------------------------
step "done"
echo ""
echo "============================================================"
case "$STAGE" in
    base)    echo " 系统基础环境与中文支持配置完成！" ;;
    desktop) echo " $DESKTOP_NAME 桌面环境安装完成！" ;;
    apps)    echo " 常用软件与中文输入法安装完成！" ;;
    vncsvc)  echo " TigerVNC 服务已配置并启动！" ;;
    *)       echo " 全部配置已完成！" ;;
esac
echo "============================================================"
if [ "$STAGE" = "desktop" ] || [ "$STAGE" = "all" ]; then
    echo "桌面环境: $DESKTOP_NAME"
fi
if [ "$STAGE" = "vncsvc" ] || [ "$STAGE" = "all" ]; then
    echo "VNC 地址: 127.0.0.1:${VNC_PORT}  (需在 Terminal 应用的「Port control」中放行 ${VNC_PORT})"
fi
if [ "$STAGE" = "base" ] || [ "$STAGE" = "all" ]; then
    echo "SSH 端口: $SSH_PORT"
fi
marker "RESULT:OK"
sleep 0.4
exit 0
