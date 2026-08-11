#!/usr/bin/env bash
# ProtogenOS OPTIONAL theme installer -- KDE Plasma color scheme + wallpaper
# for CachyOS / Arch + KDE Plasma.
#
# This script is completely separate from install_assistant.sh and is NEVER
# invoked automatically by it, by protogen-daemon, or by anything the
# assistant does at runtime. The assistant does not change your system
# theme on its own, ever -- this script exists only for you to run by hand,
# once, if you specifically want the ProtogenOS Plasma color scheme applied.
#
# Run as your normal user (NOT root). It will use sudo where needed.
set -euo pipefail

echo "=================================================="
echo " This will install packages and apply a KDE Plasma"
echo " color scheme + wallpaper to your CURRENT session."
echo " It is entirely optional and separate from the"
echo " assistant itself -- the assistant never touches"
echo " your theme on its own."
echo "=================================================="
read -rp "Continue and apply the ProtogenOS theme now? [y/N] " CONFIRM
if [[ "${CONFIRM,,}" != "y" ]]; then
    echo "Aborted -- no changes made."
    exit 0
fi

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "==> ProtogenOS theme installer running from $DIR"

need_pkg() {
    if ! pacman -Qi "$1" &>/dev/null; then
        echo "==> Installing $1"
        sudo pacman -S --needed --noconfirm "$1"
    fi
}

echo "==> Installing base packages (official repos)"
need_pkg papirus-icon-theme
need_pkg konsole
need_pkg kde-cli-tools
need_pkg plasma-workspace

if command -v paru &>/dev/null; then
    AUR=paru
elif command -v yay &>/dev/null; then
    AUR=yay
else
    AUR=""
fi

if [ -n "$AUR" ]; then
    echo "==> AUR helper found ($AUR). Installing cursor theme + KWin theme."
    "$AUR" -S --needed --noconfirm bibata-cursor-theme-bin sweet-kde-git || true
else
    echo "==> No AUR helper (paru/yay) found. Skipping Bibata cursors + Sweet KWin theme."
    echo "    Install paru (https://github.com/morganamilo/paru) then re-run for those extras."
fi

echo "==> Installing color scheme"
mkdir -p ~/.local/share/color-schemes
cp "$DIR/theme/ProtogenOS.colors" ~/.local/share/color-schemes/

echo "==> Installing Konsole color scheme"
mkdir -p ~/.local/share/konsole
cp "$DIR/theme/ProtogenOS.colorscheme" ~/.local/share/konsole/

echo "==> Installing wallpaper"
mkdir -p ~/.local/share/wallpapers/ProtogenOS
cp "$DIR/theme/protogenos_wallpaper.svg" ~/.local/share/wallpapers/ProtogenOS/contents.svg

echo "==> Applying color scheme + widget style"
plasma-apply-colorscheme ProtogenOS || echo "  (plasma-apply-colorscheme not found, set manually in System Settings > Colors)"
kwriteconfig6 --file kdeglobals --group KDE --key widgetStyle Breeze || \
kwriteconfig5 --file kdeglobals --group KDE --key widgetStyle Breeze

echo "==> Setting icon theme"
kwriteconfig6 --file kdeglobals --group Icons --key Theme Papirus-Dark || \
kwriteconfig5 --file kdeglobals --group Icons --key Theme Papirus-Dark

if [ -n "$AUR" ]; then
    echo "==> Setting cursor theme"
    kwriteconfig6 --file kcminputrc --group Mouse --key cursorTheme Bibata-Modern-Ice || \
    kwriteconfig5 --file kcminputrc --group Mouse --key cursorTheme Bibata-Modern-Ice

    echo "==> Setting KWin window decoration"
    kwriteconfig6 --file kwinrc --group org.kde.kdecoration2 --key theme "__aurorae__svg__Sweet-Dark" || true
fi

echo "==> Setting wallpaper (this session's default screen)"
PLASMA_SCRIPT="
var allDesktops = desktops();
for (i=0;i<allDesktops.length;i++){
    d = allDesktops[i];
    d.wallpaperPlugin = 'org.kde.image';
    d.currentConfigGroup = Array('Wallpaper','org.kde.image','General');
    d.writeConfig('Image', 'file://$HOME/.local/share/wallpapers/ProtogenOS/contents.svg');
}
"
qdbus org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$PLASMA_SCRIPT" 2>/dev/null || \
qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$PLASMA_SCRIPT" 2>/dev/null || \
echo "  (couldn't set wallpaper via dbus, set manually: right click desktop > Configure Desktop)"

echo "==> Restarting Plasma shell to apply changes"
kquitapp6 plasmashell 2>/dev/null || kquitapp5 plasmashell 2>/dev/null || true
(plasmashell &>/dev/null &) 

echo ""
echo "=================================================="
echo " ProtogenOS theme applied."
echo " Manual touch-ups (System Settings > Appearance):"
echo "  - Panel: right-click panel > Enter Edit Mode >"
echo "           set panel to Floating + increase transparency"
echo "           under panel settings for the HUD look."
echo "  - Fonts: pick a monospace/tech font (e.g. 'Iosevka' or"
echo "           'JetBrains Mono') under Appearance > Fonts."
echo "  - If Bibata cursors / Sweet KWin theme didn't install,"
echo "    install paru and re-run this script."
echo "=================================================="
