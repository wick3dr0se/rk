use serde::Deserialize;
use std::collections::HashMap;
use std::env::var;
use std::error::Error;
use std::fs::{read_dir, read_to_string};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{Device, EventSummary, EventType, InputEvent, KeyCode, LedCode};

#[derive(Deserialize)]
struct Config {
    toggle: String,
    mappings: Mappings,
}

#[derive(Deserialize)]
struct Mappings {
    #[serde(flatten)]
    regular: HashMap<String, String>,
    #[serde(default)]
    numlock_off: HashMap<String, String>,
}

impl Config {
    fn load() -> Result<Self, Box<dyn Error>> {
        let paths = [
            var("RK_CONFIG").ok(),
            Some("rk.toml".into()),
            var("HOME").ok().map(|h| format!("{}/.config/rk.toml", h)),
            Some("/etc/rk.toml".into()),
        ];

        for path in paths.iter().flatten() {
            if let Ok(content) = read_to_string(path) {
                println!("Loaded config: {}", path);
                return Ok(toml::from_str(&content)?);
            }
        }

        Err(
            "No config file found. Checked: $RK_CONFIG, ./rk.toml, ~/.config/rk.toml, /etc/rk.toml"
                .into(),
        )
    }
}

struct ToggleMod {
    modifiers: Vec<KeyCode>,
    key: KeyCode,
}

impl ToggleMod {
    fn parse(s: &str) -> Result<Self, Box<dyn Error>> {
        let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();

        if parts.is_empty() {
            return Err("Empty toggle combo".into());
        }

        let key = parse_keycode(parts.last().unwrap()).ok_or("Invalid key in toggle combo")?;

        let mut modifiers = Vec::new();
        for part in &parts[..parts.len() - 1] {
            let modifier = parse_keycode(part).ok_or(format!("Invalid modifier: {}", part))?;
            modifiers.push(modifier);
        }

        Ok(Self { modifiers, key })
    }

    fn is_pressed(&self, key: KeyCode, held_keys: &HashMap<KeyCode, bool>) -> bool {
        // check if this is the trigger key and all modifiers are held
        if key != self.key {
            return false;
        }

        self.modifiers
            .iter()
            .all(|m| held_keys.get(m).copied().unwrap_or(false))
    }
}

fn parse_keycode(s: &str) -> Option<KeyCode> {
    let normalized = s.to_uppercase().trim_start_matches("KEY_").to_string();
    let key_name = format!("KEY_{}", normalized);

    for code in 0u16..=767 {
        let keycode = KeyCode(code);
        if format!("{:?}", keycode).eq_ignore_ascii_case(&key_name) {
            return Some(keycode);
        }
    }
    None
}

struct KeyRemapper {
    virtual_kbd: VirtualDevice,
    active: bool,
    numlocked: bool,
    held_keys: HashMap<KeyCode, bool>,
    toggle_mod: ToggleMod,
    mappings: HashMap<KeyCode, KeyCode>,
    numlock_mappings: HashMap<KeyCode, KeyCode>,
}

impl KeyRemapper {
    fn new(template: &Device, config: &Config) -> Result<Self, Box<dyn Error>> {
        #[allow(deprecated)]
        let mut virt_kbd = VirtualDeviceBuilder::new()?.name("rk");

        // clone all supported keys so everything passes through
        if let Some(keys) = template.supported_keys() {
            virt_kbd = virt_kbd.with_keys(&keys)?;
        }

        let numlocked = template.get_led_state()?.contains(LedCode::LED_NUML);

        let mut mappings = HashMap::new();
        for (from, to) in &config.mappings.regular {
            if let (Some(from_key), Some(to_key)) = (parse_keycode(from), parse_keycode(to)) {
                mappings.insert(from_key, to_key);
            } else {
                eprintln!("Warning: Invalid mapping {} -> {}", from, to);
            }
        }

        let mut numlock_mappings = HashMap::new();
        for (from, to) in &config.mappings.numlock_off {
            if let (Some(from_key), Some(to_key)) = (parse_keycode(from), parse_keycode(to)) {
                numlock_mappings.insert(from_key, to_key);
            } else {
                eprintln!("Warning: Invalid numlock mapping {} -> {}", from, to);
            }
        }

        Ok(Self {
            virtual_kbd: virt_kbd.build()?,
            active: false,
            numlocked,
            held_keys: HashMap::new(),
            toggle_mod: ToggleMod::parse(&config.toggle)?,
            mappings,
            numlock_mappings,
        })
    }

    fn remap_key(&self, key: KeyCode) -> Option<KeyCode> {
        if !self.active {
            return None;
        }

        if !self.numlocked {
            if let Some(&remapped) = self.numlock_mappings.get(&key) {
                return Some(remapped);
            }
        }

        self.mappings.get(&key).copied()
    }

    fn process_event(&mut self, event: &InputEvent) -> Result<(), Box<dyn Error>> {
        if let EventSummary::Key(_, key, value) = event.destructure() {
            // track all key states for combo detection
            if value == 1 {
                self.held_keys.insert(key, true);
            } else if value == 0 {
                self.held_keys.insert(key, false);
            }

            if key == KeyCode::KEY_NUMLOCK && value == 1 {
                self.numlocked = !self.numlocked;
            }

            // check for toggle combo
            if value == 1 && self.toggle_mod.is_pressed(key, &self.held_keys) {
                self.active = !self.active;
                self.notify();
                return Ok(());
            }

            // emit remapped key or pass through unchanged
            let event_to_emit = self
                .remap_key(key)
                .map(|remapped| InputEvent::new(EventType::KEY.0, remapped.0, value))
                .unwrap_or(*event);
            self.virtual_kbd.emit(&[event_to_emit])?;
        } else {
            // pass through non-key events (sync, etc)
            self.virtual_kbd.emit(&[*event])?;
        }
        Ok(())
    }

    fn notify(&self) {
        let (msg, beep) = if self.active {
            ("Enabled", "\x07\x07")
        } else {
            ("Disabled", "\x07")
        };

        if let (Ok(user), Ok(uid)) = (var("SUDO_USER"), var("SUDO_UID")) {
            // try notify-send, beep if it fails
            if Command::new("sudo")
                .args([
                    "-u",
                    &user,
                    "sh",
                    "-c",
                    &format!(
                        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{}/bus notify-send -t 1500 'rk' '{}'",
                        uid, msg
                    ),
                ])
                .spawn()
                .is_err()
            {
                print!("{}", beep);
            }
        }
    }
}

fn find_keyboards() -> Result<Vec<Device>, Box<dyn Error>> {
    let mut keyboards = Vec::new();

    for entry in read_dir("/dev/input")? {
        let path = entry?.path();

        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.starts_with("event"))
        {
            continue;
        }

        if let Ok(dev) = Device::open(&path) {
            // check if keyboard if it has alpha A & doesn't have mouse left
            if dev.supported_keys().map_or(false, |keys| {
                keys.contains(KeyCode::KEY_A) && !keys.contains(KeyCode::BTN_LEFT)
            }) {
                println!("Found: {} ({:?})", dev.name().unwrap_or("Unknown"), path);
                keyboards.push(dev);
            }
        }
    }

    keyboards
        .is_empty()
        .then(|| Err("No keyboards found".into()))
        .unwrap_or(Ok(keyboards))
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::load()?;
    let mut keyboards = find_keyboards()?;
    // create virtual device that mimics the first keyboard
    let mut remapper = KeyRemapper::new(&keyboards[0], &config)?;
    // grab exclusive access to prevent duplicate events
    keyboards.iter_mut().try_for_each(|kb| kb.grab())?;

    println!("Press {} to toggle", config.toggle);

    loop {
        for kb in &mut keyboards {
            if let Ok(mut events) = kb.fetch_events() {
                events.try_for_each(|e| remapper.process_event(&e))?;
            }
        }

        sleep(Duration::from_micros(100));
    }
}
