use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{Device, EventSummary, EventType, InputEvent, KeyCode};
use std::env::var;
use std::error::Error;
use std::fs::read_dir;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

struct KeyRemapper {
    virtual_kbd: VirtualDevice,
    active: bool,
    ctrl_held: bool,
}

impl KeyRemapper {
    fn new(template: &Device) -> Result<Self, Box<dyn Error>> {
        #[allow(deprecated)]
        let mut virt_kbd = VirtualDeviceBuilder::new()?.name("rk");
        // clone all supported keys so everything passes through
        if let Some(keys) = template.supported_keys() {
            virt_kbd = virt_kbd.with_keys(&keys)?;
        }

        Ok(Self {
            virtual_kbd: virt_kbd.build()?,
            active: false,
            ctrl_held: false,
        })
    }

    fn remap_key(&self, key: KeyCode) -> Option<KeyCode> {
        self.active
            .then(|| match key {
                KeyCode::KEY_W => Some(KeyCode::KEY_UP),
                KeyCode::KEY_A => Some(KeyCode::KEY_LEFT),
                KeyCode::KEY_S => Some(KeyCode::KEY_DOWN),
                KeyCode::KEY_D => Some(KeyCode::KEY_RIGHT),
                _ => None,
            })
            .flatten()
    }

    fn process_event(&mut self, event: &InputEvent) -> Result<(), Box<dyn Error>> {
        if let EventSummary::Key(_, key, value) = event.destructure() {
            if matches!(key, KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL) {
                self.ctrl_held = value != 0;
            }
            if key == KeyCode::KEY_ENTER && value == 1 && self.ctrl_held {
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
            ("WASD → Arrows", "\x07\x07")
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

        // only check event devices
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.starts_with("event"))
        {
            continue;
        }

        if let Ok(dev) = Device::open(&path) {
            // check for grave key to identify keyboards
            if dev
                .supported_keys()
                .map_or(false, |keys| keys.contains(KeyCode::KEY_GRAVE))
            {
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
    let mut keyboards = find_keyboards()?;
    // create virtual device that mimics the first keyboard
    let mut remapper = KeyRemapper::new(&keyboards[0])?;
    // grab exclusive access to prevent duplicate events
    keyboards.iter_mut().try_for_each(|kb| kb.grab())?;

    println!("Press Ctrl+Enter to toggle");

    loop {
        for kb in &mut keyboards {
            if let Ok(mut events) = kb.fetch_events() {
                events.try_for_each(|e| remapper.process_event(&e))?;
            }
        }

        sleep(Duration::from_micros(100));
    }
}
