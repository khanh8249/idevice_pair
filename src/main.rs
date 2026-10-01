mod backend;
mod known_apps;

use std::{
    collections::HashSet,
    io::{self, Write},
};

use backend::{
    Command, Event, PairingKind, Transport,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!(
        "idevice pair v{}",
        env!("CARGO_PKG_VERSION")
    );

    let (backend, mut events) = backend::spawn();

    println!("Waiting for iOS devices...");
    println!("USB devices are detected automatically.");

    let mut pairing_started = HashSet::<String>::new();
    let mut validation_started = HashSet::<String>::new();
    let mut apps_requested = HashSet::<String>::new();
    let mut installs_started = HashSet::<String>::new();

    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                handle_event(
                    &backend,
                    event,
                    &mut pairing_started,
                    &mut validation_started,
                    &mut apps_requested,
                    &mut installs_started,
                );
            }

            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("Failed to listen for Ctrl+C: {error}");
                }

                println!("\nExiting...");
                break;
            }
        }
    }

    drop(backend);
}

fn handle_event(
    backend: &backend::Backend,
    event: Event,
    pairing_started: &mut HashSet<String>,
    validation_started: &mut HashSet<String>,
    apps_requested: &mut HashSet<String>,
    installs_started: &mut HashSet<String>,
) {
    match event {
        Event::Devices(devices) => {
            if devices.is_empty() {
                println!("\nNo iOS devices found.");
                return;
            }

            println!("\nDevices:");

            for device in devices {
                let transport = match device.transport {
                    Transport::Usb => "USB",
                    Transport::Network => "Network",
                    Transport::Remote => "Remote",
                };

                println!(
                    "  {} [{}] {}",
                    device.name,
                    transport,
                    device.key
                );

                if device.transport == Transport::Usb
                    && pairing_started.insert(format!("{}:inspect", device.key))
                {
                    println!(
                        "{}: device detected, inspecting...",
                        device.key
                    );

                    backend.send(Command::Inspect(device.key));
                }
            }

            let _ = io::stdout().flush();
        }

        Event::AppleTvs(devices) => {
            if devices.is_empty() {
                return;
            }

            println!("\nRemote Pairing hosts:");

            for device in devices {
                println!("  {}", device.name);
            }
        }

        Event::UsbmuxdFailure(error) => {
            eprintln!("usbmuxd error: {error}");
        }

        Event::Info { key, result } => {
            match result {
                Ok(info) => {
                    println!(
                        "\nDevice: {}\n  Model: {}\n  iOS: {}\n  UDID: {}",
                        key,
                        info.model,
                        info.version,
                        info.udid
                    );
                }

                Err(error) => {
                    eprintln!(
                        "Failed to inspect {key}: {error}"
                    );
                }
            }
        }

        Event::Check {
            key,
            check,
            result,
        } => {
            let name = match check {
                backend::Check::WirelessDebugging => {
                    "Wireless debugging"
                }

                backend::Check::DeveloperMode => {
                    "Developer mode"
                }
            };

            match result {
                Ok(true) => {
                    println!("{key}: {name}: enabled");
                }

                Ok(false) => {
                    println!("{key}: {name}: disabled");
                }

                Err(error) => {
                    eprintln!("{key}: {name}: {error}");
                }
            }
        }

        Event::PairRecord { key, stored } => {
            println!(
                "{key}: pairing record: {}",
                if stored {
                    "available"
                } else {
                    "missing"
                }
            );

            if stored {
                // Đã có pairing record → validate + list apps
                if validation_started.insert(key.clone()) {
                    println!("{key}: validating stored pairing...");

                    backend.send(Command::Validate {
                        key,
                        ip: None,
                    });
                }
            } else {
                // Chưa có → tạo pairing mới (Lockdown cho iOS 16)
                if pairing_started.insert(format!("{key}:pair")) {
                    println!(
                        "{key}: no pairing record, starting Lockdown pairing..."
                    );
                    println!(
                        "{key}: tap \"Trust\" on your iPhone when prompted."
                    );

                    backend.send(Command::CreatePairing {
                        key,
                        kind: PairingKind::Lockdown,
                    });
                }
            }
        }

        Event::Apps { key, result } => {
            match result {
                Ok(apps) => {
                    println!(
                        "\n{key}: {} supported apps",
                        apps.len()
                    );

                    if apps.is_empty() {
                        println!(
                            "{key}: no supported pairing targets found."
                        );
                        return;
                    }

                    for app in apps {
                        println!(
                            "  {} ({})",
                            app.name,
                            app.bundle_id
                        );

                        let install_key =
                            format!("{}:{}", key, app.name);

                        if installs_started.insert(install_key) {
                            println!(
                                "{key}: installing pairing file into {}...",
                                app.name
                            );

                            backend.send(Command::Install {
                                key: key.clone(),
                                app,
                            });
                        }
                    }
                }

                Err(error) => {
                    eprintln!(
                        "{key}: failed to list apps: {error}"
                    );
                }
            }
        }

        Event::Progress { key, message } => {
            println!("{key}: {message}");
        }

        Event::Pairing { key, result } => {
            match result {
                Ok(pairing) => {
                    println!(
                        "\n{key}: pairing file created: {}",
                        pairing.file_name
                    );

                    // Sau khi pair thành công, lưu file ra đĩa
                    let filename = format!("{}.mobiledevicepairing", key.replace(':', "_"));
                    if let Err(error) = std::fs::write(&filename, &pairing.bytes) {
                        eprintln!("{key}: failed to save {filename}: {error}");
                    } else {
                        println!("{key}: pairing file saved to {filename}");
                    }

                    // Validate lại để chắc chắn record hoạt động
                    println!("{key}: validating pairing...");

                    if validation_started.insert(key.clone()) {
                        backend.send(Command::Validate {
                            key,
                            ip: None,
                        });
                    }
                }

                Err(error) => {
                    eprintln!(
                        "{key}: pairing failed: {error}"
                    );
                }
            }
        }

        Event::Validation { key, result } => {
            match result {
                Ok(()) => {
                    println!(
                        "{key}: pairing validation succeeded."
                    );

                    println!(
                        "{key}: searching for supported apps..."
                    );

                    if apps_requested.insert(key.clone()) {
                        backend.send(Command::ListApps {
                            key,
                            kind: PairingKind::Lockdown,
                        });
                    }
                }

                Err(error) => {
                    eprintln!(
                        "{key}: pairing validation failed: {error}"
                    );

                    eprintln!(
                        "{key}: pairing file will not be installed."
                    );
                }
            }
        }

        Event::Install {
            key,
            app,
            result,
        } => {
            match result {
                Ok(()) => {
                    println!(
                        "{key}: pairing file installed into {app}."
                    );
                }

                Err(error) => {
                    eprintln!(
                        "{key}: failed to install into {app}: {error}"
                    );
                }
            }
        }

        Event::Wireless(status) => {
            match status {
                backend::WirelessStatus::Advertising(name) => {
                    println!(
                        "Wireless pairing: advertising as {name}"
                    );
                }

                backend::WirelessStatus::Connected => {
                    println!(
                        "Wireless pairing: device connected"
                    );
                }

                backend::WirelessStatus::EnterPin(host) => {
                    println!(
                        "Wireless pairing: enter the PIN shown by {host}"
                    );
                }

                backend::WirelessStatus::Pin(pin) => {
                    println!(
                        "Wireless pairing PIN: {pin}"
                    );
                }

                backend::WirelessStatus::Paired(key) => {
                    println!(
                        "Wireless pairing completed: {key}"
                    );
                }

                backend::WirelessStatus::Failed(error) => {
                    eprintln!(
                        "Wireless pairing failed: {error}"
                    );
                }
            }
        }
    }
}
