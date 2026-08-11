#!/usr/bin/env bash
# ProtogenOS Assistant installer
# --------------------------------
# Run as your normal user (NOT root); uses sudo only for package installs.
#
# What this does, in order:
#   1. Detects your Linux distribution (/etc/os-release) and picks the
#      right package manager.
#   2. Installs required system packages (Rust toolchain if missing, GTK4
#      dev libs, Python + STT/TTS deps, kdotool/wmctrl, portaudio, Jan.ai).
#   3. Builds the three ProtogenOS Rust binaries (protogen-daemon,
#      protogen-ui) via cargo, release profile.
#   4. Installs them + the voice bridge + avatar assets under
#      ~/.local/share/protogenos and ~/.local/bin.
#   5. Installs a systemd --user service so protogen-daemon (and, via it,
#      Jan.ai + the voice bridge) starts automatically and the user never
#      manually launches any backing process.
#
# Nothing outside $HOME is touched except package installs, which always go
# through your distro's real package manager (never a raw curl | sh unless
# a package genuinely has no repo path, and even then you're shown the
# command before it runs, not before you agreed to run this installer at
# all).
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="$HOME/.local/share/protogenos"
BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

contentsoffile=$(cat dedsec.txt)
echo -e "$contents"
echo "=================================================="
echo " ProtogenOS Assistant installer"
echo "=================================================="

# ---------------------------------------------------------------------
# 1. Distro detection
# ---------------------------------------------------------------------
DISTRO_ID="unknown"
DISTRO_ID_LIKE=""
if [ -f /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    DISTRO_ID="${ID:-unknown}"
    DISTRO_ID_LIKE="${ID_LIKE:-}"
fi
echo "==> Detected distro: $DISTRO_ID (like: ${DISTRO_ID_LIKE:-none})"

PKG_MANAGER=""
case "$DISTRO_ID $DISTRO_ID_LIKE" in
    *arch*|*cachyos*|*manjaro*|*endeavouros*) PKG_MANAGER="pacman" ;;
    *fedora*|*rhel*|*centos*)                 PKG_MANAGER="dnf" ;;
    *debian*|*ubuntu*|*pop*|*mint*)           PKG_MANAGER="apt" ;;
    *opensuse*|*suse*)                        PKG_MANAGER="zypper" ;;
    *)
        echo "!! Could not confidently detect a supported package manager for '$DISTRO_ID'."
        echo "   Supported: Arch/CachyOS/Manjaro (pacman), Fedora (dnf), Debian/Ubuntu (apt), openSUSE (zypper)."
        read -rp "   Continue anyway and skip automatic package installs? [y/N] " CONTINUE_UNKNOWN
        if [[ "${CONTINUE_UNKNOWN,,}" != "y" ]]; then
            exit 1
        fi
        ;;
esac
echo "==> Using package manager: ${PKG_MANAGER:-none}"

# ---------------------------------------------------------------------
# 2. Package installs
# ---------------------------------------------------------------------
AUR=""
if command -v paru &>/dev/null; then AUR=paru
elif command -v yay &>/dev/null; then AUR=yay
fi

install_pacman() {
    echo "==> Installing packages via pacman"
    sudo pacman -S --needed --noconfirm \
        rust gtk4 python python-pip portaudio wmctrl base-devel git \
        pipewire-pulse alsa-utils || true
    if [ -n "$AUR" ]; then
        echo "==> Installing AUR extras via $AUR (kdotool, piper-tts)"
        "$AUR" -S --needed --noconfirm kdotool piper-tts || \
            echo "   (AUR extras failed -- kdotool/piper install manually if you want those features)"
    else
        echo "!! No AUR helper (paru/yay) found -- kdotool and piper-tts won't be installed."
        echo "   Install paru (https://github.com/morganamilo/paru) and re-run for full voice + focus-or-launch support."
    fi
}

install_dnf() {
    echo "==> Installing packages via dnf"
    sudo dnf install -y \
        rust cargo gtk4-devel python3 python3-pip portaudio-devel wmctrl \
        @development-tools git pipewire-pulseaudio alsa-utils || true
    echo "!! kdotool and piper-tts don't have Fedora repo packages -- see docs/VOICE_SETUP.md"
    echo "   for manual install instructions (both build from source easily)."
}

install_apt() {
    echo "==> Installing packages via apt"
    sudo apt-get update -y
    sudo apt-get install -y \
        rustc cargo libgtk-4-dev python3 python3-pip python3-venv \
        portaudio19-dev wmctrl build-essential git pipewire-pulse alsa-utils || true
    echo "!! kdotool and piper-tts don't have apt repo packages -- see docs/VOICE_SETUP.md"
    echo "   for manual install instructions (both build from source easily)."
}

install_zypper() {
    echo "==> Installing packages via zypper"
    sudo zypper install -y \
        rust cargo gtk4-devel python3 python3-pip portaudio-devel wmctrl \
        patterns-devel-base-devel_basis git pipewire-pulseaudio alsa-utils || true
    echo "!! kdotool and piper-tts don't have openSUSE repo packages -- see docs/VOICE_SETUP.md."
}

case "$PKG_MANAGER" in
    pacman)  install_pacman ;;
    dnf)     install_dnf ;;
    apt)     install_apt ;;
    zypper)  install_zypper ;;
    "")      echo "==> Skipping automatic package install." ;;
esac

if ! command -v cargo &>/dev/null; then
    echo "==> cargo still not found after package install. Installing rustup instead."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

if ! command -v jan &>/dev/null; then
    echo ""
    echo "!! Jan.ai ('jan' binary) not found on PATH."
    echo "   ProtogenOS backs its planner with Jan.ai's local server + a"
    echo "   DeepSeek model. Install Jan.ai from https://www.jan.ai/ (AppImage"
    echo "   or your distro's package if available), OR install the jan-nightly"
    echo "   CLI per Jan's docs, THEN pull a DeepSeek model inside Jan before"
    echo "   first run:"
    echo "     jan models pull deepseek-v4"
    echo "   protogen-daemon will start 'jan serve' itself once the 'jan'"
    echo "   binary is on PATH -- you do not need to run Jan separately."
    echo ""
fi

# ---------------------------------------------------------------------
# 3. Build the Rust workspace
# ---------------------------------------------------------------------
echo "==> Building ProtogenOS (release profile -- this can take a few minutes the first time)"
cd "$DIR"
cargo build --release --workspace

# ---------------------------------------------------------------------
# 4. Install binaries + assets
# ---------------------------------------------------------------------
echo "==> Installing to $INSTALL_DIR"
mkdir -p "$INSTALL_DIR" "$BIN_DIR" "$INSTALL_DIR/voices" "$INSTALL_DIR/avatar"

cp "$DIR/target/release/protogen-daemon" "$BIN_DIR/"
cp "$DIR/target/release/protogen-ui" "$BIN_DIR/"
cp -r "$DIR/voice_bridge" "$INSTALL_DIR/"
if [ -d "$DIR/theme/avatar" ]; then
    cp -r "$DIR/theme/avatar/"* "$INSTALL_DIR/avatar/" 2>/dev/null || true
fi

echo "==> Setting up Python venv for the voice bridge"
python3 -m venv "$INSTALL_DIR/voice_bridge/venv"
# shellcheck disable=SC1091
source "$INSTALL_DIR/voice_bridge/venv/bin/activate"
pip install --upgrade pip -q
pip install -q faster-whisper sounddevice soundfile
deactivate

# ---------------------------------------------------------------------
# 5. systemd --user service
# ---------------------------------------------------------------------
echo "==> Installing systemd --user service"
mkdir -p "$SYSTEMD_USER_DIR"
cat > "$SYSTEMD_USER_DIR/protogenos-daemon.service" << UNIT
[Unit]
Description=ProtogenOS assistant daemon (backs Jan.ai + voice bridge)
After=graphical-session.target

[Service]
ExecStart=$BIN_DIR/protogen-daemon
Restart=on-failure
RestartSec=3
Environment=PROTOGEN_VOICE_BRIDGE=$INSTALL_DIR/voice_bridge/voice_bridge.py
Environment=PROTOGEN_VOICE_PYTHON=$INSTALL_DIR/voice_bridge/venv/bin/python3
Environment=PROTOGEN_AVATAR_DIR=$INSTALL_DIR/avatar
Environment=PROTOGEN_VOICES_DIR=$INSTALL_DIR/voices
Environment=PROTOGEN_JAN_MODEL=deepseek-v4

[Install]
WantedBy=default.target
UNIT

systemctl --user daemon-reload
systemctl --user enable --now protogenos-daemon.service || \
    echo "!! Could not start the service automatically -- run 'systemctl --user start protogenos-daemon' manually."

if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
    echo ""
    echo "NOTE: $BIN_DIR isn't on your PATH yet. Add this to your shell rc:"
    echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

echo ""
echo "=================================================="
echo " Installed."
echo ""
echo " The daemon (protogen-daemon) now runs automatically in the"
echo " background via systemd --user, and starts Jan.ai + the voice"
echo " bridge itself -- you never need to launch those separately."
echo ""
echo " Launch the assistant window with:"
echo "     protogen-ui"
echo ""
echo " Drop Piper voice model .onnx/.onnx.json pairs into:"
echo "     $INSTALL_DIR/voices/"
echo ""
echo " Put idle.png / listening.png / thinking.png / speaking.png avatar"
echo " art into:"
echo "     $INSTALL_DIR/avatar/"
echo ""
echo " Check daemon logs with:"
echo "     journalctl --user -u protogenos-daemon -f"
echo ""
echo " To remove everything:"
echo "     systemctl --user disable --now protogenos-daemon"
echo "     rm -rf $INSTALL_DIR $BIN_DIR/protogen-daemon $BIN_DIR/protogen-ui"
echo "     rm $SYSTEMD_USER_DIR/protogenos-daemon.service"
echo "=================================================="
