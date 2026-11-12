//! `cargo run --example check_original -- <path to original file>...`
//!
//! Runs the port's own loader over DiscordSRV's original files and prints a redacted summary: the names
//! of the top-level keys that the port reads and of those it does not, parse errors, and whether the
//! folder loads. It never prints a value, a token or a length.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use discordsrv_bridge::original;

fn main() {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("usage: check_original <file>...");
        std::process::exit(2);
    }
    let mut folders: BTreeSet<PathBuf> = BTreeSet::new();
    for path in &paths {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match std::fs::read_to_string(path) {
            Err(_) => println!("{name}: cannot read"),
            Ok(text) => {
                let s = original::summarize(&name, &text);
                match &s.error {
                    Some(e) => println!("{name}: parse error: {e}"),
                    None => {
                        println!("{name}: {} top-level keys", s.read.len() + s.unread.len());
                        println!("  read ({}): {}", s.read.len(), s.read.join(", "));
                        println!("  not read ({}): {}", s.unread.len(), s.unread.join(", "));
                    }
                }
            }
        }
        if let Some(folder) = path.parent() {
            folders.insert(folder.to_path_buf());
        }
    }
    for folder in folders {
        report_folder(&folder);
    }
}

fn report_folder(folder: &Path) {
    match original::load(folder) {
        Ok(Some(o)) => println!(
            "{}: loads; bot token {}; switched on, not ported: [{}]",
            folder.display(),
            if o.token.is_empty() { "absent" } else { "present" },
            o.unported.join("; ")
        ),
        Ok(None) => println!("{}: no config.yml here, nothing loaded", folder.display()),
        Err(e) => println!("{}: does not load: {e}", folder.display()),
    }
}
