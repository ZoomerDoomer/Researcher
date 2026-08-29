use bitcoin::Network;
use researcher_bitcoin_source::BitcoinCoreRpcSource;
use researcher_storage_redb::DurableStore;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = r#"Researcher

Usage:
  researcher status [options]
  researcher sync [options]

Common options:
  --db <path>                 redb database path (default: researcher.redb)
  --network <name>            bitcoin|testnet|signet|regtest (default: bitcoin)

Sync options:
  --rpc-url <url>             Bitcoin Core RPC URL (network default if omitted)
  --cookie-file <path>        Bitcoin Core cookie auth file
  --rpc-user <user>           RPC username (requires --rpc-password)
  --rpc-password <password>   RPC password (requires --rpc-user)
  --target-height <height>    stop exactly at this canonical block height

Examples:
  researcher status --db researcher.redb
  researcher sync --cookie-file /path/to/.cookie --target-height 1000
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Status,
    Sync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    command: Command,
    db_path: PathBuf,
    network: Network,
    rpc_url: String,
    cookie_file: Option<PathBuf>,
    rpc_user: Option<String>,
    rpc_password: Option<String>,
    target_height: Option<u32>,
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!();
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let config = parse_args(args)?;

    match config.command {
        Command::Status => {
            let store = DurableStore::open(&config.db_path, config.network)
                .map_err(|error| error.to_string())?;
            match store.tip().map_err(|error| error.to_string())? {
                Some(tip) => {
                    println!(
                        "network={} tip_height={} tip_hash={}",
                        network_name(config.network),
                        tip.height,
                        tip.hash
                    );
                }
                None => {
                    println!("network={} tip=empty", network_name(config.network));
                }
            }
        }
        Command::Sync => {
            let source = build_source(&config)?;
            let store = DurableStore::open(&config.db_path, config.network)
                .map_err(|error| error.to_string())?;

            let before = store.tip().map_err(|error| error.to_string())?;
            let stats = match config.target_height {
                Some(target) => store
                    .sync_to_height(&source, target)
                    .map_err(|error| error.to_string())?,
                None => store
                    .sync_to_tip(&source)
                    .map_err(|error| error.to_string())?,
            };
            let after = store.tip().map_err(|error| error.to_string())?;

            println!(
                "connected={} disconnected={} before={} after={}",
                stats.connected,
                stats.disconnected,
                format_tip(before),
                format_tip(after)
            );
        }
    }

    Ok(())
}

fn build_source(config: &Config) -> Result<BitcoinCoreRpcSource, String> {
    match (
        config.cookie_file.as_ref(),
        config.rpc_user.as_ref(),
        config.rpc_password.as_ref(),
    ) {
        (Some(cookie), None, None) => {
            BitcoinCoreRpcSource::cookie(&config.rpc_url, cookie).map_err(|error| error.to_string())
        }
        (None, Some(user), Some(password)) => {
            BitcoinCoreRpcSource::user_pass(&config.rpc_url, user, password)
                .map_err(|error| error.to_string())
        }
        _ => Err(
            "sync requires either --cookie-file or the pair --rpc-user/--rpc-password".to_owned(),
        ),
    }
}

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref() {
        Some("status") => Command::Status,
        Some("sync") => Command::Sync,
        Some("-h") | Some("--help") => return Err("help requested".to_owned()),
        Some(other) => return Err(format!("unknown command {other:?}")),
        None => return Err("missing command".to_owned()),
    };

    let mut db_path = PathBuf::from("researcher.redb");
    let mut network = Network::Bitcoin;
    let mut rpc_url = None;
    let mut cookie_file = None;
    let mut rpc_user = None;
    let mut rpc_password = None;
    let mut target_height = None;

    while let Some(flag) = args.next() {
        let value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };

        match flag.as_str() {
            "--db" => db_path = PathBuf::from(value()?),
            "--network" => network = parse_network(&value()?)?,
            "--rpc-url" => rpc_url = Some(value()?),
            "--cookie-file" => cookie_file = Some(PathBuf::from(value()?)),
            "--rpc-user" => rpc_user = Some(value()?),
            "--rpc-password" => rpc_password = Some(value()?),
            "--target-height" => {
                let raw = value()?;
                target_height = Some(
                    raw.parse::<u32>()
                        .map_err(|_| format!("invalid target height {raw:?}"))?,
                );
            }
            "-h" | "--help" => return Err("help requested".to_owned()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }

    let rpc_url = rpc_url.unwrap_or_else(|| default_rpc_url(network).to_owned());

    let config = Config {
        command,
        db_path,
        network,
        rpc_url,
        cookie_file,
        rpc_user,
        rpc_password,
        target_height,
    };

    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), String> {
    if config.command == Command::Status {
        if config.cookie_file.is_some()
            || config.rpc_user.is_some()
            || config.rpc_password.is_some()
            || config.target_height.is_some()
        {
            return Err("RPC and target-height options are only valid for sync".to_owned());
        }
        return Ok(());
    }

    let cookie = config.cookie_file.is_some();
    let user = config.rpc_user.is_some();
    let password = config.rpc_password.is_some();

    if cookie && (user || password) {
        return Err("choose cookie auth or user/password auth, not both".to_owned());
    }
    if user != password {
        return Err("--rpc-user and --rpc-password must be supplied together".to_owned());
    }
    if !cookie && !user {
        return Err(
            "sync requires either --cookie-file or the pair --rpc-user/--rpc-password".to_owned(),
        );
    }

    Ok(())
}

fn parse_network(value: &str) -> Result<Network, String> {
    match value {
        "bitcoin" | "mainnet" => Ok(Network::Bitcoin),
        "testnet" => Ok(Network::Testnet),
        "signet" => Ok(Network::Signet),
        "regtest" => Ok(Network::Regtest),
        other => Err(format!("unsupported network {other:?}")),
    }
}

fn default_rpc_url(network: Network) -> &'static str {
    if network == Network::Bitcoin {
        "http://127.0.0.1:8332"
    } else if network == Network::Testnet {
        "http://127.0.0.1:18332"
    } else if network == Network::Signet {
        "http://127.0.0.1:38332"
    } else if network == Network::Regtest {
        "http://127.0.0.1:18443"
    } else {
        "http://127.0.0.1:8332"
    }
}

fn network_name(network: Network) -> &'static str {
    if network == Network::Bitcoin {
        "bitcoin"
    } else if network == Network::Testnet {
        "testnet"
    } else if network == Network::Signet {
        "signet"
    } else if network == Network::Regtest {
        "regtest"
    } else {
        "unknown"
    }
}

fn format_tip(tip: Option<researcher_storage_redb::DurableTip>) -> String {
    match tip {
        Some(tip) => format!("{}:{}", tip.height, tip.hash),
        None => "empty".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_cookie_sync() {
        let config = parse_args(vec![
            "sync".to_owned(),
            "--network".to_owned(),
            "regtest".to_owned(),
            "--db".to_owned(),
            "test.redb".to_owned(),
            "--cookie-file".to_owned(),
            "/tmp/cookie".to_owned(),
            "--target-height".to_owned(),
            "100".to_owned(),
        ])
        .unwrap();

        assert_eq!(config.command, Command::Sync);
        assert_eq!(config.network, Network::Regtest);
        assert_eq!(config.db_path, PathBuf::from("test.redb"));
        assert_eq!(config.rpc_url, "http://127.0.0.1:18443");
        assert_eq!(config.target_height, Some(100));
    }

    #[test]
    fn sync_requires_auth() {
        let error = parse_args(vec!["sync".to_owned()]).unwrap_err();
        assert!(error.contains("requires either"));
    }

    #[test]
    fn mixed_auth_is_rejected() {
        let error = parse_args(vec![
            "sync".to_owned(),
            "--cookie-file".to_owned(),
            "/tmp/cookie".to_owned(),
            "--rpc-user".to_owned(),
            "user".to_owned(),
            "--rpc-password".to_owned(),
            "secret".to_owned(),
        ])
        .unwrap_err();

        assert!(error.contains("not both"));
    }

    #[test]
    fn status_rejects_sync_only_options() {
        let error = parse_args(vec![
            "status".to_owned(),
            "--target-height".to_owned(),
            "1".to_owned(),
        ])
        .unwrap_err();

        assert!(error.contains("only valid for sync"));
    }
}
