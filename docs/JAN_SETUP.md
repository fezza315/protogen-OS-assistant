# Jan.ai + DeepSeek setup

ProtogenOS's planner (the part that turns "install Hyperland and reboot"
into a concrete, reviewable plan) is backed by a local Jan.ai server
(https://www.jan.ai/) running a DeepSeek model. protogen-daemon starts and
manages the Jan.ai server process itself -- you never run `jan serve`
by hand.

## One-time setup

1. Install Jan.ai. The simplest path is the AppImage/installer from
   https://www.jan.ai/ -- this also installs the `jan` CLI binary that
   protogen-daemon looks for on PATH.

2. Pull a DeepSeek model inside Jan (via the Jan UI's model hub, or the CLI):
   ```
   jan models pull deepseek-v4
   ```
   If `deepseek-v4` isn't the exact model name Jan lists it under when you
   check, pick the closest current DeepSeek model in Jan's hub and set:
   ```
   # in ~/.config/systemd/user/protogenos-daemon.service, under [Service]:
   Environment=PROTOGEN_JAN_MODEL=<exact-model-name-from-jan>
   ```
   then `systemctl --user daemon-reload && systemctl --user restart protogenos-daemon`.

3. That's it. protogen-daemon will:
   - launch `jan serve --host 127.0.0.1 --port 1337 --data-dir ... --model ...`
     as a supervised background process on its own startup,
   - wait for it to become ready,
   - and restart it automatically if it ever crashes.

## Checking it's working

```
journalctl --user -u protogenos-daemon -f
```
should show `Jan.ai server ready at http://127.0.0.1:1337` shortly after
the daemon starts. If it instead logs a warning about Jan.ai not becoming
ready, check that `jan` is genuinely on PATH for the systemd user session
(systemd user services don't always inherit your interactive shell's PATH
-- if needed, add an explicit `Environment=PATH=...` line or set
`PROTOGEN_JAN_BINARY=/full/path/to/jan` in the unit file).

## Changing the port

Default is 1337. Override with `PROTOGEN_JAN_PORT` in the systemd unit if
that port is already in use on your machine.
