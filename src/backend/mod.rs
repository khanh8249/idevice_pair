mod discovery;
mod install;
mod link;
mod pairing;
mod usb;
mod validate;
mod wireless;
mod worker;

pub use install::InstalledApp;
pub use pairing::{PairingKind, PairingResult};
pub use worker::spawn;

use std::{
    net::IpAddr,
    sync::OnceLock,
};

use tokio::sync::mpsc::UnboundedSender;

/// Generate a unique host label for this process.
pub fn host_label() -> &'static str {
    static LABEL: OnceLock<String> = OnceLock::new();

    LABEL.get_or_init(|| {
        let id = uuid::Uuid::new_v4().simple().to_string();
        format!("idevice_pair-{}", &id[..6])
    })
}

/// Convert an error and its source chain into a readable string.
pub fn message(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();

    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }

    text
}

pub type DeviceKey = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transport {
    Usb,
    Network,
    Remote,
}

#[derive(Clone, Debug)]
pub struct DeviceSummary {
    pub key: DeviceKey,
    pub name: String,
    pub transport: Transport,
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub model: String,
    pub version: String,
    pub udid: String,
}

#[derive(Clone, Debug)]
pub struct AppleTv {
    pub name: String,
    pub(crate) address: discovery::Addresses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    WirelessDebugging,
    DeveloperMode,
}

pub enum Command {
    Inspect(DeviceKey),

    ListApps {
        key: DeviceKey,
        kind: PairingKind,
    },

    CreatePairing {
        key: DeviceKey,
        kind: PairingKind,
    },

    Validate {
        key: DeviceKey,
        ip: Option<IpAddr>,
    },

    Install {
        key: DeviceKey,
        app: InstalledApp,
    },

    StartWirelessPairing,

    PairAppleTv(AppleTv),

    SubmitWirelessPin(String),

    StopWirelessPairing,
}

pub enum Event {
    Devices(Vec<DeviceSummary>),

    AppleTvs(Vec<AppleTv>),

    UsbmuxdFailure(String),

    Info {
        key: DeviceKey,
        result: Result<DeviceInfo, String>,
    },

    Check {
        key: DeviceKey,
        check: Check,
        result: Result<bool, String>,
    },

    PairRecord {
        key: DeviceKey,
        stored: bool,
    },

    Apps {
        key: DeviceKey,
        result: Result<Vec<InstalledApp>, String>,
    },

    Progress {
        key: DeviceKey,
        message: String,
    },

    Pairing {
        key: DeviceKey,
        result: Result<PairingResult, String>,
    },

    Validation {
        key: DeviceKey,
        result: Result<(), String>,
    },

    Install {
        key: DeviceKey,
        app: String,
        result: Result<(), String>,
    },

    Wireless(WirelessStatus),
}

pub enum WirelessStatus {
    Advertising(String),
    Connected,
    EnterPin(String),
    Pin(String),
    Paired(DeviceKey),
    Failed(String),
}

/// Handle used by a frontend to send commands to the backend.
#[derive(Clone)]
pub struct Backend {
    commands: UnboundedSender<Command>,
}

impl Backend {
    pub fn send(&self, command: Command) {
        self.commands
            .send(command)
            .expect("backend command channel stopped");
    }
}

/// Event channel shared by the backend and any frontend.
///
/// This type intentionally contains no GUI-specific state.
/// CLI, GUI, TUI, daemon, or another frontend can consume these events.
#[derive(Clone)]
pub struct Events {
    sender: UnboundedSender<Event>,
}

impl Events {
    pub fn send(&self, event: Event) {
        self.sender
            .send(event)
            .expect("frontend event channel stopped");
    }

    pub fn progress(&self, key: &DeviceKey, message: impl Into<String>) {
        self.send(Event::Progress {
            key: key.clone(),
            message: message.into(),
        });
    }
}
