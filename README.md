# rk

Minimal & efficient WASD → arrow key remapper for Linux

## How It Works

Uses Linux evdev to intercept & remap keys at the input device level. Works on X11, Wayland & TTY

## Install

```bash
cargo build --release

# copy to $PATH
sudo cp target/release/rk /usr/local/bin/
```

## Use

```bash
sudo rk

# or if not installed to $PATH:
sudo ./target/release/rk
```

Press **Ctrl+Enter** to toggle. **Ctrl+C** to exit

When enabled: W/A/S/D becomes ↑/←/↓/→

## Background Mode

```bash
# start
(sudo rk > /tmp/rk.log 2>&1 & disown && echo $! > /tmp/rk.pid)

# stop
sudo kill $(cat /tmp/rk.pid)
```
