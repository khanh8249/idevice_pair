use futures_util::StreamExt;

use idevice::{
    lockdown::LockdownClient,
    pairing_file::PairingFile,
    provider::IdeviceProvider,
    usbmuxd::{
        Connection,
        UsbmuxdAddr,
        UsbmuxdConnection,
        UsbmuxdDevice,
        UsbmuxdListenEvent,
    },
    IdeviceError,
};

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use super::{
    host_label,
    link::{device_info, value},
    DeviceInfo,
};

const DESCRIBE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(4);

/// A usbmuxd device together with the basic information obtained
/// from Lockdown.
pub struct UsbDevice {
    pub device: UsbmuxdDevice,
    pub name: String,
    pub info: DeviceInfo,
}

/// Enumerate all devices currently visible through usbmuxd.
///
/// Devices that cannot be described through Lockdown are skipped.
/// This is intentional: a device can appear in usbmuxd before its
/// Lockdown service is ready.
pub async fn list() -> Result<Vec<UsbDevice>, IdeviceError> {
    let connection =
        UsbmuxdConnection::default().await?;

    let devices =
        connection.get_devices().await?;

    let described =
        futures_util::future::join_all(
            devices.iter().map(describe),
        )
        .await;

    Ok(devices
        .into_iter()
        .zip(described)
        .filter_map(|(device, result)| {
            result.map(|(name, info)| UsbDevice {
                device,
                name,
                info,
            })
        })
        .collect())
}

/// Return the usbmuxd host BUID.
pub async fn buid() -> Result<String, IdeviceError> {
    UsbmuxdConnection::default()
        .await?
        .get_buid()
        .await
}

/// Read the stored Lockdown pairing record for a device.
pub async fn pair_record(
    udid: &str,
) -> Result<PairingFile, IdeviceError> {
    UsbmuxdConnection::default()
        .await?
        .get_pair_record(udid)
        .await
}

/// Watch usbmuxd for device connect/disconnect events.
///
/// The watcher deliberately uses a separate task/runtime from the
/// main backend so a broken usbmuxd connection does not terminate
/// the worker.
pub async fn watch(
    changes: UnboundedSender<()>,
) {
    if let Err(error) = listen(&changes).await {
        warn!(
            "usbmuxd listen stopped: {}",
            super::message(&error)
        );
    }
}

async fn listen(
    changes: &UnboundedSender<()>,
) -> Result<(), IdeviceError> {
    let mut connection =
        UsbmuxdConnection::default().await?;

    let mut stream =
        connection.listen().await?;

    while let Some(event) = stream.next().await {
        match event? {
            UsbmuxdListenEvent::Connected(device)
            | UsbmuxdListenEvent::Disconnected(device) => {
                debug!(
                    "usbmuxd device event: {} ({:?})",
                    device.udid,
                    device.connection_type
                );

                // The receiver only needs to know that something
                // changed; the worker performs a fresh enumeration.
                if changes.send(()).is_err() {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

/// Obtain Lockdown information for one device.
///
/// A short timeout is used because a device may be visible through
/// usbmuxd while Lockdown is still starting or waiting for trust.
async fn describe(
    device: &UsbmuxdDevice,
) -> Option<(String, DeviceInfo)> {
    match tokio::time::timeout(
        DESCRIBE_TIMEOUT,
        ask(device),
    )
    .await
    {
        Ok(Ok(result)) => Some(result),

        Ok(Err(error)) => {
            debug!(
                "could not describe {}: {}",
                device.udid,
                super::message(&error)
            );

            None
        }

        Err(_) => {
            debug!(
                "describing {} timed out",
                device.udid
            );

            None
        }
    }
}

/// Connect to Lockdown and retrieve the device name and
/// basic device information.
async fn ask(
    device: &UsbmuxdDevice,
) -> Result<(String, DeviceInfo), IdeviceError> {
    let provider = device.to_provider(
        UsbmuxdAddr::default(),
        host_label(),
    );

    let mut client =
        LockdownClient::connect(&provider).await?;

    // USB devices can expose Lockdown directly.
    //
    // Network usbmuxd devices require an authenticated Lockdown
    // session using the stored pairing record.
    if device.connection_type != Connection::Usb {
        let pairing_file =
            provider.get_pairing_file().await?;

        client
            .start_session(&pairing_file)
            .await?;
    }

    let name =
        value(&mut client, "DeviceName").await?;

    let info =
        device_info(&mut client).await?;

    Ok((name, info))
}
