//! nilbox-blocklist-build — build and check blocklist.bin
//!
//! Subcommands:
//!   build   Build a blocklist.bin from sources
//!   check   Test whether domains are blocked

use anyhow::Result;
use clap::{Parser, Subcommand};
use nilbox_blocklist::BloomBlocklist;

#[derive(Parser, Debug)]
#[command(name = "nilbox-blocklist-build", about = "nilbox domain blocklist tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Build a blocklist.bin from domain sources
    Build {
        /// Output file path
        #[arg(long, default_value = "blocklist.bin")]
        output: String,
        /// Comma-separated source list (oisd,urlhaus)
        #[arg(long, default_value = "oisd")]
        sources: String,
        /// Path to extra deny domains (one per line)
        #[arg(long)]
        user_deny: Option<String>,
        /// Path to domains to exclude (one per line)
        #[arg(long)]
        user_allow: Option<String>,
        /// False positive rate (0.0–1.0)
        #[arg(long, default_value_t = 0.01)]
        fp_rate: f64,
        /// Ed25519 private key PEM path for signing
        #[arg(long)]
        sign_key: Option<String>,
        /// Category flags bitmask (default: 0xFF = all)
        #[arg(long, default_value_t = 0xFF)]
        category: u8,
        /// Offline source directory (skips network download)
        #[arg(long)]
        offline: Option<String>,
    },

    /// Check whether one or more domains are blocked
    Check {
        /// Path to blocklist.bin
        #[arg(long, default_value = "blocklist.bin")]
        blocklist: String,
        /// Domains to test (space-separated)
        #[arg(required = true)]
        domains: Vec<String>,
        /// Verify Ed25519 signature (required for CDN downloads; off by default for local builds)
        #[arg(long)]
        verify: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "nilbox_blocklist=info".to_string())
                .as_str(),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Build {
            output, sources, user_deny, user_allow,
            fp_rate, sign_key, category, offline,
        } => {
            use nilbox_blocklist::builder::{build_blocklist, BuilderConfig};

            let sources: Vec<String> = sources.split(',').map(str::trim).map(String::from).collect();
            let config = BuilderConfig {
                sources,
                user_deny_path: user_deny,
                user_allow_path: user_allow,
                fp_rate,
                sign_key_path: sign_key,
                category_flags: category,
                offline_dir: offline,
            };

            let data = build_blocklist(config).await?;
            std::fs::write(&output, &data)?;

            let bl = BloomBlocklist::load(&data, false)?;
            println!(
                "Written: {}  ({} domains, {:.1} KB, timestamp={})",
                output,
                bl.domain_count(),
                data.len() as f64 / 1024.0,
                bl.build_timestamp(),
            );
        }

        Command::Check { blocklist, domains, verify } => {
            let data = std::fs::read(&blocklist)
                .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", blocklist, e))?;
            let bl = BloomBlocklist::load(&data, verify)?;

            println!(
                "Blocklist: {} domains, timestamp={}, verified={}",
                bl.domain_count(),
                bl.build_timestamp(),
                bl.is_signature_verified(),
            );
            println!();

            let mut any_blocked = false;
            for domain in &domains {
                let blocked = bl.contains(domain);
                if blocked { any_blocked = true; }
                println!(
                    "  {}  {}",
                    if blocked { "BLOCKED" } else { "allowed" },
                    domain,
                );
            }

            // Exit code 1 if any domain is blocked (useful in scripts)
            if any_blocked {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
