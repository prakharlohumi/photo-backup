use anyhow::Context;
use photo_backup_core::{BackupController, BackupSettings};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    let controller = BackupController::new(BackupSettings {
        source_root: args.source_root.clone(),
        state_dir: args.state_dir.clone(),
        client_id: args.client_id.clone(),
        client_secret: args.client_secret.clone(),
    })?;
    controller.refresh_state()?;

    println!("photo-backup TUI");
    println!("source: {}", args.source_root.display());
    println!("state:  {}", args.state_dir.display());
    println!("commands: start, pause, resume, status, rescan, clean, quit");

    let mut line = String::new();
    loop {
        print!("backup> ");
        io::stdout().flush()?;
        line.clear();
        if io::stdin().read_line(&mut line)? == 0 {
            controller.stop()?;
            print_status_report(&controller.snapshot()?);
            break;
        }

        let command = line.trim();
        match command {
            "start" => {
                controller.start()?;
                println!("started");
            }
            "pause" => {
                controller.pause();
                println!("paused");
            }
            "resume" => {
                controller.resume()?;
                println!("resumed");
            }
            "status" => {
                print_status_report(&controller.snapshot()?);
            }
            "rescan" => {
                controller.refresh_state()?;
                println!("rescanned");
            }
            "clean" => {
                controller.clean()?;
                println!("backup state cleaned");
            }
            "quit" | "exit" => {
                controller.stop()?;
                print_status_report(&controller.snapshot()?);
                break;
            }
            "" => {}
            other => {
                println!("unknown command: {other}");
            }
        }
    }

    Ok(())
}

fn print_status_report(snapshot: &photo_backup_core::BackupSnapshot) {
    println!("--- backup status ---");
    println!("source root: {}", snapshot.source_root.display());
    println!("total items: {}", snapshot.total_items);
    println!("committed: {}", snapshot.committed);
    println!("skipped: {}", snapshot.skipped);
    println!("failed: {}", snapshot.failed);
    println!("retrying: {}", snapshot.retrying);
    println!("queued: {}", snapshot.queued);
    println!("uploading: {}", snapshot.uploading);
    println!("paused: {}", snapshot.paused);
    println!("running: {}", snapshot.running);
    if let Some(current_item) = &snapshot.current_item {
        println!("current item: {current_item}");
    }
    if let Some(message) = &snapshot.last_message {
        println!("last message: {message}");
    }

    if !snapshot.failed_items.is_empty() {
        println!("failed files:");
        for item in &snapshot.failed_items {
            if let Some(error) = &item.error {
                println!(
                    "  - {} (attempts: {}, error: {})",
                    item.path, item.attempts, error
                );
            } else {
                println!(
                    "  - {} (attempts: {}, no error recorded)",
                    item.path, item.attempts
                );
            }
        }
    } else {
        println!("failed files: none");
    }

    if !snapshot.skipped_items.is_empty() {
        println!("skipped files:");
        for item in &snapshot.skipped_items {
            if let Some(reason) = &item.reason {
                println!("  - {} ({reason})", item.path);
            } else {
                println!("  - {}", item.path);
            }
        }
    }

    println!("---------------------");
}

#[derive(Debug, Clone)]
struct Args {
    source_root: PathBuf,
    state_dir: PathBuf,
    client_id: String,
    client_secret: Option<String>,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut positional = Vec::new();
        let mut source_root = None;
        let mut state_dir = None;
        let mut client_id = env::var("GOOGLE_CLIENT_ID").ok();
        let mut client_secret = env::var("GOOGLE_CLIENT_SECRET").ok();

        let mut iter = env::args().skip(1).peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--source" => {
                    source_root = Some(
                        iter.next()
                            .context("--source requires a path argument")?
                            .into(),
                    );
                }
                "--state-dir" => {
                    state_dir = Some(
                        iter.next()
                            .context("--state-dir requires a path argument")?
                            .into(),
                    );
                }
                "--client-id" => {
                    client_id = Some(iter.next().context("--client-id requires a value")?);
                }
                "--client-secret" => {
                    client_secret = Some(iter.next().context("--client-secret requires a value")?);
                }
                other if other.starts_with('-') => {
                    anyhow::bail!("unknown flag {other}");
                }
                other => positional.push(other.to_string()),
            }
        }

        let source_root = source_root
            .or_else(|| positional.first().cloned().map(PathBuf::from))
            .context("missing source path. usage: photo-backup <source> [--state-dir DIR]")?;
        let state_dir = state_dir.unwrap_or_else(|| source_root.join(".photo-backup-state"));
        let client_id = client_id
            .context("missing Google client ID. set GOOGLE_CLIENT_ID or pass --client-id")?;

        Ok(Self {
            source_root,
            state_dir,
            client_id,
            client_secret,
        })
    }
}
