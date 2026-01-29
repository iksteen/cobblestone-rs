use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgAction, Parser, Subcommand};

mod config;
mod rockbox;
mod scrobble;
mod service;

use crate::config::{
    ServiceKeys, add_account, default_config_path, get_service_keys, iter_accounts, load_config,
    remove_account, save_config, set_service_keys,
};
use crate::rockbox::{TagCache, parse_playback_log};
use crate::scrobble::build_scrobble_tracks;
use crate::service::{ScrobbleClient, Service};

#[derive(Parser)]
#[command(
    name = "cobblestone",
    version,
    about = "Scrobble Rockbox playback logs"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    Scrobble(ScrobbleArgs),
}

#[derive(Subcommand)]
enum ServiceCommand {
    SetKeys {
        service: String,
        #[arg(long, help = "API key")]
        api_key: String,
        #[arg(long, help = "API secret")]
        api_secret: String,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    Add {
        service: String,
        #[arg(long, help = "Account username")]
        username: String,
        #[arg(long, help = "Account password")]
        password: Option<String>,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
    Remove {
        service: String,
        #[arg(long, help = "Account username")]
        username: String,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
    List {
        #[arg(long, help = "Filter by service")]
        service: Option<String>,
        #[arg(long, value_name = "PATH")]
        config_path: Option<PathBuf>,
    },
}

#[derive(Parser)]
struct ScrobbleArgs {
    #[arg(
        long,
        default_value = ".rockbox",
        help = "Path to the .rockbox directory"
    )]
    rockbox_dir: PathBuf,
    #[arg(long, help = "Optional path to playback.log")]
    playback_log: Option<PathBuf>,
    #[arg(long, help = "Limit to one service")]
    service: Option<String>,
    #[arg(long, help = "Limit to one username")]
    username: Option<String>,
    #[arg(long, value_name = "PATH")]
    config_path: Option<PathBuf>,
    #[arg(
        long = "no-truncate",
        action = ArgAction::SetFalse,
        default_value_t = true,
        help = "Do not truncate playback.log after success"
    )]
    truncate: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Parse and report without scrobbling"
    )]
    dry_run: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Print raw scrobble API responses"
    )]
    debug_response: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Service { command } => match command {
            ServiceCommand::SetKeys {
                service,
                api_key,
                api_secret,
                config_path,
            } => {
                if service != "lastfm" {
                    bail!("Only lastfm supports setting API keys.");
                }
                let config_path = config_path.unwrap_or_else(default_config_path);
                let mut config = load_config(&config_path)?;
                set_service_keys(&mut config, &service, &api_key, &api_secret);
                save_config(&config, &config_path)?;
                println!("Saved API keys for {service} in {}", config_path.display());
            }
        },
        Commands::Account { command } => handle_account(command)?,
        Commands::Scrobble(args) => handle_scrobble(args)?,
    }
    Ok(())
}

fn handle_account(command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Add {
            service,
            username,
            password,
            config_path,
        } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let mut config = load_config(&config_path)?;
            let password = match password {
                Some(value) => value,
                None => prompt_password_confirm()?,
            };
            add_account(&mut config, &service, &username, &password);
            save_config(&config, &config_path)?;
            println!(
                "Saved {service} account for {username} in {}",
                config_path.display()
            );
        }
        AccountCommand::Remove {
            service,
            username,
            config_path,
        } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let mut config = load_config(&config_path)?;
            if !remove_account(&mut config, &service, &username) {
                bail!("No account found for {service} {username}");
            }
            save_config(&config, &config_path)?;
            println!("Removed {service} account for {username}");
        }
        AccountCommand::List {
            service,
            config_path,
        } => {
            let config_path = config_path.unwrap_or_else(default_config_path);
            let config = load_config(&config_path)?;
            let accounts: Vec<_> = iter_accounts(&config, service.as_deref()).collect();
            if accounts.is_empty() {
                bail!("No accounts configured.");
            }
            for account in accounts {
                println!("{}\t{}", account.service, account.username);
            }
        }
    }
    Ok(())
}

fn handle_scrobble(args: ScrobbleArgs) -> Result<()> {
    let config_path = args.config_path.unwrap_or_else(default_config_path);
    let config = load_config(&config_path)?;
    let accounts: Vec<_> = iter_accounts(&config, args.service.as_deref())
        .filter(|account| {
            args.username
                .as_deref()
                .is_none_or(|user| account.username == user)
        })
        .cloned()
        .collect();
    if accounts.is_empty() {
        bail!("No matching accounts configured.");
    }

    let playback_path = args
        .playback_log
        .unwrap_or_else(|| args.rockbox_dir.join("playback.log"));
    if !playback_path.exists() {
        bail!("Missing playback log at {}", playback_path.display());
    }

    let entries = parse_playback_log(&playback_path)?;
    if entries.is_empty() {
        bail!("No playback entries found.");
    }

    let mut tagcache = TagCache::new(&args.rockbox_dir)?;
    let (tracks, missing) = build_scrobble_tracks(&entries, &mut tagcache)?;
    tagcache.close();

    if !missing.is_empty() {
        println!("Missing metadata for {} paths", missing.len());
    }
    if tracks.is_empty() {
        bail!("No scrobble-eligible tracks found.");
    }
    if args.dry_run {
        println!("Would scrobble {} tracks.", tracks.len());
        return Ok(());
    }

    let failures = scrobble_for_accounts(&config, &accounts, &tracks, args.debug_response)?;

    if failures > 0 {
        println!("Finished with {failures} scrobble failures.");
    }
    if args.truncate {
        truncate_playback_log(&playback_path)?;
        println!("Truncated {}", playback_path.display());
    }
    Ok(())
}

fn scrobble_for_accounts(
    config: &config::Config,
    accounts: &[config::Account],
    tracks: &[scrobble::ScrobbleTrack],
    debug_response: bool,
) -> Result<usize> {
    let mut failures = 0;
    for account in accounts {
        let keys = if account.service == "librefm" {
            ServiceKeys {
                api_key: "cobblestone".to_string(),
                api_secret: "cobblestone".to_string(),
            }
        } else {
            let Some(keys) = get_service_keys(config, &account.service) else {
                println!("Missing API keys for {}", account.service);
                failures += 1;
                continue;
            };
            keys.clone()
        };
        let service = Service::parse(&account.service)?;
        match ScrobbleClient::new(service, &keys, account, debug_response) {
            Ok(client) => {
                let errors = client.scrobble_tracks(tracks);
                let error_count = errors.len();
                if error_count == 0 {
                    println!(
                        "Scrobbled {} tracks to {} for {}",
                        tracks.len(),
                        account.service,
                        account.username
                    );
                } else {
                    println!(
                        "Scrobbled {} tracks to {} for {} with {} failures:",
                        tracks.len(),
                        account.service,
                        account.username,
                        error_count
                    );
                    for error in errors {
                        println!("  {error}");
                    }
                    failures += error_count;
                }
            }
            Err(err) => {
                println!("Failed scrobbling to {}: {err}", account.service);
                failures += 1;
            }
        }
    }
    Ok(failures)
}

fn truncate_playback_log(path: &Path) -> Result<()> {
    std::fs::write(path, "").map_err(|err| anyhow::anyhow!(err))
}

fn prompt_password_confirm() -> Result<String> {
    let password = rpassword::prompt_password("Password: ")?;
    let confirm = rpassword::prompt_password("Confirm password: ")?;
    if password != confirm {
        bail!("Passwords do not match.");
    }
    Ok(password)
}
