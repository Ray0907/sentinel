#![allow(dead_code)]

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

mod actor;
mod actuator;
mod cdp;
mod daemon;
mod diff;
mod query;
mod recording;
mod sensor;

#[derive(Parser)]
#[command(
    name = "sentinel",
    about = "Continuous-observation agent browser framework"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Chrome executable path
    #[arg(long, default_value = "google-chrome")]
    chrome: String,

    /// Remote debugging port
    #[arg(long, default_value_t = 9222)]
    port: u16,

    /// Disable headless mode (show browser window)
    #[arg(long)]
    no_headless: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Navigate to a URL and start observing
    Navigate { url: String },
    /// Observe the current page (attach to existing Chrome)
    Observe {
        /// Duration in seconds to observe
        #[arg(short, long, default_value_t = 10)]
        duration: u64,
    },
    /// Navigate to URL, then click an element and observe
    Run {
        /// URL to navigate to first
        #[arg(long)]
        url: String,
        /// CSS selector to click after page loads
        #[arg(long)]
        click: Option<String>,
        /// Text to type (requires --into selector)
        #[arg(long)]
        text: Option<String>,
        /// CSS selector to type into
        #[arg(long)]
        into: Option<String>,
        /// Observation duration in seconds
        #[arg(short, long, default_value_t = 10)]
        duration: u64,
    },
    /// Watch a page continuously, streaming all events in real-time
    Watch {
        /// URL to navigate to
        url: String,
        /// Duration in seconds (0 = indefinite until Ctrl-C)
        #[arg(short, long, default_value_t = 30)]
        duration: u64,
        /// Filter: only show events matching this type (dom, network, console, layout, error, all)
        #[arg(short, long, default_value = "all")]
        filter: String,
    },
    /// Click an element on the current page
    Click { selector: String },
    /// Type text into an element
    Type { selector: String, text: String },
    /// Take a compatibility snapshot
    Snapshot,
    /// Record a full session to a JSON file for later replay/analysis
    Record {
        /// URL to navigate to
        url: String,
        /// Output file path (default: sentinel-recording-{timestamp}.json)
        #[arg(short, long)]
        output: Option<String>,
        /// Duration in seconds
        #[arg(short, long, default_value_t = 30)]
        duration: u64,
        /// CSS selector to click after page loads
        #[arg(long)]
        click: Option<String>,
    },
    /// Replay/analyze a recorded session file
    Replay {
        /// Path to the recording JSON file
        file: String,
        /// Show summary only (default: full timeline)
        #[arg(short, long)]
        summary: bool,
    },
    /// Daemon mode: persistent browser session
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Send a command to a running daemon
    Send {
        /// Command: navigate, click, type, snapshot, ping, shutdown
        action: String,
        /// URL for navigate
        #[arg(long)]
        url: Option<String>,
        /// CSS selector for click/type
        #[arg(long)]
        selector: Option<String>,
        /// Text for type
        #[arg(long)]
        text: Option<String>,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon (background Chrome + socket server)
    Start,
    /// Stop a running daemon
    Stop,
    /// Check if daemon is running
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    // Handle daemon and send commands separately (don't launch Chrome)
    match &cli.command {
        Commands::Daemon { action } => match action {
            DaemonAction::Start => {
                return start_daemon(&cli).await;
            }
            DaemonAction::Stop => {
                let cmd = daemon::DaemonCommand {
                    action: "shutdown".to_string(),
                    url: None,
                    selector: None,
                    text: None,
                    duration: None,
                    filter: None,
                };
                return daemon::send_to_daemon(cli.port, cmd).await;
            }
            DaemonAction::Status => {
                if daemon::is_running(cli.port).await {
                    println!("Daemon is running on port {}", cli.port);
                } else {
                    println!("No daemon running on port {}", cli.port);
                }
                return Ok(());
            }
        },
        Commands::Replay {
            ref file,
            ref summary,
        } => {
            let rec = recording::Recording::load(file)?;
            if *summary {
                rec.print_summary();
            } else {
                rec.print_timeline();
            }
            return Ok(());
        }
        Commands::Send {
            action,
            url,
            selector,
            text,
        } => {
            let cmd = daemon::DaemonCommand {
                action: action.clone(),
                url: url.clone(),
                selector: selector.clone(),
                text: text.clone(),
                duration: None,
                filter: None,
            };
            return daemon::send_to_daemon(cli.port, cmd).await;
        }
        _ => {} // Fall through to standard flow
    }

    let browser = cdp::browser::Browser::launch(&cli.chrome, cli.port, !cli.no_headless).await?;
    tracing::info!(port = cli.port, "Connected to Chrome");

    let cdp_client = cdp::client::CdpClient::connect(&browser.ws_url).await?;
    tracing::info!("CDP WebSocket connected");

    let (sensor_tx, sensor_rx) = tokio::sync::mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(256);
    let (report_tx, mut report_rx) = tokio::sync::mpsc::channel(64);

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sensor_client = cdp_client.clone();
    let sensor_handle =
        tokio::spawn(async move { sensor::run(sensor_client, sensor_tx, Some(ready_tx)).await });

    ready_rx
        .await
        .map_err(|_| anyhow::anyhow!("Sensor failed to initialize"))?;
    tracing::info!("Sensor ready");

    // Check if we need streaming mode (Watch or Record command)
    let needs_stream = matches!(
        cli.command,
        Commands::Watch { .. } | Commands::Record { .. }
    );
    let (stream_tx, stream_rx) = if needs_stream {
        let (tx, rx) = tokio::sync::mpsc::channel::<actuator::StreamEvent>(4096);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let actor_client = cdp_client.clone();
    let actor_handle = tokio::spawn(async move {
        actor::run_with_stream(actor_client, sensor_rx, cmd_rx, report_tx, stream_tx).await
    });

    match cli.command {
        Commands::Navigate { url } => {
            cmd_tx
                .send(actuator::ActuatorCommand::Navigate { url })
                .await?;
            query::commands::observe(report_rx, 10).await?;
        }
        Commands::Run {
            url,
            click,
            text,
            into,
            duration,
        } => {
            // Step 1: Navigate
            cmd_tx
                .send(actuator::ActuatorCommand::Navigate { url })
                .await?;
            // Wait for navigation report
            let mut report_rx = query::commands::observe_until_settled(report_rx).await?;

            // Step 2: Click (if specified)
            if let Some(selector) = click {
                cmd_tx
                    .send(actuator::ActuatorCommand::Click { selector })
                    .await?;
                report_rx = query::commands::observe_until_settled(report_rx).await?;
            }

            // Step 3: Type (if specified)
            if let (Some(text), Some(selector)) = (text, into) {
                cmd_tx
                    .send(actuator::ActuatorCommand::Type { selector, text })
                    .await?;
                report_rx = query::commands::observe_until_settled(report_rx).await?;
            }

            // Continue observing for remaining duration
            query::commands::observe(report_rx, duration).await?;
        }
        Commands::Observe { duration } => {
            query::commands::observe(report_rx, duration).await?;
        }
        Commands::Watch {
            url,
            duration,
            filter,
        } => {
            // Navigate first
            cmd_tx
                .send(actuator::ActuatorCommand::Navigate { url })
                .await?;

            // Stream events in real-time
            if let Some(stream_rx) = stream_rx {
                query::commands::watch_stream(stream_rx, duration, &filter).await?;
            }
        }
        Commands::Click { selector } => {
            cmd_tx
                .send(actuator::ActuatorCommand::Click { selector })
                .await?;
            query::commands::observe(report_rx, 5).await?;
        }
        Commands::Type { selector, text } => {
            cmd_tx
                .send(actuator::ActuatorCommand::Type { selector, text })
                .await?;
            query::commands::observe(report_rx, 5).await?;
        }
        Commands::Snapshot => {
            cmd_tx.send(actuator::ActuatorCommand::Snapshot).await?;
            query::commands::observe(report_rx, 2).await?;
        }
        Commands::Record {
            url,
            output,
            duration,
            click,
        } => {
            let mut rec = recording::Recording::new(&url);

            // Navigate
            cmd_tx
                .send(actuator::ActuatorCommand::Navigate { url })
                .await?;
            report_rx = query::commands::observe_until_settled(report_rx).await?;

            // Click if specified
            if let Some(selector) = click {
                cmd_tx
                    .send(actuator::ActuatorCommand::Click { selector })
                    .await?;
                report_rx = query::commands::observe_until_settled(report_rx).await?;
            }

            // Enable streaming for recording
            cmd_tx
                .send(actuator::ActuatorCommand::EnableStreaming)
                .await?;

            // Record stream events for the duration
            if let Some(stream_rx) = stream_rx {
                let deadline =
                    tokio::time::Instant::now() + tokio::time::Duration::from_secs(duration);
                let mut stream_rx = stream_rx;

                loop {
                    tokio::select! {
                        Some(event) = stream_rx.recv() => {
                            rec.add_event(event);
                        }
                        Some(report) = report_rx.recv() => {
                            rec.add_report(report);
                        }
                        _ = tokio::time::sleep_until(deadline) => {
                            break;
                        }
                    }
                }
            }

            // Save recording
            let output_path = output.unwrap_or_else(|| {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                format!("sentinel-recording-{ts}.json")
            });

            rec.save(&output_path)?;
            rec.print_summary();
            println!("\nRecording saved to: {output_path}");
        }
        // Replay, Daemon and Send are handled above before Chrome launch
        Commands::Replay { .. } | Commands::Daemon { .. } | Commands::Send { .. } => unreachable!(),
    }

    drop(cmd_tx);
    sensor_handle.abort();
    actor_handle.abort();
    browser.shutdown().await?;

    Ok(())
}

/// Start the daemon: launch Chrome, set up actor, listen on Unix socket.
async fn start_daemon(cli: &Cli) -> Result<()> {
    if daemon::is_running(cli.port).await {
        eprintln!("Daemon already running on port {}", cli.port);
        std::process::exit(1);
    }

    let browser = cdp::browser::Browser::launch(&cli.chrome, cli.port, !cli.no_headless).await?;
    tracing::info!(port = cli.port, "Daemon: Chrome launched");

    let cdp_client = cdp::client::CdpClient::connect(&browser.ws_url).await?;
    tracing::info!("Daemon: CDP connected");

    let (sensor_tx, sensor_rx) = tokio::sync::mpsc::channel(4096);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(256);
    let (report_tx, report_rx) = tokio::sync::mpsc::channel(64);

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let sensor_client = cdp_client.clone();
    tokio::spawn(async move {
        let _ = sensor::run(sensor_client, sensor_tx, Some(ready_tx)).await;
    });

    ready_rx
        .await
        .map_err(|_| anyhow::anyhow!("Sensor failed to initialize"))?;
    tracing::info!("Daemon: Sensor ready");

    let actor_client = cdp_client.clone();
    tokio::spawn(async move {
        let _ = actor::run(actor_client, sensor_rx, cmd_rx, report_tx).await;
    });

    tracing::info!(
        port = cli.port,
        "Daemon started. Use 'sentinel send' to send commands."
    );

    // Run the daemon socket server (blocks until shutdown)
    daemon::run_daemon(cli.port, cmd_tx, report_rx).await?;

    browser.shutdown().await?;
    Ok(())
}
