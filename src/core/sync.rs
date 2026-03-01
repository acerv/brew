// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Spawn a background thread that runs `cmd` via `sh -c` every `interval`
/// seconds. Results are sent through `tx`: `None` on success, `Some(msg)` on
/// failure. The thread exits when `shutdown` is disconnected (i.e. the sender
/// is dropped).
pub fn spawn(
    cmd: String,
    interval: u64,
    tx: mpsc::Sender<Option<String>>,
    shutdown: mpsc::Receiver<()>,
) {
    thread::spawn(move || {
        loop {
            match shutdown.recv_timeout(Duration::from_secs(interval)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let msg = match Command::new("sh").args(["-c", &cmd]).output() {
                Ok(out) if out.status.success() => None,
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let code = out.status.code().unwrap_or(-1);
                    Some(if stderr.is_empty() {
                        format!("exit {}", code)
                    } else {
                        stderr
                    })
                }
                Err(e) => Some(e.to_string()),
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
}
