mod backend;
mod cache;
mod known_apps;

use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use console::style;
use inquire::{Confirm, MultiSelect, Select};
use tokio::sync::{mpsc::UnboundedReceiver, RwLock};
use tracing::warn;

use backend::{
    AppleTv, Check, Command, DeviceSummary, Event, InstalledApp, PairingKind,
    Transport, WirelessStatus,
};
use cache::Cache;

// =============================================================================
// Shared state
// =============================================================================

#[derive(Default)]
struct AppState {
    devices: Vec<DeviceSummary>,
    apple_tvs: Vec<AppleTv>,
    last_apps: HashMap<String, Vec<InstalledApp>>,
    last_pairing: HashMap<String, PairingInfo>,
    device_info: HashMap<String, DeviceMeta>,
}

#[derive(Clone, Default)]
struct DeviceMeta {
    model: String,
    ios_version: String,
}

#[derive(Clone)]
struct PairingInfo {
    file_name: String,
    bytes: Vec<u8>,
}

// =============================================================================
// Entry point
// =============================================================================

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    print_banner();

    let (backend, events) = backend::spawn();
    let state = Arc::new(RwLock::new(AppState::default()));

    let event_task = tokio::spawn(event_loop(events, state.clone()));

    tokio::time::sleep(Duration::from_millis(800)).await;

    let result = main_menu(backend.clone(), state.clone()).await;

    event_task.abort();

    result
}

fn print_banner() {
    println!();
    println!(
        "  {} {}",
        style("idevice pair").bold().cyan(),
        style(format!("v{}", env!("CARGO_PKG_VERSION"))).dim()
    );
    println!(
        "  {}",
        style("Interactive CLI for iOS pairing file generation").dim()
    );
    println!();
}

// =============================================================================
// Event loop
// =============================================================================

async fn event_loop(
    mut events: UnboundedReceiver<Event>,
    state: Arc<RwLock<AppState>>,
) {
    while let Some(event) = events.recv().await {
        match event {
            Event::Devices(list) => {
                let mut s = state.write().await;
                s.devices = list;
            }

            Event::AppleTvs(list) => {
                let mut s = state.write().await;
                s.apple_tvs = list;
            }

            Event::Info { key, result } => match result {
                Ok(info) => {
                    eprintln!(
                        "{} {} - {} - iOS {}",
                        style("[info]").dim(),
                        style(&key).cyan(),
                        info.model,
                        info.version
                    );

                    let mut s = state.write().await;
                    s.device_info.insert(
                        key,
                        DeviceMeta {
                            model: info.model,
                            ios_version: info.version,
                        },
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} - {}",
                        style("[info]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Check {
                key,
                check,
                result,
            } => {
                let name = match check {
                    Check::WirelessDebugging => "Wireless debugging",
                    Check::DeveloperMode => "Developer mode",
                };
                let status = match result {
                    Ok(true) => style("enabled").green().to_string(),
                    Ok(false) => style("disabled").yellow().to_string(),
                    Err(e) => style(e).red().to_string(),
                };
                eprintln!(
                    "{} {} - {}: {}",
                    style("[check]").dim(),
                    style(&key).cyan(),
                    name,
                    status
                );
            }

            Event::PairRecord { key, stored } => {
                let status = if stored {
                    style("available").green()
                } else {
                    style("missing").yellow()
                };
                eprintln!(
                    "{} {} - pairing record: {}",
                    style("[record]").dim(),
                    style(&key).cyan(),
                    status
                );
            }

            Event::Progress { key, message } => {
                eprintln!(
                    "{} {} - {}",
                    style("[progress]").dim(),
                    style(&key).cyan(),
                    style(message).italic()
                );
            }

            Event::Pairing { key, result } => match result {
                Ok(pairing) => {
                    eprintln!(
                        "{} {} - pairing created: {}",
                        style("[pairing]").dim(),
                        style(&key).cyan(),
                        style(&pairing.file_name).green().bold()
                    );

                    let mut s = state.write().await;
                    s.last_pairing.insert(
                        key.clone(),
                        PairingInfo {
                            file_name: pairing.file_name.clone(),
                            bytes: pairing.bytes.clone(),
                        },
                    );

                    let device_name = s
                        .devices
                        .iter()
                        .find(|d| d.key == key)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| "unknown".into());

                    let ios_version = s
                        .device_info
                        .get(&key)
                        .map(|m| m.ios_version.clone())
                        .unwrap_or_else(|| "unknown".into());

                    let udid = key
                        .split_once(':')
                        .map(|(_, u)| u)
                        .unwrap_or(&key)
                        .to_string();

                    let kind = if pairing.file_name == "pairingFile.plist" {
                        PairingKind::Remote
                    } else {
                        PairingKind::Lockdown
                    };

                    drop(s);

                    if let Ok(cache) = Cache::open() {
                        if let Err(error) = cache.save(
                            &key,
                            kind,
                            &pairing.bytes,
                            &udid,
                            &pairing.file_name,
                            &device_name,
                            &ios_version,
                        ) {
                            warn!("failed to cache pairing: {error}");
                        } else {
                            eprintln!(
                                "{} cached pairing for {}",
                                style("[cache]").dim().green(),
                                style(&device_name).cyan()
                            );
                        }
                    }
                }
                Err(error) => {
                    eprintln!(
                        "{} {} - {}",
                        style("[pairing]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Validation { key, result } => match result {
                Ok(()) => {
                    eprintln!(
                        "{} {} - validation {}",
                        style("[validate]").dim(),
                        style(&key).cyan(),
                        style("succeeded").green().bold()
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} - validation failed: {}",
                        style("[validate]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Apps { key, result } => match result {
                Ok(apps) => {
                    eprintln!(
                        "{} {} - {} supported apps",
                        style("[apps]").dim(),
                        style(&key).cyan(),
                        style(apps.len()).green().bold()
                    );
                    for app in &apps {
                        eprintln!(
                            "           - {} ({})",
                            app.name, app.bundle_id
                        );
                    }

                    let mut s = state.write().await;
                    s.last_apps.insert(key, apps);
                }
                Err(error) => {
                    eprintln!(
                        "{} {} - {}",
                        style("[apps]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Install { key, app, result } => match result {
                Ok(()) => {
                    eprintln!(
                        "{} {} - installed into {}",
                        style("[install]").dim(),
                        style(&key).cyan(),
                        style(&app).green().bold()
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} - install into {} failed: {}",
                        style("[install]").dim(),
                        style(&key).cyan(),
                        &app,
                        style(error).red()
                    );
                }
            },

            Event::Wireless(status) => match status {
                WirelessStatus::Advertising(name) => {
                    eprintln!(
                        "{} advertising as {}",
                        style("[wireless]").magenta().bold(),
                        style(name).cyan()
                    );
                }
                WirelessStatus::Connected => {
                    eprintln!(
                        "{} device connected",
                        style("[wireless]").magenta().bold()
                    );
                }
                WirelessStatus::EnterPin(host) => {
                    eprintln!(
                        "{} enter the PIN shown by {}",
                        style("[wireless]").magenta().bold(),
                        style(host).cyan()
                    );
                }
                WirelessStatus::Pin(pin) => {
                    eprintln!(
                        "{} PIN: {}",
                        style("[wireless]").magenta().bold(),
                        style(pin).yellow().bold()
                    );
                }
                WirelessStatus::Paired(key) => {
                    eprintln!(
                        "{} paired: {}",
                        style("[wireless]").magenta().bold(),
                        style(key).green().bold()
                    );
                }
                WirelessStatus::Failed(error) => {
                    eprintln!(
                        "{} {}",
                        style("[wireless]").magenta().bold(),
                        style(error).red()
                    );
                }
            },

            Event::UsbmuxdFailure(error) => {
                eprintln!(
                    "{} {}",
                    style("[usbmuxd]").red().bold(),
                    error
                );
            }
        }
    }
}

// =============================================================================
// Menu
// =============================================================================

#[derive(Clone, Copy, PartialEq)]
enum Action {
    PairLockdown,
    PairRemote,
    Validate,
    ListApps,
    InstallApps,
    WirelessAccept,
    WirelessAppleTv,
    ShowDevices,
    SavePairingFile,
    LoadFromCache,
    ClearCache,
    Exit,
}

async fn main_menu(
    backend: backend::Backend,
    state: Arc<RwLock<AppState>>,
) -> Result<()> {
    loop {
        println!();

        let choice = match Select::new(
            "Select action:",
            vec![
                "Pair device (Lockdown - iOS 16)",
                "Pair device (Remote - iOS 17.4+)",
                "Validate pairing",
                "List supported apps",
                "Install pairing file into app",
                "Wireless pairing (accept)",
                "Pair Apple TV",
                "Show connected devices",
                "Save last pairing file to disk",
                "Load pairing from cache",
                "Clear pairing cache",
                "Exit",
            ],
        )
        .with_page_size(14)
        .prompt()
        {
            Ok(c) => c,
            Err(inquire::InquireError::OperationInterrupted) => break,
            Err(inquire::InquireError::OperationCanceled) => break,
            Err(e) => return Err(e.into()),
        };

        let action = match choice {
            s if s.starts_with("Pair device (Lockdown") => Action::PairLockdown,
            s if s.starts_with("Pair device (Remote") => Action::PairRemote,
            s if s.starts_with("Validate") => Action::Validate,
            s if s.starts_with("List supported") => Action::ListApps,
            s if s.starts_with("Install") => Action::InstallApps,
            s if s.starts_with("Wireless pairing") => Action::WirelessAccept,
            s if s.starts_with("Pair Apple TV") => Action::WirelessAppleTv,
            s if s.starts_with("Show connected") => Action::ShowDevices,
            s if s.starts_with("Save last") => Action::SavePairingFile,
            s if s.starts_with("Load pairing") => Action::LoadFromCache,
            s if s.starts_with("Clear pairing") => Action::ClearCache,
            _ => Action::Exit,
        };

        let result = match action {
            Action::Exit => break,
            Action::ShowDevices => {
                show_devices(&state).await;
                Ok(())
            }
            Action::PairLockdown => {
                do_pair(&backend, &state, PairingKind::Lockdown).await
            }
            Action::PairRemote => {
                do_pair(&backend, &state, PairingKind::Remote).await
            }
            Action::Validate => do_validate(&backend, &state).await,
            Action::ListApps => do_list_apps(&backend, &state).await,
            Action::InstallApps => do_install_apps(&backend, &state).await,
            Action::WirelessAccept => do_wireless_accept(&backend).await,
            Action::WirelessAppleTv => do_wireless_apple_tv(&backend, &state).await,
            Action::SavePairingFile => do_save_pairing(&state).await,
            Action::LoadFromCache => do_load_from_cache(&state).await,
            Action::ClearCache => do_clear_cache().await,
        };

        if let Err(error) = result {
            eprintln!("\n{} {}", style("ERROR:").red().bold(), error);
        }
    }

    println!("\n{}", style("Goodbye").dim());
    Ok(())
}

// =============================================================================
// Helpers
// =============================================================================

async fn pick_device(state: &Arc<RwLock<AppState>>) -> Result<DeviceSummary> {
    let devices = state.read().await.devices.clone();

    if devices.is_empty() {
        anyhow::bail!(
            "No device detected. Connect an iPhone via USB and wait a moment."
        );
    }

    let labels: Vec<String> = devices
        .iter()
        .map(|d| {
            format!(
                "{} [{}] {}",
                d.name,
                transport_label(d.transport),
                d.key
            )
        })
        .collect();

    let choice = Select::new("Select device:", labels)
        .with_page_size(10)
        .prompt()?;

    devices
        .into_iter()
        .find(|d| {
            format!(
                "{} [{}] {}",
                d.name,
                transport_label(d.transport),
                d.key
            ) == choice
        })
        .context("device no longer exists")
}

fn transport_label(t: Transport) -> &'static str {
    match t {
        Transport::Usb => "USB",
        Transport::Network => "Network",
        Transport::Remote => "Remote",
    }
}

// =============================================================================
// Actions
// =============================================================================

async fn show_devices(state: &Arc<RwLock<AppState>>) {
    let s = state.read().await;

    println!();
    if s.devices.is_empty() {
        println!("{}", style("No devices.").yellow());
    } else {
        println!("{}", style("Devices:").bold());
        for d in &s.devices {
            println!(
                "  - {} [{}] {}",
                style(&d.name).cyan(),
                transport_label(d.transport),
                style(&d.key).dim()
            );
        }
    }

    if !s.apple_tvs.is_empty() {
        println!("\n{}", style("Apple TVs (Remote Pairing):").bold());
        for tv in &s.apple_tvs {
            println!("  - {}", style(&tv.name).cyan());
        }
    }
}

async fn do_pair(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
    kind: PairingKind,
) -> Result<()> {
    let device = pick_device(state).await?;

    let kind_label = match kind {
        PairingKind::Lockdown => "Lockdown",
        PairingKind::Remote => "Remote",
    };

    let confirm = Confirm::new(&format!(
        "Start pairing {} with {} in {} mode?",
        device.name, device.key, kind_label
    ))
    .with_default(true)
    .prompt()?;

    if !confirm {
        return Ok(());
    }

    println!(
        "\n{} Pairing started... watch for Trust prompt on iPhone\n",
        style(">>").cyan().bold()
    );

    backend.send(Command::CreatePairing {
        key: device.key,
        kind,
    });

    tokio::time::sleep(Duration::from_secs(30)).await;

    Ok(())
}

async fn do_validate(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
) -> Result<()> {
    let device = pick_device(state).await?;

    println!(
        "\n{} Validating...\n",
        style(">>").cyan().bold()
    );

    backend.send(Command::Validate {
        key: device.key,
        ip: None,
    });

    tokio::time::sleep(Duration::from_secs(10)).await;
    Ok(())
}

async fn do_list_apps(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
) -> Result<()> {
    let device = pick_device(state).await?;

    println!(
        "\n{} Listing apps...\n",
        style(">>").cyan().bold()
    );

    backend.send(Command::ListApps {
        key: device.key,
        kind: PairingKind::Lockdown,
    });

    tokio::time::sleep(Duration::from_secs(5)).await;
    Ok(())
}

async fn do_install_apps(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
) -> Result<()> {
    let device = pick_device(state).await?;
    let key = device.key.clone();

    let apps = {
        let s = state.read().await;
        s.last_apps.get(&key).cloned()
    };

    let apps = match apps {
        Some(apps) if !apps.is_empty() => apps,
        _ => {
            println!(
                "\n{} No cached apps, listing now...\n",
                style(">>").cyan().bold()
            );
            backend.send(Command::ListApps {
                key: key.clone(),
                kind: PairingKind::Lockdown,
            });
            tokio::time::sleep(Duration::from_secs(5)).await;

            let s = state.read().await;
            s.last_apps.get(&key).cloned().unwrap_or_default()
        }
    };

    if apps.is_empty() {
        anyhow::bail!("No supported app found for pairing file install.");
    }

    let choices: Vec<String> = apps
        .iter()
        .map(|a| format!("{} ({})", a.name, a.bundle_id))
        .collect();

    let selected = MultiSelect::new(
        "Select apps to install pairing file:",
        choices,
    )
    .with_page_size(10)
    .prompt()?;

    for label in selected {
        let app = apps
            .iter()
            .find(|a| format!("{} ({})", a.name, a.bundle_id) == label)
            .cloned();

        if let Some(app) = app {
            println!(
                "\n{} Installing into {}...\n",
                style(">>").cyan().bold(),
                style(&app.name).cyan()
            );

            backend.send(Command::Install {
                key: key.clone(),
                app,
            });

            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    Ok(())
}

async fn do_wireless_accept(backend: &backend::Backend) -> Result<()> {
    println!(
        "\n{} Starting wireless pairing... (press Enter to stop)\n",
        style(">>").cyan().bold()
    );

    backend.send(Command::StartWirelessPairing);

    let _ = tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
    })
    .await;

    backend.send(Command::StopWirelessPairing);
    Ok(())
}

async fn do_wireless_apple_tv(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
) -> Result<()> {
    let tvs = state.read().await.apple_tvs.clone();

    if tvs.is_empty() {
        anyhow::bail!("No Apple TV found via mDNS.");
    }

    let labels: Vec<String> = tvs.iter().map(|tv| tv.name.clone()).collect();
    let choice = Select::new("Select Apple TV:", labels).prompt()?;

    let tv = tvs
        .into_iter()
        .find(|t| t.name == choice)
        .context("Apple TV no longer exists")?;

    println!(
        "\n{} Pairing Apple TV {}...\n",
        style(">>").cyan().bold(),
        style(&tv.name).cyan()
    );

    backend.send(Command::PairAppleTv(tv));

    tokio::time::sleep(Duration::from_secs(30)).await;
    Ok(())
}

async fn do_save_pairing(state: &Arc<RwLock<AppState>>) -> Result<()> {
    let s = state.read().await;

    if s.last_pairing.is_empty() {
        anyhow::bail!("No pairing file created in this session.");
    }

    let keys: Vec<String> = s.last_pairing.keys().cloned().collect();
    let choice = Select::new("Select pairing file to save:", keys).prompt()?;

    let info = s
        .last_pairing
        .get(&choice)
        .context("pairing info not found")?;

    let filename = format!(
        "{}_{}",
        choice.replace(':', "_"),
        info.file_name
    );

    std::fs::write(&filename, &info.bytes)
        .with_context(|| format!("failed to write {}", filename))?;

    println!(
        "\n{} Saved {} ({} bytes)\n",
        style("OK").green().bold(),
        style(&filename).cyan(),
        info.bytes.len()
    );

    Ok(())
}

async fn do_load_from_cache(state: &Arc<RwLock<AppState>>) -> Result<()> {
    let cache = Cache::open()?;
    let entries = cache.list()?;

    if entries.is_empty() {
        anyhow::bail!("Cache is empty. No paired device yet.");
    }

    let labels: Vec<String> = entries
        .iter()
        .map(|m| {
            format!(
                "{} [{}] {} (iOS {})",
                m.device_name,
                match m.kind {
                    cache::PairingKindMeta::Lockdown => "Lockdown",
                    cache::PairingKindMeta::Remote => "Remote",
                },
                m.udid,
                m.ios_version
            )
        })
        .collect();

    let choice = Select::new("Select pairing file from cache:", labels)
        .with_page_size(10)
        .prompt()?;

    let idx = labels
        .iter()
        .position(|l| l == &choice)
        .context("cache entry not found")?;

    let meta = &entries[idx];
    let kind = match meta.kind {
        cache::PairingKindMeta::Lockdown => PairingKind::Lockdown,
        cache::PairingKindMeta::Remote => PairingKind::Remote,
    };

    let bytes = cache
        .load(&meta.key, kind)?
        .context("cache file missing")?;

    let mut s = state.write().await;
    s.last_pairing.insert(
        meta.key.clone(),
        PairingInfo {
            file_name: meta.file_name.clone(),
            bytes: bytes.clone(),
        },
    );

    println!(
        "\n{} Loaded pairing from cache for {} ({} bytes)\n",
        style("OK").green().bold(),
        style(&meta.device_name).cyan(),
        bytes.len()
    );

    Ok(())
}

async fn do_clear_cache() -> Result<()> {
    let confirm = Confirm::new("Clear all pairing cache?")
        .with_default(false)
        .prompt()?;

    if !confirm {
        return Ok(());
    }

    let cache = Cache::open()?;
    cache.clear()?;

    println!(
        "\n{} Cache cleared\n",
        style("OK").green().bold()
    );

    Ok(())
}
