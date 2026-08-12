# ProtogenOS

### small warning that this project has only been tested on guardia arch with kde plasma and is **not** compatible with windows at all during this stage
but it should work on fedora, debian, ubuntu, most arch distributions and openSUSE, this goes into more detail in the "how to install" part of the README
this project assumes you have python and rust installed before compiling refer to the [install rust forum](https://rust-lang.org/tools/install/) and the [python download](https://www.python.org/downloads/) pages

A local, voice-and-text controllable system assistant for Linux, with a
GTK4 avatar UI in the spirit of [Nyarch Assistant](https://github.com/NyarchLinux/NyarchAssistant),
backed by [Jan.ai](https://www.jan.ai/) running a small local model (Qwen3.5
4B by default -- see the note on model choice below), offline
Whisper speech-to-text, and offline Piper speech output.

It can open/focus applications by voice or text, control common system
settings (volume, brightness, lock, screenshot), and -- for anything it
doesn't already know how to do -- research the request, build a concrete
step-by-step plan, show you exactly what it intends to run, and only touch
your system after you approve it. Approved plans are remembered, so the
next time you ask for the same thing it's instant.

## A note on "DeepSeek"

Earlier drafts of this project targeted a full DeepSeek model as the
backend. In practice, current DeepSeek releases are 200B-670B+ parameter
models that need 100GB+ of disk and 64GB+ of RAM even at aggressive
quantization -- not something a normal laptop, including one with
integrated graphics, can run. **The default model is Qwen3.5 4B**, a small
model that actually fits consumer hardware and runs entirely on CPU. See
[docs/JAN_SETUP.md](docs/JAN_SETUP.md) for the full explanation and for how
to point this at a real DeepSeek quant if you have a machine that can
actually run one.

## Why this is safe to run

The single hard rule this whole project is built around: **the AI model
never gets to run arbitrary shell text.** It can only ever propose steps
from a small, fixed, code-defined vocabulary (launch/focus an app, close a
window, run a named binary in a terminal, install a named package, toggle
a named systemd unit, a handful of utility actions, a power action, a
scoped config write) -- see `plan-types/src/lib.rs` for the exact list.
Every step is validated against that vocabulary before it's even shown to
you, and any step outside "open an app" or "toggle a utility" always stops
and asks for your explicit confirmation first, regardless of how you
phrased the request. There is no code path, anywhere in this project, that
hands the model's output to a shell interpreter.

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
(e.g. correct package/unit names for your distro) → Jan.ai is asked
for a plan, constrained to the closed step vocabulary, narrating its
reasoning as it goes → the plan is shown in the UI as a card listing
exactly what will run → you approve or cancel. Only
`launch_or_focus`/`utility` steps ever skip this confirmation step;
everything else (package installs, systemctl, power actions, config
writes) always waits for you.
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

## Editing what it knows

- **New utility actions** (volume/brightness/etc-style toggles): add to
`cmdrunner/src/lib.rs`'s `utility_command()` table and
`daemon/src/dispatcher.rs`'s `utility_names()` list. These are
code-defined on purpose (not a user-editable JSON file), since they run
without confirmation.
- **Personality/tone**: `~/.config/protogenos/profile.json` (created from
the default in `daemon/src/personality.rs` if absent):
  ```
  { "name": "Protogen", "tone": "..." }
  ```
- **Forgetting a learned command**: say "forget install hyperland and
reboot" (or whatever phrase) -- routed to the daemon's `Forget` message,
which removes it from the memory bank so it goes through research again
next time.
- **Avatar art**: drop `idle.png` / `listening.png` / `thinking.png` /
`speaking.png` into `~/.local/share/protogenos/avatar/`.

## What this deliberately does NOT do and will NEVER be able to

It does not give the LLM a way to run arbitrary shell commands, no matter
how the request is phrased ("run whatever you think is needed", "just use
sudo", etc.) -- that vocabulary boundary in `plan-types` is fixed in code,
not configurable, and is the one thing in this project not meant to be
loosened.
With internet access, if you ask for something the assistant doesn't
already know how to do, it researches the command/package/steps needed,
builds a plan, and shows you exactly what it intends to do -- asking for
your confirmation before actually doing it. Nothing it hasn't seen before
runs without you approving it first.

## what are the big ai models/tools used in this project that run off your device if you chose to install this project for yourself

[jan.ai](https://www.jan.ai/), [Qwen3.5](https://qwenlm.github.io/) (default model, see the DeepSeek note above), [piper](https://github.com/rhasspy/piper), [whisper](https://github.com/openai/whisper)

everything required, including the default AI model, is installed and
downloaded automatically by the quick install command -- Jan.ai's CLI is
downloaded and set up, and the default model is pulled during install, so
by the time you run `protogen-ui` for the first time it's already there.
This does mean the install command downloads a few GB and can take a
while depending on your connection.

## credits

[nyarch-assistant](https://github.com/NyarchLinux/NyarchAssistant) for the insparation of the idea, the many "i made jarvis" videos on instagram

## customization

the avatar folder has places you can put the avatars frames as png as atm it animates in pngtuber style(this will change to be an actual 2d rig like vtubers)

## a slight summary of what the assistant is capable of

**open [app]** --> checks whether the app is already open (even if
minimized or just not focused) and, if so, brings that window to the
front instead of opening a duplicate. If it's not already open, it
searches your system's app launcher folder (`.desktop` files) for a
matching shortcut and launches it from there.

**close [app]** --> finds the window for the named app and closes it.

**run [binary] in a new console/terminal window** --> opens a terminal
and runs the given command inside it, so you can watch its output live.

**volume up / volume down / mute** --> adjusts system audio via PipeWire.

**brightness up / brightness down** --> adjusts screen brightness.

**lock screen** --> locks your session.

**screenshot** --> takes a screenshot.

**install [package]** --> installs a package through your distro's actual
package manager (pacman/apt/dnf/zypper, or an AUR helper on Arch-based
systems), after showing you exactly what it's about to run and asking for
confirmation first.

**remove [package]** --> same as above, in reverse.

**start/stop/restart/enable/disable [service]** --> manages a systemd
service, again always shown as a plan and confirmed before it runs.

**reboot / shutdown / suspend** --> power actions, always confirmed
first, no exceptions.

**"forget [phrase]"** --> removes a previously learned command from
memory, so the next time you ask for it, it gets re-researched instead
of using the old cached plan.

**remembers what you approve** --> once you approve a plan for something
it didn't already know, it remembers that exact request so next time you
ask, it skips straight to running it instead of researching it again.

**understands shortened/casual phrasing** --> "libre office" resolves to
LibreOffice Writer, "the browser" resolves to whatever browser is
installed, and so on -- it matches against the closest real app by
meaning, not exact spelling for when using writen mode instead of voice controls.

# how to install

open a terminal to whatever dir you want the project folder and run the command below into it

```
# cloans this repo to the place you had your terminal located before CD'ing into it
git clone https://github.com/fezza315/protogen-OS-assistant
cd protogen-OS-assistant
# compiling with rust before running the bash installer to install the code for the commands to ~/.local/share/protogenos
cargo build --release --workspace
bash installer/install_assistant.sh
```

wait for it to compile and install dependencies and once complete
to run the assistant is just

```
protogen-ui
```

if this command doesn't load the ui app for the assistant try rebooting and trying the launch command again, if any other problems occur feel free to inform me through the issues tab in this github page

The installer detects your distro (`/etc/os-release`) and installs the
right packages automatically for Arch/CachyOS/Manjaro/guardia (pacman + AUR),
Fedora (dnf), Debian/Ubuntu (apt), and openSUSE (zypper). Two tools
(kdotool, piper-tts) don't have packages on every distro -- see [docs/VOICE_SETUP.md](docs/VOICE_SETUP.md) for the two-command manual
install on distros without a repo package. See [docs/JAN_SETUP.md](docs/JAN_SETUP.md) for more detail on the default model and how to switch to a bigger one if you have the hardware for it.

Everything backing the assistant -- Jan.ai's server, the Whisper/Piper
worker -- is started **by** `protogen-daemon` itself as soon as it launches
(via a `systemd --user` service the installer sets up), so you never start
any model or backend process by hand. it is ran with the primary start command

## this project does not directly install anything outside of the user home dir except requirement packages for python etc

## this does run completely offline and does not need you to manually install dependencies(if you use the quick install command)

### note on specifically `install_theme.sh`
Earlier versions of this script had a tendency to crash plasmashell when
restarting it to apply the new wallpaper/color scheme. That's been fixed
-- the script now checks whether plasmashell actually restarted and tells
you if it didn't, instead of just leaving your desktop gone. If you ever
do end up without a panel/desktop after running it, run: `kstart plasmashell` in a terminal(open with ctrl+alt+T) and it will launch plasma-shell so your panel, desktop etc will come back
also atm the theme isnt what i plan it to be in the end, i want to get the wallpaper commissioned, find a more fitting theme. the only reason it is here now is because i plan to make the assistant part of plasmashell as something that goes on your panel as a widget or item that sits atop it.

# final note

this is 100% not a final build and **is not complete** as written in the description. i am still working on this and im still new to coding and development to this scale, this is my first bigger project using anything but python, shell and js
also i would be very grateful for tips, suggestions or issues youve had in the issues tab and i will be setting up a google form that you can actually fill out in the near future(within the coming days)
