mod backend;
mod known_apps;

use std::io::{self, Write};

use backend::{
    Backend, Command, Event, PairingKind, Transport,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!(
        "idevice pair v{}",
        env!("CARGO_PKG_VERSION")
    );

    let (backend, mut events) = backend::spawn();

    println!("Waiting for iOS devices...");

    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                handle_event(event);
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

fn handle_event(event: Event) {
    match event {
        Event::Devices(devices) => {
            println!("\nDevices:");

            if devices.is_empty() {
                println!("  No devices found.");
                return;
            }

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
            }

            print!("> ");
            let _ = io::stdout().flush();
        }

        Event::AppleTvs(devices) => {
            if !devices.is_empty() {
                println!("\nRemote Pairing hosts:");

                for device in devices {
                    println!("  {}", device.name);
                }
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
                    eprintln!("Failed to inspect {key}: {error}");
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
                if stored { "available" } else { "missing" }
            );
        }

        Event::Apps { key, result } => {
            match result {
                Ok(apps) => {
                    println!("{key}: {} supported apps", apps.len());

                    for app in apps {
                        println!(
                            "  {} ({})",
                            app.name,
                            app.bundle_id
                        );
                    }
                }

                Err(error) => {
                    eprintln!("{key}: failed to list apps: {error}");
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
                        "{key}: pairing file created ({:?})",
                        pairing
                    );
                }

                Err(error) => {
                    eprintln!("{key}: pairing failed: {error}");
                }
            }
        }

        Event::Validation { key, result } => {
            match result {
                Ok(()) => {
                    println!("{key}: pairing validation succeeded");
                }

                Err(error) => {
                    eprintln!("{key}: pairing validation failed: {error}");
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
                    println!("{key}: installed pairing file into {app}");
                }

                Err(error) => {
                    eprintln!("{key}: failed to install into {app}: {error}");
                }
            }
        }

        Event::Wireless(status) => {
            match status {
                backend::WirelessStatus::Advertising(name) => {
                    println!("Wireless pairing: advertising as {name}");
                }

                backend::WirelessStatus::Connected => {
                    println!("Wireless pairing: device connected");
                }

                backend::WirelessStatus::EnterPin(host) => {
                    println!(
                        "Wireless pairing: enter the PIN shown by {host}"
                    );
                }

                backend::WirelessStatus::Pin(pin) => {
                    println!("Wireless pairing PIN: {pin}");
                }

                backend::WirelessStatus::Paired(key) => {
                    println!("Wireless pairing completed: {key}");
                }

                backend::WirelessStatus::Failed(error) => {
                    eprintln!("Wireless pairing failed: {error}");
                }
            }
        }
    }
}
