use std::io;
use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::zip;
use std::rc::Rc;

use breadx::{display::DisplayConnection, prelude::*};
use breadx::protocol::xinput::{DeviceUse, GetDevicePropertyItems};
use breadx::protocol::xproto::Atom;
use futures::Stream;
use futures::stream::StreamExt;
use tokio::pin;
use tokio::process::Command;
use tokio_udev::{AsyncMonitorSocket, Device, Enumerator, MonitorBuilder};

#[derive(Clone, Debug)]
struct KeyboardInfo {
    name: String,
    device_node: String,
    xinput_id: u8,
    vendor_id: u16,
    product_id: u16,
}

struct XInputKeyboardInfo {
    name: String,
    xinput_id: u8,
}

struct UdevKeyboardInfo {
    vendor_id: u16,
    product_id: u16,
}

struct KeyboardPresenceState<'a> {
    conn: &'a RefCell<DisplayConnection>,
    // Maps from the canonical device note path to the input-device record.
    known_keyboards: HashMap<String, Rc<KeyboardInfo>>
}

impl <'a> KeyboardPresenceState<'a> {
    fn new(conn: &'a RefCell<DisplayConnection>) -> Self {
        Self {
            conn,
            known_keyboards: HashMap::new(),
        }
    }

    // Returns added keyboards
    fn update(&mut self) -> Vec<Rc<KeyboardInfo>> {
        // We get the list of keyboards from xinput, but its classification of devices as
        // keyboards is a little too broad (for example it classifies a power switch as a keyboard).
        // So we also get the list of keyboards from udev, and use that to filter the xinput list
        // into a final list of keyboards.
        let xinput_keyboards = self.get_xinput_keyboards();
        let udev_keyboards = Self::get_udev_keyboards();

        let mut keyboards = HashMap::with_capacity(xinput_keyboards.len());
        for (device_node, xinput_kbd) in xinput_keyboards.iter() {
            if let Some(udev_kbd) = udev_keyboards.get(device_node) {
                let keyboard = Rc::new(KeyboardInfo {
                    name: xinput_kbd.name.clone(),
                    device_node: device_node.clone(),
                    xinput_id: xinput_kbd.xinput_id,
                    vendor_id: udev_kbd.vendor_id,
                    product_id: udev_kbd.product_id,
                });
                keyboards.insert(device_node.clone(), keyboard);
            }
        }

        // Now we determine which ones were added or removed since the last update.
        // The added ones will be returned to the caller; the removed ones will
        // be removed from the known_keyboards map.
        let mut added = Vec::with_capacity(keyboards.len());
        for (device_node, keyboard) in keyboards.iter() {
            if !self.known_keyboards.contains_key(device_node) {
                self.known_keyboards.insert(device_node.clone(), keyboard.clone());
                added.push(keyboard.clone());
            }
        }

        self.known_keyboards.retain(|device_node, _| keyboards.contains_key(device_node));

        added
    }

    fn get_udev_keyboards() -> HashMap<String, UdevKeyboardInfo> {
        let mut keyboards = HashMap::new();

        let mut enumerator = Enumerator::new().expect("failed to create udev enumerator");
        enumerator.match_is_initialized().expect("failed to match udev devices");
        enumerator.match_subsystem("input").expect("failed to match udev devices");
        let list = enumerator.scan_devices().expect("failed to scan udev devices");
        for device in list {
            if !device_is_keyboard(&device) {
                continue;
            }
            let devnode = match device.devnode() {
                Some(devnode) => devnode.to_string_lossy().to_string(),
                None => continue,
            };
            let vendor_id = match device.property_value("ID_VENDOR_ID") {
                Some(vendor_id) => u16::from_str_radix(vendor_id.to_str().unwrap(), 16).unwrap(),
                None => continue,
            };
            let product_id = match device.property_value("ID_MODEL_ID") {
                Some(product_id) => u16::from_str_radix(product_id.to_str().unwrap(), 16).unwrap(),
                None => continue,
            };
            keyboards.insert(
                devnode.clone(),
                UdevKeyboardInfo {
                    vendor_id,
                    product_id,
                }
            );
        }

        keyboards
    }

    fn get_xinput_keyboards(&mut self) -> HashMap<String, XInputKeyboardInfo> {
        let mut conn = self.conn.borrow_mut();

        let input_devices_reply = conn.xinput_list_input_devices_immediate()
            .expect("failed to list input devices");
        let names = input_devices_reply.names;
        let devices = input_devices_reply.devices;

        let device_node_atom = conn.intern_atom_immediate(false, "Device Node")
            .expect("failed to get device-node atom")
            .atom;

        let mut result = HashMap::with_capacity(devices.len());
        for (name, device) in zip(names, devices.iter()) {
            if device.device_use != DeviceUse::IS_X_KEYBOARD && device.device_use != DeviceUse::IS_X_EXTENSION_KEYBOARD {
                continue
            }
            let xinput_device_node = xinput_get_string_property(&mut conn, device.device_id, device_node_atom);
            if xinput_device_node.is_empty() {
                continue
            }
            let name = String::from_utf8(name.name).expect("device name is not valid utf8");
            /*eprintln!("Keyboard: {}", name);
            eprintln!("  device node: {}", xinput_device_node);
            eprintln!("  xinput id: {}", device.device_id);*/
            result.insert(
                xinput_device_node,
                XInputKeyboardInfo {
                    name,
                    xinput_id: device.device_id,
                }
            );
        }

        result
    }
}

pub(crate) async fn reinit_loop(conn: &RefCell<DisplayConnection>, init_keyboard_command: &str) {
    let events = monitor_udev_input();
    let events = filter_keyboard_events(events);
    pin!(events);
    let mut presence_state = KeyboardPresenceState::new(conn);
    loop {
        let added_keyboards = presence_state.update();
        for keyboard in added_keyboards {
            init_keyboard(&keyboard, init_keyboard_command).await;
        }
        // Wait for a new keyboard to be plugged in
        if (events.next().await).is_none() {
            eprintln!("udev event stream ended, exiting");
        }
    }
}

/// Initialize a keyboard by running init_keyboard_command. The arguments to the command are:
/// - Name of the keyboard
/// - device node
/// - xinput device id
/// - vendor:product ID
async fn init_keyboard(keyboard: &KeyboardInfo, init_keyboard_command: &str) {
    let mut cmd = Command::new(init_keyboard_command);
    cmd.arg(keyboard.name.clone());
    cmd.arg(keyboard.device_node.clone());
    cmd.arg(keyboard.xinput_id.to_string());
    cmd.arg(format!("{:04x}:{:04x}", keyboard.vendor_id, keyboard.product_id));

    let status = cmd.status().await;
    match status {
        Ok(status) => {
            if !status.success() {
                eprintln!("init_keyboard_command exited with status {}", status);
            }
        }
        Err(e) => {
            eprintln!("Failed to run {}: {}", init_keyboard_command, e);
        }
    }
}

fn monitor_udev_input() -> impl Stream<Item = Result<tokio_udev::Event, io::Error>> {
    let monitor: AsyncMonitorSocket = MonitorBuilder::new()
        .expect("Couldn't create builder")
        .match_subsystem("input")
        .expect("Failed to add filter for input subsystem")
        .listen()
        .expect("Couldn't create MonitorSocket")
        .try_into()
        .expect("Couldn't convert MonitorSocket to AsyncMonitorSocket");
    monitor
}

fn filter_keyboard_events(events: impl Stream<Item = Result<tokio_udev::Event, io::Error>>) -> impl Stream<Item = tokio_udev::Event> {
    events.filter_map(|event| async {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                eprintln!("Error reading udev event: {}", e);
                return None;
            },
        };
        if !matches!(event.event_type(),
            tokio_udev::EventType::Add | tokio_udev::EventType::Remove) {
            return None;
        }

        if device_is_keyboard(&event.device()) {
            Some(event)
        } else {
            None
        }
    })
}

fn device_is_keyboard(device: &Device) -> bool {
    let input_keyboard = match device.property_value("ID_INPUT_KEYBOARD") {
        Some(value) => value == "1",
        None => false,
    };
    let input_key = match device.property_value("ID_INPUT_KEY") {
        Some(value) => value == "1",
        None => false,
    };
    let has_device_node = device.devnode().is_some();
    input_keyboard && input_key && has_device_node
}

fn xinput_get_string_property(conn: &mut DisplayConnection, device_id: u8, property_atom: Atom) -> String {
    const INITIAL_BUFFER: u32 = 256;
    let mut reply = conn.xinput_get_device_property_immediate(
        property_atom,
        u32::from(breadx::protocol::xproto::AtomEnum::STRING),
        0, // offset
        INITIAL_BUFFER, // length
        device_id,
        false, // delete
    ).expect("failed to get device node");

    if reply.length == 0 {
        return String::new();
    }
    if reply.bytes_after != 0 {
        let needed_length = reply.length + reply.bytes_after;

        reply = conn.xinput_get_device_property_immediate(
            property_atom,
            u32::from(breadx::protocol::xproto::AtomEnum::STRING),
            0, // offset
            needed_length, // length
            device_id,
            false, // delete
        ).expect("failed to get device node");
    }

    match reply.items {
        GetDevicePropertyItems::Data8(data) => {
            String::from_utf8(data).expect("device node is not valid utf8")
        }
        _ => panic!("device node is not a string"),
    }
}
