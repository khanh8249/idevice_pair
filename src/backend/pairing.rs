use super::{host_label, link::Link, DeviceKey, Events};

use idevice::{
    pairing_file::PairingFile,
    remote_pairing::{RemotePairingClient, RpPairingFile},
    IdeviceError,
    ReadWrite,
    RemoteXpcClient,
};

const RP_FILE_NAME: &str = "pairingFile.plist";

const TUNNEL_SERVICE: &str =
    "com.apple.internal.dt.coredevice.untrusted.tunnelservice";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PairingKind {
    Lockdown,

    #[default]
    Remote,
}

impl PairingKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lockdown => "Lockdown",
            Self::Remote => "Remote pairing",
        }
    }
}

#[derive(Clone)]
pub enum Payload {
    Lockdown(Box<PairingFile>),
    Remote(Box<RpPairingFile>),
}

impl Payload {
    /// Serialize the pairing payload exactly as it should be written
    /// to disk or transferred to an application.
    pub fn bytes(&self) -> Result<Vec<u8>, IdeviceError> {
        match self {
            Self::Lockdown(file) => file.clone().serialize(),

            Self::Remote(file) => {
                Ok(file.to_bytes())
            }
        }
    }

    /// Convert the pairing payload into a result suitable for the
    /// frontend/CLI.
    pub fn result(
        &self,
        udid: &str,
    ) -> Result<PairingResult, IdeviceError> {
        let bytes = self.bytes()?;

        let text = std::str::from_utf8(&bytes)
            .map_err(|_| {
                IdeviceError::UnexpectedResponse(
                    "pairing file is not UTF-8".into(),
                )
            })?
            .trim_end()
            .to_owned();

        let file_name = match self {
            Self::Lockdown(_) => {
                format!("{udid}.plist")
            }

            Self::Remote(_) => {
                RP_FILE_NAME.to_string()
            }
        };

        Ok(PairingResult {
            bytes,
            text,
            file_name,
        })
    }
}

pub struct PairingResult {
    pub bytes: Vec<u8>,
    pub text: String,
    pub file_name: String,
}

/// Create a new Lockdown pairing file.
///
/// For usbmuxd devices we use the host BUID from usbmuxd.
/// For RSD devices a fresh UUID is used as the system BUID.
pub async fn lockdown_file(
    link: &mut Link,
    udid: &str,
    events: &Events,
    key: &DeviceKey,
) -> Result<PairingFile, IdeviceError> {
    let system_buid = match link {
        Link::Usbmuxd { .. } => {
            alter(super::usb::buid().await?)
        }

        Link::Rsd { .. } => {
            uuid()
        }
    };

    let mut client = link.lockdown().await?;

    events.progress(
        key,
        "Tap “Trust” on your device if it asks",
    );

    let mut file = client
        .pair(
            uuid(),
            system_buid,
            Some(host_label()),
        )
        .await?;

    // Explicitly associate the generated pairing record with
    // the actual device UDID.
    file.udid = Some(udid.to_owned());

    Ok(file)
}

/// Load an existing Lockdown pairing record from usbmuxd.
pub async fn stored_lockdown_file(
    udid: &str,
) -> Result<PairingFile, IdeviceError> {
    let mut file =
        super::usb::pair_record(udid).await?;

    file.udid = Some(udid.to_owned());

    Ok(file)
}

/// Create a Remote Pairing file through the CoreDevice
/// tunnel service.
pub async fn remote_file(
    link: &mut Link,
    events: &Events,
    key: &DeviceKey,
) -> Result<RpPairingFile, IdeviceError> {
    let mut file =
        RpPairingFile::generate(host_label());

    for message in [
        "Trust this computer on your device",
        "Saving the pairing on the device",
    ] {
        events.progress(key, message);

        tunnel_service_client(link)
            .await?
            .connect(
                &mut file,
                || async {
                    // The tunnel service expects a PIN provider.
                    // The pairing flow itself supplies the actual
                    // trust interaction on the device.
                    "000000".to_string()
                },
            )
            .await?;
    }

    Ok(file)
}

/// Verify an existing Remote Pairing file against the device.
pub async fn verify_remote(
    link: &mut Link,
    file: &mut RpPairingFile,
) -> Result<(), IdeviceError> {
    let mut client =
        tunnel_service_client(link).await?;

    client.attempt_pair_verify().await?;
    client.validate_pairing(file).await
}

/// Connect to Apple's CoreDevice tunnel service and establish
/// the Remote XPC handshake.
async fn tunnel_service_client(
    link: &mut Link,
) -> Result<
    RemotePairingClient<
        RemoteXpcClient<Box<dyn ReadWrite>>,
    >,
    IdeviceError,
> {
    let stream =
        link.connect_rsd_service(TUNNEL_SERVICE)
            .await?;

    let mut xpc =
        RemoteXpcClient::new(stream).await?;

    xpc.do_handshake().await?;

    // The root message is part of the Remote XPC handshake.
    // We don't currently need its contents.
    let _ = xpc.recv_root().await;

    Ok(RemotePairingClient::new(
        xpc,
        host_label(),
    ))
}

/// Generate an uppercase UUID used by Apple's pairing
/// protocol.
fn uuid() -> String {
    uuid::Uuid::new_v4()
        .to_string()
        .to_uppercase()
}

/// Transform the usbmuxd BUID into the alternate BUID expected
/// by the Lockdown pairing flow.
fn alter(mut buid: String) -> String {
    let first = if buid.starts_with('F') {
        'A'
    } else {
        'F'
    };

    buid.replace_range(..1, &first.to_string());

    buid
}
