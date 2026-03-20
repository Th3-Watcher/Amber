use amber_core::{
    config::Config,
    ipc::DaemonCommand,
};
use anyhow::Result;
use notify::EventKind;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::mpsc,
};
use tracing::{error, info, warn};

mod daemon;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let amber_dir = PathBuf::from(&home).join(".amber");
    std::fs::create_dir_all(&amber_dir)?;

    // Write PID file
    std::fs::write(amber_dir.join("amberd.pid"), std::process::id().to_string())?;

    let config = Config::load()?;
    let socket_path = amber_dir.join("amberd.sock");

    // Remove stale socket
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    info!("amberd started, listening on {:?}", socket_path);

    let state = Arc::new(Mutex::new(daemon::DaemonState::new(config.clone())?));

    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Event>();

    {
        let tx_watcher = tx.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx_watcher.send(event);
            }
        })?;
        let mut s = state.lock().unwrap();
        s.set_watcher(watcher, tx);
    }

    // Event processing loop
    let state_for_events = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let EventKind::Modify(_) | EventKind::Create(_) = event.kind {
                for path in event.paths {
                    if path.is_file() {
                        let mut s = state_for_events.lock().unwrap();
                        if let Err(e) = s.handle_file_event(&path) {
                            warn!("Error handling event for {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    });

    // IPC accept loop
    loop {
        let (stream, _) = listener.accept().await?;
        let state_clone = Arc::clone(&state);

        tokio::spawn(async move {
            let mut stream = stream;
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_err() {
                return;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            if stream.read_exact(&mut buf).await.is_err() {
                return;
            }
            let cmd: DaemonCommand = match bincode::deserialize(&buf) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to deserialize command: {}", e);
                    return;
                }
            };

            let response = {
                let mut s = state_clone.lock().unwrap();
                s.handle_command(cmd)
            };

            let resp_bytes = bincode::serialize(&response).unwrap_or_default();
            let resp_len = (resp_bytes.len() as u32).to_le_bytes();
            let _ = stream.write_all(&resp_len).await;
            let _ = stream.write_all(&resp_bytes).await;
        });
    }
}
