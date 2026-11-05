//! The byte pipe between the session and the robot. One trait, two kinds of implementation: a USB
//! serial port, and (in development builds and tests) the simulator. Everything above this file
//! works on bytes only, so the same session code runs on both.
//!
//! The device on the other end is untrusted input. This file never trusts a port name from the
//! webview: a port is opened only if it appears in a fresh listing of USB serial ports.

use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

use serde::Serialize;

/// Espressif's USB Serial/JTAG controller, which is what the ESP32-S3's native USB port presents.
/// Many Espressif boards share it, so a match means "could be a Moby", never "is one": only the
/// handshake proves that.
pub const MOBY_VID: u16 = 0x303A;
pub const MOBY_PID: u16 = 0x1001;

const MAX_LABEL_CHARS: usize = 40;
const MAX_NAME_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// The device is gone or the port failed. The link is over.
    Closed,
}

pub trait Transport: Send {
    /// Reads what has arrived. Returns `Ok(0)` when nothing came within `wait`. Real ports block
    /// for up to `wait`; the simulator never does.
    fn read(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, TransportError>;
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    /// Matches the USB IDs a Moby presents.
    Moby,
    /// Any other USB serial device, offered only as a manual choice.
    OtherUsb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortInfo {
    pub name: String,
    pub kind: PortKind,
    /// Text for people, cleaned: the device chose it.
    pub label: String,
    pub simulated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// Not in the current listing, or unplugged since.
    NotFound,
    /// Another program holds the port.
    Busy,
    Failed,
}

pub trait Connector: Send {
    /// USB serial ports that exist now. Never fails: an error counts as an empty list.
    fn ports(&mut self) -> Vec<PortInfo>;
    fn open(&mut self, name: &str) -> Result<Box<dyn Transport>, OpenError>;
}

/// Keeps printable characters and cuts the text short. A device name is untrusted.
pub fn clean_label(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "USB serial device".to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// A port name the webview may pass on: short, plain ASCII. It still has to be in the listing.
pub fn plausible_port_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn kind_of(vid: u16, pid: u16) -> PortKind {
    if vid == MOBY_VID && pid == MOBY_PID {
        PortKind::Moby
    } else {
        PortKind::OtherUsb
    }
}

pub struct SerialConnector;

impl Connector for SerialConnector {
    fn ports(&mut self) -> Vec<PortInfo> {
        let Ok(found) = serialport::available_ports() else {
            return Vec::new();
        };
        let mut ports: Vec<PortInfo> = found
            .into_iter()
            .filter_map(|port| match port.port_type {
                serialport::SerialPortType::UsbPort(usb)
                    if plausible_port_name(&port.port_name) =>
                {
                    Some(PortInfo {
                        kind: kind_of(usb.vid, usb.pid),
                        label: clean_label(usb.product.as_deref().unwrap_or("")),
                        name: port.port_name,
                        simulated: false,
                    })
                }
                _ => None,
            })
            .collect();
        ports.sort_by(|a, b| a.name.cmp(&b.name));
        ports
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn Transport>, OpenError> {
        // Only a port that is in the listing right now can be opened.
        if !plausible_port_name(name) || !self.ports().iter().any(|port| port.name == name) {
            return Err(OpenError::NotFound);
        }
        let port = serialport::new(name, 115_200)
            .timeout(Duration::from_millis(20))
            // Not asserting DTR on open: some host sequences on DTR and RTS reset an ESP32.
            // Whether the robot needs it is a hardware question, see the ADR.
            .dtr_on_open(false)
            .open()
            .map_err(|error| match error.kind() {
                serialport::ErrorKind::NoDevice => OpenError::NotFound,
                serialport::ErrorKind::Io(ErrorKind::PermissionDenied) => OpenError::Busy,
                _ => OpenError::Failed,
            })?;
        Ok(Box::new(SerialTransport { port }))
    }
}

struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl Transport for SerialTransport {
    fn read(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, TransportError> {
        self.port
            .set_timeout(wait.max(Duration::from_millis(1)))
            .map_err(|_| TransportError::Closed)?;
        match self.port.read(buf) {
            Ok(count) => Ok(count),
            Err(error) if error.kind() == ErrorKind::TimedOut => Ok(0),
            Err(_) => Err(TransportError::Closed),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.port
            .write_all(data)
            .and_then(|()| self.port.flush())
            .map_err(|_| TransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_moby_usb_ids_count_as_a_likely_robot() {
        assert_eq!(kind_of(0x303A, 0x1001), PortKind::Moby);
        assert_eq!(kind_of(0x303A, 0x1002), PortKind::OtherUsb);
        assert_eq!(kind_of(0x0403, 0x6001), PortKind::OtherUsb);
    }

    #[test]
    fn port_names_from_the_page_must_be_plain_and_short() {
        assert!(plausible_port_name("COM3"));
        assert!(plausible_port_name("SIMULATOR"));
        for bad in [
            "",
            "\\\\.\\COM3",
            "COM3 ",
            "C:\\x",
            "../x",
            "CON:",
            &"A".repeat(33),
        ] {
            assert!(!plausible_port_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn device_labels_lose_control_characters_and_are_cut() {
        assert_eq!(clean_label("USB\u{1b}[31m JTAG"), "USB[31m JTAG");
        assert_eq!(clean_label("\u{0}\n"), "USB serial device");
        assert_eq!(clean_label(&"x".repeat(500)).chars().count(), 40);
    }
}
