# rk

Minimal, efficient, configurable keyboard remapper for Linux

## How It Works

Uses Linux evdev to intercept & remap keys at the input device level. Works on X11, Wayland & TTY

## Features

- Config-driven key remapping
- Conditional mappings (e.g., NumLock-dependent)
- Custom toggle key combinations
- Desktop notifications on toggle
- Works on X11, Wayland, and TTY

## Install

```bash
cargo build --release

# copy to $PATH
sudo cp target/release/rk /usr/local/bin/
```

## Configuration

Create rk.toml in one of these locations:

- ./rk.toml (current directory)
- ~/.config/rk.toml (user config)
- /etc/rk.toml (system-wide)
- Set `RK_CONFIG=/path/to/config.toml`

## Example config:

```toml
# toggle modifier (single key or combination)
toggle = "ctrl+enter"

# always-active mappings
[mappings]
w = "up"
a = "left"
s = "down"
d = "right"
```

_See the [included example rk.toml](./rk.toml) for more_

_Key names: Use w, W, or KEY_W; See [evdev key codes](https://docs.rs/evdev/latest/evdev/struct.KeyCode.html)_

## Usage

```bash
sudo rk

# or if not installed to $PATH:
sudo ./target/release/rk
```

Press your configured toggle key (default: Ctrl+Enter) to enable/disable remapping

## Background Mode

```bash
# start
(sudo rk > /tmp/rk.log 2>&1 & disown && echo $! > /tmp/rk.pid)

# stop
sudo kill $(cat /tmp/rk.pid)
```
