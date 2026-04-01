//! Chrome process management. Launches Chrome with --remote-debugging-port,
//! connects to a page-level WebSocket target (not browser-level).

use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct Browser {
    pub ws_url: String,
    child: Option<Child>,
    port: u16,
}

impl Browser {
    /// Launch Chrome with remote debugging enabled.
    /// Returns a Browser with ws_url pointing to a page-level target.
    pub async fn launch(chrome_path: &str, port: u16, headless: bool) -> Result<Self> {
        // First, try to connect to an already-running Chrome instance
        if let Ok(ws_url) = Self::fetch_page_ws_url(port).await {
            tracing::info!(port, "Attached to existing Chrome page target");
            return Ok(Self {
                ws_url,
                child: None,
                port,
            });
        }

        // Use a temporary user data dir for clean profile
        let tmp_dir = std::env::temp_dir().join(format!("sentinel-chrome-{port}"));
        let _ = std::fs::create_dir_all(&tmp_dir);

        // Launch a new Chrome process
        let mut args = vec![
            format!("--remote-debugging-port={port}"),
            format!("--user-data-dir={}", tmp_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-client-side-phishing-detection".to_string(),
            "--disable-default-apps".to_string(),
            "--disable-extensions".to_string(),
            "--disable-component-extensions-with-background-pages".to_string(),
            "--disable-hang-monitor".to_string(),
            "--disable-popup-blocking".to_string(),
            "--disable-prompt-on-repost".to_string(),
            "--disable-sync".to_string(),
            "--disable-translate".to_string(),
            "--metrics-recording-only".to_string(),
            "--safebrowsing-disable-auto-update".to_string(),
            "--enable-features=NetworkService,NetworkServiceInProcess".to_string(),
        ];

        if headless {
            args.push("--headless=new".to_string());
        }

        args.push("about:blank".to_string());

        let child = Command::new(chrome_path)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("Failed to launch Chrome at '{chrome_path}': {e}"))?;

        tracing::info!(pid = child.id(), port, "Chrome process launched");

        // Wait for Chrome to be ready and get a page target
        let ws_url = Self::wait_for_ready(port).await?;

        Ok(Self {
            ws_url,
            child: Some(child),
            port,
        })
    }

    /// Fetch the WebSocket URL for a page-type target from Chrome's /json endpoint.
    /// This gives us a page-level connection where Page.navigate etc. work.
    async fn fetch_page_ws_url(port: u16) -> Result<String> {
        let url = format!("http://127.0.0.1:{port}/json");
        let resp = reqwest::get(&url).await?;
        let targets: Vec<serde_json::Value> = resp.json().await?;

        // Find the first "page" type target
        for target in &targets {
            if target["type"].as_str() == Some("page") {
                if let Some(ws_url) = target["webSocketDebuggerUrl"].as_str() {
                    return Ok(ws_url.to_string());
                }
            }
        }

        Err(anyhow!("No page target found in /json response"))
    }

    /// Poll until Chrome is ready, up to 20 seconds.
    async fn wait_for_ready(port: u16) -> Result<String> {
        for i in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            match Self::fetch_page_ws_url(port).await {
                Ok(url) => {
                    tracing::info!(attempt = i + 1, "Chrome page target is ready");
                    return Ok(url);
                }
                Err(_) => continue,
            }
        }
        Err(anyhow!(
            "Chrome did not become ready within 20 seconds on port {port}"
        ))
    }

    /// Gracefully shut down Chrome.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            tracing::info!("Shutting down Chrome");
            let _ = child.kill().await;
        }
        Ok(())
    }
}
