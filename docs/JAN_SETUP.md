# Jan.ai model setup

ProtogenOS's planner (the part that turns "install Hyperland and reboot"
into a concrete, reviewable plan) is backed by a local Jan.ai server
(https://www.jan.ai/). `protogen-daemon` starts and manages the `jan`
process itself via `jan serve` -- you never run it by hand.

## A note on model choice, read this first

The original idea for this project was to run a full DeepSeek model as the
backend. **That doesn't work on a laptop.** Current DeepSeek releases
(V3, V3.1, V4) are 200B-670B+ parameter Mixture-of-Experts models --
even at aggressive quantization they need 100GB+ of disk and 64GB+ of
RAM just to load, and are not something an integrated-graphics laptop
like a ProBook can run at a usable speed, or in some cases run at all.

So by default, ProtogenOS uses a small model that actually fits
consumer laptop hardware: **Qwen3.5 4B** (`qwen3.5-4b`), a ~4 billion
parameter model that runs entirely on CPU at a few GB of RAM and gives
usable response times on hardware like an i5 with integrated graphics.
It's noticeably less capable than a frontier model, but the planner's job
here is narrow (map a request to a small closed set of step types), which
small models handle reasonably well.

If you have a machine with 64GB+ RAM (or a strong discrete GPU with tens
of GB of VRAM), you can point ProtogenOS at a real DeepSeek quant instead
-- see "Using a bigger model" below.

## One-time setup (default: Qwen3.5 4B)

1. Install Jan.ai. On Linux the supported path is the AppImage from
   https://github.com/janhq/jan/releases/latest -- download it, make it
   executable, and run it once:
   ```
   chmod +x Jan-*.AppImage
   ./Jan-*.AppImage
   ```
   Launching it once installs the `jan` CLI binary (to `/usr/local/bin/jan`
   if writable, otherwise `~/.local/bin/jan` -- make sure that's on your
   `$PATH`). You can close the Jan window afterward; you don't need to
   keep it open.

2. Pull the default model once so it's cached locally:
   ```
   jan serve qwen3.5-4b
   ```
   The first run downloads the model (a few GB) and starts serving it --
   press Ctrl+C once it says it's ready. After this, `protogen-daemon`
   will start and stop it on its own; you don't run this by hand again.

3. That's it. `protogen-daemon` runs `jan serve qwen3.5-4b --port 6767` as
   a supervised background process on its own startup, waits for it to
   become ready, and restarts it automatically if it ever crashes.

## Using a bigger model (64GB+ RAM machines only)

Set `PROTOGEN_JAN_MODEL` in the systemd unit to a HuggingFace GGUF repo id
Jan can download, for example a smaller DeepSeek distill or quant that
actually fits your hardware:
```
# in ~/.config/systemd/user/protogenos-daemon.service, under [Service]:
Environment=PROTOGEN_JAN_MODEL=unsloth/DeepSeek-V3.1-GGUF
```
then:
```
systemctl --user daemon-reload
systemctl --user restart protogenos-daemon
```
Check the model's actual RAM/disk requirements on its Hugging Face page
before doing this -- full DeepSeek quants commonly need 100GB+ of disk and
will not load at all on a machine without enough RAM.

## Checking it's working

```
journalctl --user -u protogenos-daemon -f
```
should show the daemon waiting on Jan.ai and then proceeding normally
once `jan serve` reports ready. If it instead logs a warning about Jan.ai
not becoming ready, check:

- Is `jan` genuinely on PATH for the systemd user session? (systemd user
  services don't always inherit your interactive shell's PATH -- if
  needed, set `PROTOGEN_JAN_BINARY=/full/path/to/jan` in the unit file.)
- Run the exact command the daemon runs, directly, to see Jan's own error
  output: `jan serve qwen3.5-4b --port 6767`
- Was the model actually pulled once already per step 2 above?

## Changing the port

Default is `6767` (Jan CLI's own default). Override with
`PROTOGEN_JAN_PORT` in the systemd unit if that port is already in use.
