# Voice & focus-or-launch setup

The installer handles everything it can through your distro's package
manager. Two tools don't have packages everywhere and need a short manual
step depending on your distro:

## kdotool (window focus-or-launch on KDE Wayland/X11 + wlroots compositors)

Arch/CachyOS (via AUR): `paru -S kdotool` (the installer does this for you
if you have `paru`/`yay` installed).

Everything else, build from source (needs Go):
```
git clone https://github.com/jinliu/kdotool
cd kdotool
go build
sudo install -m755 kdotool /usr/local/bin/kdotool
```

Without kdotool (and without wmctrl either), "open firefox" always opens a
new window instead of focusing an existing one -- everything else still
works.

## Piper TTS (offline voice output)

Arch/CachyOS (via AUR): `paru -S piper-tts`.

Everything else, grab a prebuilt release binary:
```
curl -L -o piper.tar.gz https://github.com/rhasspy/piper/releases/latest/download/piper_linux_x86_64.tar.gz
tar -xzf piper.tar.gz
sudo install -m755 piper/piper /usr/local/bin/piper
```

Then drop a voice model into `~/.local/share/protogenos/voices/`:
```
curl -LO https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx
curl -LO https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx.json
mv en_US-amy-medium.onnx* ~/.local/share/protogenos/voices/
```

Any Piper voice from https://github.com/rhasspy/piper/blob/master/VOICES.md
works the same way -- both files (`.onnx` + `.onnx.json`) need to be present.

## Whisper STT

Installed automatically into the voice bridge's own venv by the installer
(`faster-whisper`). No manual step needed. First run downloads the "base"
model (~150MB) once; set `PROTOGEN_WHISPER_MODEL=tiny` in the systemd unit
environment for a smaller/faster model on lower-end hardware, at some
accuracy cost.
