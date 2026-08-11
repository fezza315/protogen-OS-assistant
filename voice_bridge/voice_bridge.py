#!/usr/bin/env python3
"""
voice_bridge.py
----------------
Persistent STT/TTS worker process, spawned and supervised by
protogen-daemon (see daemon/src/backend.rs). Kept alive for the whole
session instead of being re-launched per utterance like the old
--voice/--speak flags, so the whisper model only loads once and turnaround
per utterance is fast.

Protocol: newline-delimited JSON on stdin/stdout.

  stdin  -> {"op": "listen", "seconds": 5}
  stdout <- {"op": "transcript", "text": "open firefox"}

  stdin  -> {"op": "speak", "text": "Opening Firefox."}
  stdout <- {"op": "spoke", "ok": true}

  stdin  -> {"op": "shutdown"}
  (process exits)

This process does not decide what to DO with a transcript -- it only
converts audio to text and text to audio. All planning/execution stays in
protogen-daemon (Rust), on the other side of the daemon's own stdin/stdout
pipe to this process. Kept as Python because faster-whisper and Piper's
Python ergonomics are the path of least friction for these two libraries
specifically, per the project's own preference for Python/Rust/sh.
"""
import json
import sys
import tempfile
import subprocess
import os
from pathlib import Path

VOICES_DIR = Path(
    os.environ.get("PROTOGEN_VOICES_DIR", str(Path.home() / ".local/share/protogenos/voices"))
)
VOICES_DIR.mkdir(parents=True, exist_ok=True)

WHISPER_MODEL_SIZE = os.environ.get("PROTOGEN_WHISPER_MODEL", "base")
DEFAULT_VOICE_BANK = os.environ.get("PROTOGEN_VOICE_BANK")


def log(msg):
    print(f"[voice_bridge] {msg}", file=sys.stderr, flush=True)


class STT:
    def __init__(self, model_size=WHISPER_MODEL_SIZE):
        from faster_whisper import WhisperModel
        # int8 keeps this workable on integrated graphics / CPU-only laptops
        # (matches the target hardware profile: Intel i5-1335U iGPU).
        self.model = WhisperModel(model_size, device="cpu", compute_type="int8")

    def listen_once(self, seconds=5, samplerate=16000):
        import sounddevice as sd
        import soundfile as sf

        log(f"listening for {seconds}s...")
        audio = sd.rec(int(seconds * samplerate), samplerate=samplerate, channels=1)
        sd.wait()
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            sf.write(f.name, audio, samplerate)
            wav_path = f.name
        try:
            segments, _info = self.model.transcribe(wav_path, beam_size=1)
            return " ".join(seg.text.strip() for seg in segments).strip()
        finally:
            os.unlink(wav_path)


class TTS:
    def __init__(self, voice_bank=None):
        self.voice_bank = voice_bank or DEFAULT_VOICE_BANK
        self._piper_available = self._check_piper()

    def _check_piper(self):
        try:
            subprocess.run(["piper", "--help"], capture_output=True, timeout=5)
            return True
        except (FileNotFoundError, subprocess.TimeoutExpired):
            return False

    def _voice_model_path(self):
        if not self.voice_bank:
            candidates = sorted(VOICES_DIR.glob("*.onnx"))
            return candidates[0] if candidates else None
        candidate = VOICES_DIR / f"{self.voice_bank}.onnx"
        return candidate if candidate.exists() else None

    def speak(self, text):
        if not self._piper_available:
            log(f"piper not found on PATH, cannot speak: {text}")
            return False
        model_path = self._voice_model_path()
        if not model_path:
            log("no voice bank available in " + str(VOICES_DIR))
            return False
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            wav_path = f.name
        try:
            subprocess.run(
                ["piper", "--model", str(model_path), "--output_file", wav_path],
                input=text.encode("utf-8"),
                capture_output=True,
                timeout=30,
            )
            subprocess.run(["paplay", wav_path], capture_output=True, timeout=30)
            return True
        except Exception as e:
            log(f"TTS playback failed: {e}")
            return False
        finally:
            try:
                os.unlink(wav_path)
            except OSError:
                pass


def main():
    stt = None
    tts = None

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps({"op": "error", "message": "bad json"}), flush=True)
            continue

        op = req.get("op")

        if op == "shutdown":
            break

        elif op == "listen":
            if stt is None:
                try:
                    stt = STT()
                except Exception as e:
                    print(json.dumps({"op": "error", "message": f"STT init failed: {e}"}), flush=True)
                    continue
            seconds = req.get("seconds", 5)
            try:
                text = stt.listen_once(seconds=seconds)
                print(json.dumps({"op": "transcript", "text": text}), flush=True)
            except Exception as e:
                print(json.dumps({"op": "error", "message": f"listen failed: {e}"}), flush=True)

        elif op == "speak":
            if tts is None:
                tts = TTS(voice_bank=req.get("voice_bank"))
            ok = tts.speak(req.get("text", ""))
            print(json.dumps({"op": "spoke", "ok": ok}), flush=True)

        elif op == "set_voice_bank":
            if tts is None:
                tts = TTS(voice_bank=req.get("voice_bank"))
            else:
                tts.voice_bank = req.get("voice_bank")
            print(json.dumps({"op": "ack"}), flush=True)

        else:
            print(json.dumps({"op": "error", "message": f"unknown op '{op}'"}), flush=True)


if __name__ == "__main__":
    main()
