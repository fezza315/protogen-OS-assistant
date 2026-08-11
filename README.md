# ProtogenOS

A local, voice-and-text controllable system assistant for Linux, with a
GTK4 avatar UI in the spirit of [Nyarch Assistant](https://github.com/NyarchLinux/NyarchAssistant),
backed by [Jan.ai](https://www.jan.ai/) running a DeepSeek model, offline
Whisper speech-to-text, and offline Piper speech output.

It can open/focus applications by voice or text, control common system
settings (volume, brightness, lock, screenshot), and -- for anything it
doesn't already know how to do -- research the request, build a concrete
step-by-step plan, show you exactly what it intends to run, and only touch
your system after you approve it. Approved plans are remembered, so the
next time you ask for the same thing it's instant.

## Why this is safe to run

The single hard rule this whole project is built around: **the AI model
never gets to run arbitrary shell text.** It can only ever propose steps
from a small, fixed, code-defined vocabulary (launch/focus an app, install
a named package, toggle a named systemd unit, a handful of utility actions,
a power action, a scoped config write) -- see `plan-types/src/lib.rs` for
the exact list. Every step is validated against that vocabulary before it's
even shown to you, and any step outside "open an app" or "toggle a utility"
always stops and asks for your explicit confirmation first, regardless of
how you phrased the request. There is no code path, anywhere in this
project, that hands the model's output to a shell interpreter.

## Architecture

```
protogenos/
├── plan-types/      Rust. The closed action vocabulary (the security boundary).
├── launcher-scan/   Rust. .desktop file indexing + focus-or-launch (kdotool/wmctrl).
├── cmdrunner/        Rust. The only place a Step becomes a real process.
├── daemon/           Rust. protogen-daemon -- the actual assistant:
│                       memory bank (SQLite), Jan.ai client, web research,
│                       dispatcher, Unix socket API, process supervision
│                       for Jan.ai + the voice bridge.
├── ui/               Rust (gtk4-rs). protogen-ui -- avatar + chat window.
├── voice_bridge/     Python. Persistent Whisper STT + Piper TTS worker,
│                       spoken to over stdin/stdout JSON by the daemon.
├── installer/        sh. Distro detection, package installs, systemd
│                       --user service, theme installer.
├── theme/            KDE Plasma color scheme + wallpaper (unchanged from
│                       the original ProtogenOS theme).
└── docs/             Jan.ai setup, voice tool setup.
```

Language split: **Rust** for everything that needs to be fast, resident in
memory, or handle untrusted input safely (the daemon, the executor, the
UI) -- **Python** only where it genuinely earns its keep (Whisper/Piper
have first-class Python bindings and this isn't a hot path) -- **sh** for
the installer, matching what you said you're most comfortable reading and
maintaining.

## How a request flows through the system

1. You type or say something. Voice goes through Whisper first and enters
   the exact same pipeline as typed text from that point on -- there is no
   separate, less-checked "voice fast path."
2. **Known app launch** ("open firefox") resolves directly against the
   live `.desktop` file index and either focuses an existing Firefox window
   (via kdotool, falling back to wmctrl) or launches a new one. No LLM call.
3. **Known phrase** (anything approved before) is looked up in the SQLite
   memory bank and run instantly.
4. **Unknown request** triggers: a couple of web searches for grounding
   (e.g. correct package/unit names for your distro) → Jan.ai/DeepSeek is
   asked for a plan, constrained to the closed step vocabulary → the plan
   is shown in the UI as a card listing exactly what will run → you approve
   or cancel. Only `launch_or_focus`/`utility` steps ever skip this
   confirmation step; everything else (package installs, systemctl,
   power actions, config writes) always waits for you.
5. Once approved, the plan is remembered under the phrase you used, so
   asking again is instant next time.

Example: *"ProtoOS install Hyperland and reboot"* → daemon doesn't
recognize it → researches Hyprland's Arch package name → Jan.ai proposes a
plan → you see:

```
Install via Pacman: hyprland
Reboot the machine
```

→ you approve → it runs → next time you say the same thing, it skips
straight to that plan (still shown, since it's still system-changing) with
no research step.

## Installing

```
git clone <this repo>
cd protogenos
./installer/install_assistant.sh    # the assistant: daemon, UI, voice, Jan.ai wiring
```

That's the only script you need. It never touches your KDE theme, color
scheme, icon theme, or wallpaper.

There is a **separate, optional** `installer/install_theme.sh` for the
original ProtogenOS Plasma color scheme + wallpaper from `theme/`. It is
not run by `install_assistant.sh`, not run by the daemon, and not
something the assistant will ever invoke on its own -- the assistant's
system prompt explicitly instructs the model never to propose a theme/
appearance change unless you specifically ask for one, and even then it
would show you the exact change as a plan you'd have to approve first (see
`SetConfig` in `plan-types/src/lib.rs`). Run `install_theme.sh` by hand,
once, only if you actually want that look applied -- it will ask for
confirmation before doing anything.

The installer detects your distro (`/etc/os-release`) and installs the
right packages automatically for Arch/CachyOS/Manjaro (pacman + AUR),
Fedora (dnf), Debian/Ubuntu (apt), and openSUSE (zypper). Two tools
(kdotool, piper-tts) don't have packages on every distro -- see
[docs/VOICE_SETUP.md](docs/VOICE_SETUP.md) for the two-command manual
install on distros without a repo package. See
[docs/JAN_SETUP.md](docs/JAN_SETUP.md) for pulling the DeepSeek model into
Jan.ai the first time.

Everything backing the assistant -- Jan.ai's server, the Whisper/Piper
worker -- is started **by** `protogen-daemon` itself as soon as it launches
(via a `systemd --user` service the installer sets up), so you never start
any model or backend process by hand. Just run:

```
protogen-ui
```

to open the assistant window, whenever you want it.

## Editing what it knows

- **New utility actions** (volume/brightness/etc-style toggles): add to
  `cmdrunner/src/lib.rs`'s `utility_command()` table and
  `daemon/src/dispatcher.rs`'s `utility_names()` list. These are
  code-defined on purpose (not a user-editable JSON file), since they run
  without confirmation.
- **Personality/tone**: `~/.config/protogenos/profile.json` (created from
  the default in `daemon/src/personality.rs` if absent):
  ```json
  { "name": "Protogen", "tone": "..." }
  ```
- **Forgetting a learned command**: say "forget install hyperland and
  reboot" (or whatever phrase) -- routed to the daemon's `Forget` message,
  which removes it from the memory bank so it goes through research again
  next time.
- **Avatar art**: drop `idle.png` / `listening.png` / `thinking.png` /
  `speaking.png` into `~/.local/share/protogenos/avatar/`.

## Status / honesty note

This is a substantial rewrite from the original Python prototype into a
multi-crate Rust + Python project, built without access to a Rust
toolchain in the environment it was written in -- it has been carefully
hand-reviewed for type/borrow correctness but **has not been compiled**.
Expect to run `cargo build --release --workspace` and fix a handful of real
compiler errors on first build; the architecture and security boundary are
the parts to trust, the exact syntax in a few files (especially
`ui/src/client.rs`'s threading and the gtk4-rs API surface) is the part
most likely to need small fixes against whatever gtk4-rs version actually
resolves.

## What this deliberately does NOT do

It does not give the LLM a way to run arbitrary shell commands, no matter
how the request is phrased ("run whatever you think is needed", "just use
sudo", etc.) -- that vocabulary boundary in `plan-types` is fixed in code,
not configurable, and is the one thing in this project not meant to be
loosened.

## what are the big ai models/tools used in this project that run off your device if you chose to install this project for yourself

[jan.ai](https://www.jan.ai/), [deepseekV4](https://www.deepseek.com/en/), [piper](https://github.com/rhasspy/piper), [whisper](https://github.com/openai/whisper)

## credits

[nyarch-assistant](https://github.com/NyarchLinux/NyarchAssistant) for the insparation of the idea, the many "i made jarvis" videos on instagram

# this project has only been tested on guardia arch with kde plasma
