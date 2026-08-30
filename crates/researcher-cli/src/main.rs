use bitcoin::Network;
use researcher_bitcoin_source::{BitcoinCoreRpcSource, NodeStatus};
use researcher_storage_redb::{DurableStore, DurableTip};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const DEFAULT_PRUNE_LAG_BLOCKS: u32 = 10_000;
const DEFAULT_BATCH_BLOCKS: u32 = 5_000;
const DEFAULT_POLL_SECONDS: u64 = 5;
const MIN_PRUNE_LAG_BLOCKS: u32 = 1_000;

const USAGE: &str = r#"Researcher

Usage:
  researcher status [options]
  researcher doctor [options]
  researcher sync [options]
  researcher backfill [options]

Common options:
  --db <path>                   redb database path (default: researcher.redb)
  --network <name>              bitcoin|testnet|signet|regtest (default: bitcoin)

RPC options (doctor/sync/backfill):
  --rpc-url <url>               Bitcoin Core RPC URL (network default if omitted)
  --cookie-file <path>          Bitcoin Core cookie auth file
  --rpc-user <user>             RPC username (requires --rpc-password)
  --rpc-password <password>     RPC password (requires --rpc-user)

Doctor/sync/backfill:
  --target-height <height>      bounded target height

Backfill:
  --prune-lag-blocks <count>    retain this many committed blocks in Core (default: 10000)
  --batch-blocks <count>        max blocks committed before pruning opportunity (default: 5000)
  --poll-seconds <seconds>      wait when caught up to current IBD tip (default: 5)

Examples:
  researcher status --db researcher.redb
  researcher doctor --cookie-file /path/to/.cookie --target-height 1000
  researcher sync --cookie-file /path/to/.cookie --target-height 1000
  researcher backfill --cookie-file /path/to/.cookie --target-height 11000 --db researcher.redb
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Status,
    Doctor,
    Sync,
    Backfill,
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
    prune_lag_blocks: u32,
    batch_blocks: u32,
    poll_seconds: u64,
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
        Command::Status => run_status(&config),
        Command::Doctor => run_doctor(&config),
        Command::Sync => run_sync(&config),
        Command::Backfill => run_backfill(&config),
    }
}

fn run_status(config: &Config) -> Result<(), String> {
    let store =
        DurableStore::open(&config.db_path, config.network).map_err(|error| error.to_string())?;
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
    Ok(())
}

fn run_doctor(config: &Config) -> Result<(), String> {
    let source = build_source(config)?;
    let status = source.node_status().map_err(|error| error.to_string())?;
    print_node_status(status);
    validate_node_for_sync(status, config.network, config.target_height, 0)?;
    println!("sync_ready=true");
    Ok(())
}

fn run_sync(config: &Config) -> Result<(), String> {
    let source = build_source(config)?;
    let status = source.node_status().map_err(|error| error.to_string())?;

    let store =
        DurableStore::open(&config.db_path, config.network).map_err(|error| error.to_string())?;
    let before = store.tip().map_err(|error| error.to_string())?;
    let next_needed = next_needed_height(before);

    validate_node_for_sync(status, config.network, config.target_height, next_needed)?;

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
    Ok(())
}

fn run_backfill(config: &Config) -> Result<(), String> {
    let source = build_source(config)?;
    let store =
        DurableStore::open(&config.db_path, config.network).map_err(|error| error.to_string())?;

    loop {
        let status = source.node_status().map_err(|error| error.to_string())?;
        validate_manual_backfill_node(status, config.network)?;

        let before = store.tip().map_err(|error| error.to_string())?;
        if backfill_target_reached(before.map(|tip| tip.height), config.target_height)? {
            let limit = config.target_height.expect("target is set when reached");
            println!("backfill_target_reached=true tip_height={limit}");
            return Ok(());
        }

        if let Some(limit) = config.target_height {
            if !status.initial_block_download && status.blocks < limit {
                return Err(format!(
                    "Bitcoin Core finished IBD at height {}, below requested backfill target {limit}",
                    status.blocks
                ));
            }
        }

        let next_needed = next_needed_height(before);
        validate_history_available(status, next_needed)?;

        let available_target = config.target_height.map_or(status.blocks, |limit| limit.min(status.blocks));

        if next_needed <= available_target {
            let target = next_needed
                .saturating_add(config.batch_blocks - 1)
                .min(available_target);
            let stats = store
                .sync_to_height(&source, target)
                .map_err(|error| error.to_string())?;
            let after = store.tip().map_err(|error| error.to_string())?;

            let prune_result =
                maybe_prune_after_commit(&source, status, after, config.prune_lag_blocks)?;

            println!(
                "batch_target={} connected={} disconnected={} before={} after={} prune={}",
                target,
                stats.connected,
                stats.disconnected,
                format_tip(before),
                format_tip(after),
                prune_result.map_or_else(|| "none".to_owned(), |height| height.to_string())
            );

            if config
                .target_height
                .is_some_and(|limit| after.is_some_and(|tip| tip.height == limit))
            {
                println!("backfill_target_reached=true tip_height={target}");
                return Ok(());
            }
        }

        let refreshed = source.node_status().map_err(|error| error.to_string())?;
        validate_manual_backfill_node(refreshed, config.network)?;
        let tip = store.tip().map_err(|error| error.to_string())?;

        if !refreshed.initial_block_download
            && tip.is_some_and(|tip| tip.height == refreshed.blocks)
        {
            println!(
                "backfill_complete=true tip_height={} core_size_on_disk={}",
                refreshed.blocks, refreshed.size_on_disk
            );
            return Ok(());
        }

        if tip.map_or(0, |tip| tip.height) >= refreshed.blocks {
            thread::sleep(Duration::from_secs(config.poll_seconds));
        }
    }
}

fn backfill_target_reached(
    durable_height: Option<u32>,
    target_height: Option<u32>,
) -> Result<bool, String> {
    let (Some(durable), Some(target)) = (durable_height, target_height) else {
        return Ok(false);
    };

    if durable > target {
        return Err(format!(
            "backfill target {target} is below durable Researcher tip {durable}"
        ));
    }

    Ok(durable == target)
}

fn maybe_prune_after_commit(
    source: &BitcoinCoreRpcSource,
    status: NodeStatus,
    committed_tip: Option<DurableTip>,
    prune_lag_blocks: u32,
) -> Result<Option<u32>, String> {
    let Some(tip) = committed_tip else {
        return Ok(None);
    };
    let Some(target) = planned_prune_height(tip.height, prune_lag_blocks, status.prune_height)
    else {
        return Ok(None);
    };

    source
        .prune_blockchain(target)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn planned_prune_height(
    committed_height: u32,
    prune_lag_blocks: u32,
    current_prune_height: Option<u32>,
) -> Option<u32> {
    let target = committed_height.checked_sub(prune_lag_blocks)?;
    let first_available = current_prune_height.unwrap_or(0);
    (target >= first_available && target > 0).then_some(target)
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
            "RPC access requires either --cookie-file or the pair --rpc-user/--rpc-password"
                .to_owned(),
        ),
    }
}

fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut args = args.into_iter();
    let command = match args.next().as_deref() {
        Some("status") => Command::Status,
        Some("doctor") => Command::Doctor,
        Some("sync") => Command::Sync,
        Some("backfill") => Command::Backfill,
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
    let mut prune_lag_blocks = DEFAULT_PRUNE_LAG_BLOCKS;
    let mut batch_blocks = DEFAULT_BATCH_BLOCKS;
    let mut poll_seconds = DEFAULT_POLL_SECONDS;

    while let Some(flag) = args.next() {
        let mut value = || {
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
            "--prune-lag-blocks" => {
                let raw = value()?;
                prune_lag_blocks = raw
                    .parse::<u32>()
                    .map_err(|_| format!("invalid prune lag {raw:?}"))?;
            }
            "--batch-blocks" => {
                let raw = value()?;
                batch_blocks = raw
                    .parse::<u32>()
                    .map_err(|_| format!("invalid batch size {raw:?}"))?;
            }
            "--poll-seconds" => {
                let raw = value()?;
                poll_seconds = raw
                    .parse::<u64>()
                    .map_err(|_| format!("invalid poll interval {raw:?}"))?;
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
        prune_lag_blocks,
        batch_blocks,
        poll_seconds,
    };

    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), String> {
    let backfill_options_changed = config.prune_lag_blocks != DEFAULT_PRUNE_LAG_BLOCKS
        || config.batch_blocks != DEFAULT_BATCH_BLOCKS
        || config.poll_seconds != DEFAULT_POLL_SECONDS;

    if config.command == Command::Status {
        if config.cookie_file.is_some()
            || config.rpc_user.is_some()
            || config.rpc_password.is_some()
            || config.target_height.is_some()
            || backfill_options_changed
        {
            return Err("RPC/sync/backfill options are not valid for status".to_owned());
        }
        return Ok(());
    }

    if config.command == Command::Backfill {
        if config.prune_lag_blocks < MIN_PRUNE_LAG_BLOCKS {
            return Err(format!(
                "--prune-lag-blocks must be at least {MIN_PRUNE_LAG_BLOCKS}"
            ));
        }
        if config.batch_blocks == 0 {
            return Err("--batch-blocks must be greater than zero".to_owned());
        }
        if config.poll_seconds == 0 {
            return Err("--poll-seconds must be greater than zero".to_owned());
        }
    } else if backfill_options_changed {
        return Err(
            "--prune-lag-blocks, --batch-blocks and --poll-seconds are only valid for backfill"
                .to_owned(),
        );
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
            "RPC access requires either --cookie-file or the pair --rpc-user/--rpc-password"
                .to_owned(),
        );
    }

    Ok(())
}

fn validate_node_for_sync(
    status: NodeStatus,
    expected_network: Network,
    target_height: Option<u32>,
    next_needed: u32,
) -> Result<(), String> {
    validate_network(status, expected_network)?;
    validate_history_available(status, next_needed)?;

    match target_height {
        Some(target) => {
            if status.blocks < target {
                return Err(format!(
                    "Bitcoin Core has only validated through block {}, but target height {} was requested",
                    status.blocks, target
                ));
            }
        }
        None if status.initial_block_download => {
            return Err(format!(
                "Bitcoin Core is still in initial block download (blocks={}, headers={}); an unbounded sync requires IBD to finish",
                status.blocks, status.headers
            ));
        }
        None => {}
    }

    Ok(())
}

fn validate_manual_backfill_node(
    status: NodeStatus,
    expected_network: Network,
) -> Result<(), String> {
    validate_network(status, expected_network)?;

    if !status.pruned {
        return Err(
            "backfill requires Bitcoin Core manual pruning mode (start Core with prune=1)"
                .to_owned(),
        );
    }

    match status.automatic_pruning {
        Some(false) => Ok(()),
        Some(true) => Err(
            "backfill refuses automatic pruning because Core could delete blocks before Researcher commits them; restart Core with prune=1"
                .to_owned(),
        ),
        None => Err(
            "Bitcoin Core did not report pruning mode details; refusing destructive pruning control"
                .to_owned(),
        ),
    }
}

fn validate_network(status: NodeStatus, expected_network: Network) -> Result<(), String> {
    if status.network != expected_network {
        return Err(format!(
            "Bitcoin Core network mismatch: expected {}, node reports {}",
            network_name(expected_network),
            network_name(status.network)
        ));
    }
    Ok(())
}

fn validate_history_available(status: NodeStatus, next_needed: u32) -> Result<(), String> {
    if !status.pruned {
        return Ok(());
    }

    let Some(first_available) = status.prune_height else {
        return Err(
            "Bitcoin Core reports pruning enabled but no prune height; refusing to assume required history is available"
                .to_owned(),
        );
    };

    if next_needed < first_available {
        return Err(format!(
            "required block {next_needed} is no longer available; Bitcoin Core's first retained block is {first_available}"
        ));
    }

    Ok(())
}

fn next_needed_height(tip: Option<DurableTip>) -> u32 {
    tip.map_or(0, |tip| tip.height.saturating_add(1))
}

fn print_node_status(status: NodeStatus) {
    println!(
        "node_network={} blocks={} headers={} initial_block_download={} size_on_disk={} pruned={} prune_height={} automatic_pruning={} prune_target_size={}",
        network_name(status.network),
        status.blocks,
        status.headers,
        status.initial_block_download,
        status.size_on_disk,
        status.pruned,
        status
            .prune_height
            .map_or_else(|| "none".to_owned(), |height| height.to_string()),
        status
            .automatic_pruning
            .map_or_else(|| "none".to_owned(), |enabled| enabled.to_string()),
        status
            .prune_target_size
            .map_or_else(|| "none".to_owned(), |bytes| bytes.to_string())
    );
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

fn format_tip(tip: Option<DurableTip>) -> String {
    match tip {
        Some(tip) => format!("{}:{}", tip.height, tip.hash),
        None => "empty".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_status() -> NodeStatus {
        NodeStatus {
            network: Network::Bitcoin,
            blocks: 900_000,
            headers: 950_000,
            initial_block_download: true,
            size_on_disk: 100_000_000,
            pruned: true,
            prune_height: Some(0),
            automatic_pruning: Some(false),
            prune_target_size: None,
        }
    }

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
    fn parses_manual_prune_backfill() {
        let config = parse_args(vec![
            "backfill".to_owned(),
            "--cookie-file".to_owned(),
            "/tmp/cookie".to_owned(),
            "--prune-lag-blocks".to_owned(),
            "20000".to_owned(),
            "--batch-blocks".to_owned(),
            "2500".to_owned(),
            "--poll-seconds".to_owned(),
            "2".to_owned(),
            "--target-height".to_owned(),
            "11000".to_owned(),
        ])
        .unwrap();

        assert_eq!(config.command, Command::Backfill);
        assert_eq!(config.prune_lag_blocks, 20_000);
        assert_eq!(config.batch_blocks, 2_500);
        assert_eq!(config.poll_seconds, 2);
        assert_eq!(config.target_height, Some(11_000));
    }

    #[test]
    fn backfill_target_guard_is_monotonic() {
        assert_eq!(backfill_target_reached(Some(5_000), Some(5_000)), Ok(true));
        assert_eq!(backfill_target_reached(Some(4_999), Some(5_000)), Ok(false));
        assert_eq!(backfill_target_reached(Some(5_000), None), Ok(false));

        let error = backfill_target_reached(Some(5_001), Some(5_000)).unwrap_err();
        assert!(error.contains("below durable Researcher tip 5001"));
    }

    #[test]
    fn backfill_has_hard_safety_bounds() {
        let error = parse_args(vec![
            "backfill".to_owned(),
            "--cookie-file".to_owned(),
            "/tmp/cookie".to_owned(),
            "--prune-lag-blocks".to_owned(),
            "999".to_owned(),
        ])
        .unwrap_err();

        assert!(error.contains("at least 1000"));
    }

    #[test]
    fn manual_backfill_rejects_automatic_pruning() {
        let automatic = NodeStatus {
            automatic_pruning: Some(true),
            ..node_status()
        };
        assert!(validate_manual_backfill_node(automatic, Network::Bitcoin)
            .unwrap_err()
            .contains("automatic pruning"));

        assert!(validate_manual_backfill_node(node_status(), Network::Bitcoin).is_ok());
    }

    #[test]
    fn history_check_allows_resume_after_older_blocks_were_pruned() {
        let status = NodeStatus {
            prune_height: Some(100_000),
            ..node_status()
        };

        assert!(validate_history_available(status, 100_000).is_ok());
        assert!(validate_history_available(status, 100_001).is_ok());
        assert!(validate_history_available(status, 99_999)
            .unwrap_err()
            .contains("no longer available"));
    }

    #[test]
    fn prune_plan_never_touches_uncommitted_or_recent_blocks() {
        assert_eq!(planned_prune_height(9_999, 10_000, Some(0)), None);
        assert_eq!(planned_prune_height(20_000, 10_000, Some(0)), Some(10_000));
        assert_eq!(planned_prune_height(20_000, 10_000, Some(10_001)), None);
    }

    #[test]
    fn sync_allows_bounded_ibd_when_requested_height_is_available() {
        let ready = NodeStatus {
            pruned: false,
            prune_height: None,
            automatic_pruning: None,
            ..node_status()
        };

        assert!(validate_node_for_sync(ready, Network::Bitcoin, Some(1_000), 0).is_ok());

        let too_early = NodeStatus {
            blocks: 999,
            ..ready
        };
        assert!(
            validate_node_for_sync(too_early, Network::Bitcoin, Some(1_000), 0)
                .unwrap_err()
                .contains("only validated through block 999")
        );
    }

    #[test]
    fn status_rejects_rpc_options() {
        let error = parse_args(vec![
            "status".to_owned(),
            "--cookie-file".to_owned(),
            "/tmp/cookie".to_owned(),
        ])
        .unwrap_err();

        assert!(error.contains("not valid for status"));
    }
}
