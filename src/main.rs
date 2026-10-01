mod backend;
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

use backend::{
    AppleTv, Check, Command, DeviceSummary, Event, InstalledApp, PairingKind,
    Transport, WirelessStatus,
};

// =============================================================================
// Shared state — cập nhật realtime từ backend events
// =============================================================================

#[derive(Default)]
struct AppState {
    devices: Vec<DeviceSummary>,
    apple_tvs: Vec<AppleTv>,
    /// Cache kết quả ListApps gần nhất, key = device key
    last_apps: HashMap<String, Vec<InstalledApp>>,
    /// Cache pairing file vừa tạo, key = device key
    last_pairing: HashMap<String, PairingInfo>,
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
    // Khởi tạo tracing (chỉ log ra stderr để không phá TUI)
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

    // Spawn task xử lý events từ backend và cập nhật shared state
    let event_task = tokio::spawn(event_loop(events, state.clone()));

    // Đợi một chút để backend discover devices lần đầu
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Main loop: hiển thị menu
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
// Event loop — chạy song song với menu
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
                        "{} {} — {} — iOS {}",
                        style("[info]").dim(),
                        style(&key).cyan(),
                        info.model,
                        info.version
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} — {}",
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
                    "{} {} — {}: {}",
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
                    "{} {} — pairing record: {}",
                    style("[record]").dim(),
                    style(&key).cyan(),
                    status
                );
            }

            Event::Progress { key, message } => {
                eprintln!(
                    "{} {} — {}",
                    style("[progress]").dim(),
                    style(&key).cyan(),
                    style(message).italic()
                );
            }

            Event::Pairing { key, result } => match result {
                Ok(pairing) => {
                    eprintln!(
                        "{} {} — pairing created: {}",
                        style("[pairing]").dim(),
                        style(&key).cyan(),
                        style(&pairing.file_name).green().bold()
                    );

                    let mut s = state.write().await;
                    s.last_pairing.insert(
                        key,
                        PairingInfo {
                            file_name: pairing.file_name,
                            bytes: pairing.bytes,
                        },
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} — {}",
                        style("[pairing]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Validation { key, result } => match result {
                Ok(()) => {
                    eprintln!(
                        "{} {} — validation {}",
                        style("[validate]").dim(),
                        style(&key).cyan(),
                        style("succeeded").green().bold()
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} — validation failed: {}",
                        style("[validate]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Apps { key, result } => match result {
                Ok(apps) => {
                    eprintln!(
                        "{} {} — {} supported apps",
                        style("[apps]").dim(),
                        style(&key).cyan(),
                        style(apps.len()).green().bold()
                    );
                    for app in &apps {
                        eprintln!("           • {} ({})", app.name, app.bundle_id);
                    }

                    let mut s = state.write().await;
                    s.last_apps.insert(key, apps);
                }
                Err(error) => {
                    eprintln!(
                        "{} {} — {}",
                        style("[apps]").dim(),
                        style(&key).cyan(),
                        style(error).red()
                    );
                }
            },

            Event::Install { key, app, result } => match result {
                Ok(()) => {
                    eprintln!(
                        "{} {} — installed into {}",
                        style("[install]").dim(),
                        style(&key).cyan(),
                        style(&app).green().bold()
                    );
                }
                Err(error) => {
                    eprintln!(
                        "{} {} — install into {} failed: {}",
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
                    eprintln!("{} device connected", style("[wireless]").magenta().bold());
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
                eprintln!("{} {}", style("[usbmuxd]").red().bold(), error);
            }
        }
    }
}

// =============================================================================
// Menu chính
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
    Exit,
}

async fn main_menu(backend: backend::Backend, state: Arc<RwLock<AppState>>) -> Result<()> {
    loop {
        println!();
        let choice = Select::new(
            "Chọn hành động:",
            vec![
                "📱  Pair device (Lockdown — iOS 16)",
                "📱  Pair device (Remote — iOS 17.4+)",
                "✓   Validate pairing",
                "📋  List supported apps",
                "📥  Install pairing file into app",
                "📡  Wireless pairing (accept)",
                "📺  Pair Apple TV",
                "🔍  Show connected devices",
                "💾  Save last pairing file to disk",
                "🚪  Exit",
            ],
        )
        .with_page_size(12)
        .prompt()
        .context("menu cancelled")?;

        let action = match choice {
            s if s.starts_with("📱  Pair device (Lockdown") => Action::PairLockdown,
            s if s.starts_with("📱  Pair device (Remote") => Action::PairRemote,
            s if s.starts_with("✓") => Action::Validate,
            s if s.starts_with("📋") => Action::ListApps,
            s if s.starts_with("📥") => Action::InstallApps,
            s if s.starts_with("📡") => Action::WirelessAccept,
            s if s.starts_with("📺") => Action::WirelessAppleTv,
            s if s.starts_with("🔍") => Action::ShowDevices,
            s if s.starts_with("💾") => Action::SavePairingFile,
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
            Action::PairRemote => do_pair(&backend, &state, PairingKind::Remote).await,
            Action::Validate => do_validate(&backend, &state).await,
            Action::ListApps => do_list_apps(&backend, &state).await,
            Action::InstallApps => do_install_apps(&backend, &state).await,
            Action::WirelessAccept => do_wireless_accept(&backend).await,
            Action::WirelessAppleTv => do_wireless_apple_tv(&backend, &state).await,
            Action::SavePairingFile => do_save_pairing(&state).await,
        };

        if let Err(error) = result {
            eprintln!("\n{} {}", style("✗").red().bold(), error);
        }
    }

    println!("\n{}", style("Goodbye 👋").dim());
    Ok(())
}

// =============================================================================
// Helper chọn device
// =============================================================================

async fn pick_device(state: &Arc<RwLock<AppState>>) -> Result<DeviceSummary> {
    let devices = state.read().await.devices.clone();

    if devices.is_empty() {
        anyhow::bail!(
            "Chưa có device nào được phát hiện. Cắm iPhone qua USB và đợi vài giây."
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

    let choice = Select::new("Chọn device:", labels)
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
        .context("device không còn tồn tại")
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
        println!("{}", style("Không có device nào.").yellow());
    } else {
        println!("{}", style("Devices:").bold());
        for d in &s.devices {
            println!(
                "  • {} [{}] {}",
                style(&d.name).cyan(),
                transport_label(d.transport),
                style(&d.key).dim()
            );
        }
    }

    if !s.apple_tvs.is_empty() {
        println!("\n{}", style("Apple TVs (Remote Pairing):").bold());
        for tv in &s.apple_tvs {
            println!("  • {}", style(&tv.name).cyan());
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
        "Bắt đầu pair {} với {} ở mode {}?",
        device.name, device.key, kind_label
    ))
    .with_default(true)
    .prompt()?;

    if !confirm {
        return Ok(());
    }

    println!(
        "\n{} Đang pair... (theo dõi prompt \"Trust\" trên iPhone)\n",
        style("→").cyan().bold()
    );

    backend.send(Command::CreatePairing {
        key: device.key,
        kind,
    });

    // Đợi 1 chút để event loop in progress
    tokio::time::sleep(Duration::from_secs(30)).await;

    Ok(())
}

async fn do_validate(
    backend: &backend::Backend,
    state: &Arc<RwLock<AppState>>,
) -> Result<()> {
    let device = pick_device(state).await?;

    println!("\n{} Đang validate...\n", style("→").cyan().bold());

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

    println!("\n{} Đang list apps...\n", style("→").cyan().bold());

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

    // Nếu chưa có cache apps → list trước
    let apps = {
        let s = state.read().await;
        s.last_apps.get(&key).cloned()
    };

    let apps = match apps {
        Some(apps) if !apps.is_empty() => apps,
        _ => {
            println!(
                "\n{} Chưa có danh sách apps, đang list...\n",
                style("→").cyan().bold()
 state            );
            backend.send(Command::ListApps {
                key: key.clone(),
                kind: PairingKind::Lockdown,
            });
           : tokio::time::sleep(Duration::from_secs(5)).await;

            let s = state.read().await;
            s.last &_apps.get(&key).cloned().unwrap_or_default()
        }
    };

    if apps.is_empty() {
        anyhow::bailArc!("Không tìm thấy app nào hỗ trợ pairing file.");
    }

    let choices: Vec<String> = apps
       <R .iter()
        .map(|a| format!("{} ({})", a.name, a.bundle_id))
        .collect();

    let selected = MultiSelect::new("Chọn app để install pairing file:", choices)
        .with_page_size(10)
        .prompt()?;

    for label in selected {
        let app = apps
            .iter()
            .find(|a| format!("{} ({})", a.name, a.bundle_id) == label)
            .cloned();

        if let Some(app) = app {
            println!(
                "\n{} Đang install vào {}...\n",
                style("→").cyan().bold(),
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
        "\n{} Bắt đầu wireless pairing... (Ctrl+C để dừng)\n",
        style("→").cyan().bold()
    );

    backend.send(Command::StartWirelessPairing);

    // Đợi cho đến khi user nhấn Enter
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
   wLock<AppState>>,
) -> Result<()> {
    let tvs = state.read().await.apple_tvs.clone();

    if tvs.is_empty() {
        anyhow::bail!("Không tìm thấy Apple TV nào qua mDNS.");
    }

    let labels: Vec<String> = tvs.iter().map(|tv| tv.name.clone()).collect();
    let choice = Select::new("Chọn Apple TV:", labels).prompt()?;

    let tv = tvs
        .into_iter()
        .find(|t| t.name == choice)
        .context("Apple TV không tồn tại")?;

    println!(
        "\n{} Đang pair Apple TV {}...\n",
        style("→").cyan().bold(),
        style(&tv.name).cyan()
    );

    backend.send(Command::PairAppleTv(tv));

    tokio::time::sleep(Duration::from_secs(30)).await;
    Ok(())
}

async fn do_save_pairing(state: &Arc<RwLock<AppState>>) -> Result<()> {
    let s = state.read().await;

    if s.last_pairing.is_empty() {
        anyhow::bail!("Chưa có pairing file nào được tạo trong session này.");
    }

    let keys: Vec<String> = s.last_pairing.keys().cloned().collect();
    let choice = Select::new("Chọn pairing file để lưu:", keys).prompt()?;

    let info = s
        .last_pairing
        .get(&choice)
        .context("pairing info không tồn tại")?;

    let filename = format!(
        "{}_{}",
        choice.replace(':', "_"),
        info.file_name
    );

    std::fs::write(&filename, &info.bytes)
        .with_context(|| format!("không ghi được {}", filename))?;

    println!(
        "\n{} Đã lưu {} ({} bytes)\n",
        style("✓").green().bold(),
        style(&filename).cyan(),
        info.bytes.len()
    );

    Ok(())
}
