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
if [ "$(id -u)" -eq 0 ]; then
    SUDO=""
elif sudo -n true 2>/dev/null; then
    SUDO="sudo -n"
elif command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
else
    fail "虚拟机内没有 sudo，无法自动安装。"
fi
if [ -n "$SUDO" ] && ! $SUDO true 2>/dev/null; then
    fail "当前账户 ($TARGET_USER) 的 sudo 需要交互输入密码，无法自动安装。请先在终端中执行 sudo -v 设置好免密或缓存凭据后重试。"
fi
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

step "environment"
echo "=== Android Linux 桌面环境安装 ==="
echo "日志文件: $LOG_FILE"
echo "用户: $TARGET_USER  (sudo='${SUDO:-none}')"

# ---------------------------- 系统识别 ----------------------------
step "detect_system"
DEB_RAW="$(cat /etc/debian_version 2>/dev/null | tr -d '[:space:]')"
DEB_VER="$(printf '%s' "$DEB_RAW" | cut -d. -f1 | tr -dc '0-9')"
case "$DEB_VER" in
    12) SUITE="bookworm" ;;
    13) SUITE="trixie" ;;
    *)  SUITE="$(lsb_release -cs 2>/dev/null || echo bookworm)" ;;
esac
echo "系统: Debian ${DEB_VER:-未知} (suite=$SUITE)"
echo "架构: $(uname -m)   内核: $(uname -r)"

APT_OPTS="-o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold"

# ---------------------------- 切换国内源 ----------------------------
if [ "$CN_MIRROR" = "1" ]; then
    step "mirror"
    echo "正在切换到国内镜像源 (清华 TUNA)..."
    DEB_CN="https://mirrors.tuna.tsinghua.edu.cn/debian"
    SEC_CN="https://mirrors.tuna.tsinghua.edu.cn/debian-security"
    $SUDO cp -n /etc/apt/sources.list /etc/apt/sources.list.bak.acl 2>/dev/null || true
    $SUDO cp -n /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list.d/debian.sources.bak.acl 2>/dev/null || true

    # deb822 格式（Debian 12/13 默认）
    $SUDO tee /etc/apt/sources.list.d/debian.sources > /dev/null <<EOF
Types: deb
URIs: $DEB_CN
Suites: $SUITE $SUITE-updates
Components: main contrib non-free non-free-firmware
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg

Types: deb
URIs: $SEC_CN
Suites: $SUITE-security
Components: main contrib non-free non-free-firmware
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
EOF

    # 经典格式（兼容老镜像）
    $SUDO tee /etc/apt/sources.list > /dev/null <<EOF
deb $DEB_CN $SUITE main contrib non-free non-free-firmware
deb $DEB_CN $SUITE-updates main contrib non-free non-free-firmware
deb $SEC_CN $SUITE-security main contrib non-free non-free-firmware
EOF
    echo "国内源已生效: $DEB_CN ($SUITE)"
fi

# ---------------------------- 更新软件包 ----------------------------
step "apt_update"
echo "正在更新软件包列表..."
$SUDO apt-get $APT_OPTS update -y || fail "apt-get update 失败。请检查虚拟机网络连通性，或临时关闭「使用国内源」后重试。"
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

# ---------------------------- 桌面环境 ----------------------------
step "desktop"
# 注意：Debian 12/13 的仓库里**没有** `xfce-desktop` 这类 tasksel 任务元包
# （`apt-cache show xfce-desktop` 查不到），所以必须直接安装真实的软件包；
# tasksel 只在任务包确实存在时才用。
case "$DESKTOP" in
    kde)       DESKTOP_NAME="KDE Plasma";     SESSION_WANT="plasmax11";               SESSION_KEY="plasma";    TASK_NAME="kde-desktop";              TASK_PACKAGES="kde-plasma-desktop plasma-workspace" ;;
    gnome)     DESKTOP_NAME="GNOME";           SESSION_WANT="gnome-xorg";               SESSION_KEY="gnome";     TASK_NAME="gnome-desktop";            TASK_PACKAGES="gnome-session gnome-session-xsession gnome-shell" ;;
    xfce)      DESKTOP_NAME="XFCE";            SESSION_WANT="xfce";                     SESSION_KEY="xfce";      TASK_NAME="xfce-desktop";             TASK_PACKAGES="xfce4 xfce4-terminal" ;;
    mate)      DESKTOP_NAME="MATE";            SESSION_WANT="mate";                     SESSION_KEY="mate";      TASK_NAME="mate-desktop";             TASK_PACKAGES="mate-desktop-environment mate-session-manager" ;;
    cinnamon)  DESKTOP_NAME="Cinnamon";        SESSION_WANT="cinnamon";                 SESSION_KEY="cinnamon";  TASK_NAME="cinnamon-desktop";         TASK_PACKAGES="cinnamon cinnamon-session cinnamon-common" ;;
    lxqt)      DESKTOP_NAME="LXQt";            SESSION_WANT="lxqt";                     SESSION_KEY="lxqt";      TASK_NAME="lxqt-desktop";             TASK_PACKAGES="lxqt lxqt-session" ;;
    lxde)      DESKTOP_NAME="LXDE";            SESSION_WANT="lxde";                     SESSION_KEY="lxde";      TASK_NAME="lxde-desktop";             TASK_PACKAGES="lxde lxde-session" ;;
    flashback) DESKTOP_NAME="GNOME Flashback"; SESSION_WANT="gnome-flashback-metacity"; SESSION_KEY="flashback"; TASK_NAME="gnome-flashback-desktop"; TASK_PACKAGES="gnome-flashback gnome-session-xsession" ;;
    *)         fail "未知的桌面环境类型: $DESKTOP" ;;
esac
echo "正在安装 $DESKTOP_NAME 桌面环境，这可能需要较长时间..."
# 任务包在仓库里存在时才走 tasksel，否则直接装真实软件包
if apt-cache show "$TASK_NAME" > /dev/null 2>&1; then
    $SUDO apt-get $APT_OPTS install -y tasksel || echo "[警告] 安装 tasksel 失败。"
    if ! $SUDO tasksel install "$TASK_NAME" < /dev/null; then
        echo "[提示] tasksel 安装未完全成功，改用直接安装任务包..."
    fi
fi
$SUDO apt-get $APT_OPTS install -y $TASK_PACKAGES || fail "安装 $DESKTOP_NAME 桌面环境失败（软件包: $TASK_PACKAGES）。"

# ---------------------------- VNC 配置 ----------------------------
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
    echo "可用的 X 会话: $(ls /usr/share/xsessions/ 2>/dev/null | tr '\n' ' ')"
    fail "桌面环境已安装，但没有找到可用的 X 会话文件（/usr/share/xsessions）。"
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
    for PW_DIR in "$HOME/.vnc" "$HOME/.config/tigervnc"; do
        PW_FILE="$PW_DIR/passwd"
        printf '%s\n%s\n' "$VNC_PASSWORD" "$VNC_PASSWORD" | vncpasswd "$PW_FILE" > /dev/null 2>&1 || true
        PW_SIZE=$(wc -c < "$PW_FILE" 2>/dev/null | tr -d ' ')
        if [ "$PW_SIZE" != "8" ]; then
            echo "[提示] vncpasswd 不可用，改用内置方式写入密码文件..."
            PW_BYTES=$(printf '%s' "$VNC_PASSWORD" | od -An -tu1 -N8 | tr -s ' \n' ' ')
            printf '%s\n' "$PW_BYTES" | awk '{
                for (i = 1; i <= 8; i++) {
                    c = (i <= NF) ? $i : 0; r = 0;
                    for (j = 0; j < 8; j++) { r = r * 2 + (c % 2); c = int(c / 2); }
                    printf "%c", r;
                }
            }' > "$PW_FILE"
        fi
        # TigerVNC 强制要求密码文件权限为 0600，否则拒绝启动
        chmod 600 "$PW_FILE" 2>/dev/null || true
    done
    if [ "$(wc -c < "$HOME/.config/tigervnc/passwd" 2>/dev/null | tr -d ' ')" = "8" ]; then
        echo "VNC 密码已设置。"
    else
        echo "[警告] VNC 密码文件创建失败，VNC 将可能不校验密码。"
    fi
fi

mkdir -p "$HOME/.vnc" "$HOME/.config/tigervnc"
VNC_PORT=$(( 5900 + ${VNC_DISPLAY#:} ))
cat > "$HOME/.vnc/config" <<EOF
session=$ACTUAL_SESSION
geometry=$VNC_GEOMETRY
localhost=no
alwaysshared
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
vnc_port_listening() {
    (ss -ltn 2>/dev/null || netstat -ltn 2>/dev/null) | grep -q ":${VNC_PORT}\b"
}
# 上次残留的会话进程不会随 Xvnc 一起退出，会让新会话以
# "X server already running" 的形式提前退出（桌面依旧黑屏），所以先清掉。
vnc_kill_stale_session() {
    pkill -u "$TARGET_USER" -f 'xfce4-session|xfce4-panel|xfdesktop|xfwm4|xfsettingsd|tigervncconfig' 2>/dev/null || true
    sleep 2
}
if [ -n "$VNC_UNIT" ]; then
    vnc_kill_stale_session
    $SUDO systemctl restart "$VNC_UNIT" 2>/dev/null || true
    sleep 4
    if vnc_port_listening; then
        VNC_OK=1
    fi
fi
if [ "$VNC_OK" != "1" ]; then
    echo "[提示] systemd 方式未能监听 ${VNC_PORT}，改用 $VNC_CMD 直接启动..."
    $RUNAS $VNC_CMD -kill "$VNC_DISPLAY" > /dev/null 2>&1 || true
    vnc_kill_stale_session
    if $RUNAS $VNC_CMD "$VNC_DISPLAY" -geometry "$VNC_GEOMETRY" -localhost no > /dev/null 2>&1; then
        sleep 4
        if vnc_port_listening; then
            VNC_OK=1
        fi
    fi
fi
if [ "$VNC_OK" = "1" ]; then
    sleep 4
    if vnc_port_listening; then
        echo "VNC 服务已启动，监听 ${VNC_PORT} 端口。"
        if pgrep -u "$TARGET_USER" -x xfwm4 > /dev/null 2>&1; then
            echo "桌面会话已启动（窗口管理器 xfwm4 运行中）。"
        else
            echo "[警告] VNC 端口已监听，但桌面会话未启动，连接后可能是黑屏。最近会话日志:"
            tail -3 "$HOME/.xsession-errors" 2>/dev/null | sed 's/^/    /'
        fi
    else
        echo "[警告] VNC 进程已启动，但未检测到 ${VNC_PORT} 端口监听，请稍后重试连接。"
    fi
else
    echo "[警告] VNC 服务启动失败，可稍后手动执行: vncserver $VNC_DISPLAY"
    echo "最近日志: $(tail -3 "$HOME/.vnc/"*.log 2>/dev/null | tr '\n' ' ')"
fi

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

# ---------------------------- 完成 ----------------------------
step "done"
echo ""
echo "============================================================"
echo " 全部配置已完成！"
echo "============================================================"
echo "桌面环境: $DESKTOP_NAME"
echo "VNC 地址: 127.0.0.1:${VNC_PORT}  (需在 Terminal 应用的「Port control」中放行 ${VNC_PORT})"
echo "SSH 端口: $SSH_PORT"
marker "RESULT:OK"
sleep 0.4
exit 0
