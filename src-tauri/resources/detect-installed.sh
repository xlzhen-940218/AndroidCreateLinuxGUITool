#!/bin/bash
# 检测虚拟机内已经安装的桌面环境 / 常用软件 / 中文输入法。
# 由 PC 端推送到共享目录后执行：bash acl_probe.sh <日志文件>
# 结果写成 KEY=VALUE 行，最后一行 @@DONE 表示检测结束。

LOG="$1"
[ -z "$LOG" ] && LOG="/mnt/shared/acl_probe.log"
: > "$LOG" 2>/dev/null

has() { dpkg -s "$1" > /dev/null 2>&1; }

desktop_installed() {
    case "$1" in
        xfce)      has xfce4 || has xfce4-session ;;
        gnome)     has gnome-shell ;;
        kde)       has plasma-workspace || has kde-plasma-desktop ;;
        mate)      has mate-session-manager ;;
        cinnamon)  has cinnamon ;;
        lxqt)      has lxqt-session ;;
        lxde)      has lxsession || has lxde ;;
        flashback) has gnome-flashback ;;
        *)         return 1 ;;
    esac
}

# ---- 桌面环境 ----
DESK=""
for d in xfce gnome kde mate cinnamon lxqt lxde flashback; do
    desktop_installed "$d" || continue
    if [ -z "$DESK" ]; then DESK="$d"; else DESK="$DESK,$d"; fi
done

# ---- 常用软件套件 ----
APP_TOTAL=0
APP_HAVE=0
for p in curl wget git htop vlc gimp remmina libreoffice chromium; do
    APP_TOTAL=$((APP_TOTAL + 1))
    has "$p" && APP_HAVE=$((APP_HAVE + 1))
done

# ---- 中文输入法 / VNC / SSH ----
IME=0
if has fcitx5 && has fcitx5-chinese-addons; then IME=1; fi
VNC=0
if has tigervnc-standalone-server; then VNC=1; fi
# 装了包不等于服务在跑：查 5901 是否真的在监听
VNC_PORT=0
if command -v ss > /dev/null 2>&1; then
    ss -ltnH 2>/dev/null | awk '{print $4}' | grep -q ':5901$' && VNC_PORT=1
elif command -v netstat > /dev/null 2>&1; then
    netstat -ltn 2>/dev/null | awk '{print $4}' | grep -q ':5901$' && VNC_PORT=1
fi
VNC_UNIT="$(systemctl is-active tigervncserver@:1.service 2>/dev/null | head -n1)"
SSH=0
if has openssh-server; then SSH=1; fi

{
    echo "DESKTOP=$DESK"
    echo "APPS=$APP_HAVE/$APP_TOTAL"
    echo "IME=$IME"
    echo "VNC=$VNC"
    echo "VNC_PORT=$VNC_PORT"
    echo "VNC_UNIT=$VNC_UNIT"
    echo "SSH=$SSH"
    echo "@@DONE"
} >> "$LOG" 2>/dev/null
