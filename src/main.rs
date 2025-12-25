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
    #[serde(default)]
    mappings: HashMap<String, HashMap<String, String>>,
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

fn parse_led(s: &str) -> Option<LedCode> {
    let normalized = s.to_uppercase().trim_start_matches("LED_").to_string();
    let led_name = format!("LED_{}", normalized);

    for code in 0u16..=15 {
        let led = LedCode(code);
        if format!("{:?}", led).eq_ignore_ascii_case(&led_name) {
            return Some(led);
        }
    }
    None
}

fn parse_toggle(s: &str) -> Result<(Vec<KeyCode>, KeyCode), Box<dyn Error>> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();

    if parts.is_empty() {
        return Err("Empty toggle".into());
    }

    let key = parse_keycode(parts.last().unwrap()).ok_or("Invalid key in toggle combo")?;

    let modifiers: Result<Vec<_>, Box<dyn Error>> = parts[..parts.len() - 1]
        .iter()
        .map(|part| parse_keycode(part).ok_or_else(|| format!("Invalid modifier: {}", part).into()))
        .collect();

    Ok((modifiers?, key))
}

fn parse_condition(s: &str) -> Option<(LedCode, bool)> {
    if let Some(led_name) = s.strip_suffix("_on") {
        parse_led(led_name).map(|led| (led, true))
    } else if let Some(led_name) = s.strip_suffix("_off") {
        parse_led(led_name).map(|led| (led, false))
    } else {
        None
    }
}

struct MappingRule {
    from: KeyCode,
    to: KeyCode,
    led_conditions: Vec<(LedCode, bool)>,
}

impl MappingRule {
    fn matches(&self, key: KeyCode, leds: &[LedCode]) -> bool {
        if self.from != key {
            return false;
        }

        self.led_conditions
            .iter()
            .all(|(led, should_be_on)| leds.contains(led) == *should_be_on)
    }
}

struct KeyRemapper {
    virtual_kbd: VirtualDevice,
    enabled: bool,
    held_keys: HashMap<KeyCode, bool>,
    leds: Vec<LedCode>,
    toggle_mods: Vec<KeyCode>,
    toggle_key: KeyCode,
    rules: Vec<MappingRule>,
}

impl KeyRemapper {
    fn new(template: &Device, config: &Config) -> Result<Self, Box<dyn Error>> {
        #[allow(deprecated)]
        let mut virt_kbd = VirtualDeviceBuilder::new()?.name("rk");

        if let Some(keys) = template.supported_keys() {
            virt_kbd = virt_kbd.with_keys(&keys)?;
        }

        let leds = template.get_led_state()?.into_iter().collect();
        let (toggle_mods, toggle_key) = parse_toggle(&config.toggle)?;

        let mut rules = Vec::new();

        for (section, mappings) in &config.mappings {
            let led_conditions = if section == "default" {
                vec![]
            } else {
                section.split('.').filter_map(parse_condition).collect()
            };

            for (from, to) in mappings {
                match (parse_keycode(from), parse_keycode(to)) {
                    (Some(from_key), Some(to_key)) => {
                        rules.push(MappingRule {
                            from: from_key,
                            to: to_key,
                            led_conditions: led_conditions.clone(),
                        });
                    }
                    _ => {
                        eprintln!(
                            "Warning: Invalid mapping in [mappings.{}]: {} -> {}",
                            section, from, to
                        );
                    }
                }
            }
        }

        Ok(Self {
            virtual_kbd: virt_kbd.build()?,
            enabled: false,
            held_keys: HashMap::new(),
            leds,
            toggle_mods,
            toggle_key,
            rules,
        })
    }

    fn is_toggle_pressed(&self, key: KeyCode) -> bool {
        key == self.toggle_key
            && self
                .toggle_mods
                .iter()
                .all(|m| self.held_keys.get(m).copied().unwrap_or(false))
    }

    fn update_led(&mut self, key: KeyCode) {
        let led = match key {
            KeyCode::KEY_NUMLOCK => LedCode::LED_NUML,
            KeyCode::KEY_CAPSLOCK => LedCode::LED_CAPSL,
            KeyCode::KEY_SCROLLLOCK => LedCode::LED_SCROLLL,
            _ => return,
        };

        if self.leds.contains(&led) {
            self.leds.retain(|&l| l != led);
        } else {
            self.leds.push(led);
        }
    }

    fn remap_key(&self, key: KeyCode) -> Option<KeyCode> {
        if !self.enabled {
            return None;
        }

        self.rules
            .iter()
            .find(|r| r.matches(key, &self.leds))
            .map(|r| r.to)
    }

    fn process_event(&mut self, event: &InputEvent) -> Result<(), Box<dyn Error>> {
        if let EventSummary::Key(_, key, value) = event.destructure() {
            if value == 1 {
                self.update_led(key);
            }

            if value == 1 || value == 2 {
                self.held_keys.insert(key, true);
            } else if value == 0 {
                self.held_keys.insert(key, false);
            }

            if value == 1 && self.is_toggle_pressed(key) {
                self.enabled = !self.enabled;
                self.notify();
                return Ok(());
            }

            let event_to_emit = self
                .remap_key(key)
                .map(|remapped| InputEvent::new(EventType::KEY.0, remapped.0, value))
                .unwrap_or(*event);
            self.virtual_kbd.emit(&[event_to_emit])?;
        } else {
            self.virtual_kbd.emit(&[*event])?;
        }
        Ok(())
    }

    fn notify(&self) {
        let (msg, beep) = if self.enabled {
            ("Enabled", "\x07\x07")
        } else {
            ("Disabled", "\x07")
        };

        if let (Ok(user), Ok(uid)) = (var("SUDO_USER"), var("SUDO_UID")) {
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
    let mut remapper = KeyRemapper::new(&keyboards[0], &config)?;
    keyboards.iter_mut().try_for_each(|kb| kb.grab())?;

    println!("Loaded {} mapping rules", remapper.rules.len());
    println!("Press {} to toggle remapping", config.toggle);

    loop {
        for kb in &mut keyboards {
            if let Ok(mut events) = kb.fetch_events() {
                events.try_for_each(|e| remapper.process_event(&e))?;
            }
        }

        sleep(Duration::from_micros(100));
    }
}
