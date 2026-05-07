use std::{
    env,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anchor_lang::{
    prelude::{borsh, Pubkey},
    AnchorDeserialize,
};
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use solana_client::{
    pubsub_client::PubsubClient,
    rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter},
};
use solana_sdk::commitment_config::CommitmentConfig;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

const BLINK_PROOF_PROGRAM_ID: &str = "Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ";
const DEFAULT_WEBSOCKET_URL: &str = "ws://127.0.0.1:8900";
const DEFAULT_DATABASE_PATH_FROM_WORKSPACE: &str = "services/indexer/hashes.db";
const DEFAULT_DATABASE_PATH_FROM_SERVICE: &str = "hashes.db";
const CONTENT_REGISTERED_DISCRIMINATOR: [u8; 8] = [234, 59, 220, 137, 21, 89, 2, 148];

#[derive(Debug, AnchorDeserialize)]
struct ContentRegistered {
    creator: Pubkey,
    salted_fingerprint: [u8; 32],
    raw_phash: [u8; 8],
    timestamp: i64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let database_path = resolve_database_path();
    let pool = init_database(&database_path).await?;
    let filter = RpcTransactionLogsFilter::Mentions(vec![BLINK_PROOF_PROGRAM_ID.to_string()]);
    let config = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::confirmed()),
    };

    println!("连接 Solana 日志流：{DEFAULT_WEBSOCKET_URL}");
    println!("SQLite 本地库：{}", database_path.display());
    let (_subscription, receiver) =
        PubsubClient::logs_subscribe(DEFAULT_WEBSOCKET_URL, filter, config)
            .context("failed to subscribe to blink_proof logs")?;
    println!("开始监听 blink_proof 存证事件：{BLINK_PROOF_PROGRAM_ID}");

    loop {
        let notification = receiver
            .recv()
            .context("Solana logs subscription channel closed")?;

        if let Some(error) = notification.value.err {
            println!(
                "跳过失败交易 {}，错误：{error:?}",
                notification.value.signature
            );
            continue;
        }

        for event in parse_content_registered_events(&notification.value.logs) {
            upsert_registered_content(&pool, &event).await?;
            println!(
                "监听到新存证：{} 已同步至本地库",
                short_hex(&event.salted_fingerprint)
            );
        }
    }
}

async fn init_database(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create SQLite database directory")?;
    }

    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .context("failed to open SQLite database")?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS registered_content (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            salted_fingerprint BLOB NOT NULL UNIQUE,
            raw_phash BLOB NOT NULL,
            creator TEXT NOT NULL,
            timestamp DATETIME NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("failed to create registered_content table")?;

    Ok(pool)
}

fn resolve_database_path() -> PathBuf {
    if let Ok(path) = env::var("BLINK_INDEXER_DB_PATH") {
        return PathBuf::from(path);
    }

    if Path::new("services/indexer").exists() {
        return PathBuf::from(DEFAULT_DATABASE_PATH_FROM_WORKSPACE);
    }

    PathBuf::from(DEFAULT_DATABASE_PATH_FROM_SERVICE)
}

fn parse_content_registered_events(logs: &[String]) -> Vec<ContentRegistered> {
    logs.iter()
        .filter_map(|log| log.strip_prefix("Program data: "))
        .filter_map(|encoded| decode_content_registered_event(encoded).ok())
        .collect()
}

fn decode_content_registered_event(encoded: &str) -> Result<ContentRegistered> {
    let decoded = BASE64
        .decode(encoded)
        .context("failed to base64-decode Anchor event log")?;
    let (discriminator, mut payload) = decoded
        .split_first_chunk::<8>()
        .context("Anchor event log is shorter than its discriminator")?;

    if discriminator != &CONTENT_REGISTERED_DISCRIMINATOR {
        anyhow::bail!("Anchor event discriminator does not match ContentRegistered");
    }

    ContentRegistered::deserialize(&mut payload)
        .context("failed to deserialize ContentRegistered event payload")
}

async fn upsert_registered_content(pool: &SqlitePool, event: &ContentRegistered) -> Result<()> {
    sqlx::query(
        r#"
        INSERT OR REPLACE INTO registered_content (
            salted_fingerprint,
            raw_phash,
            creator,
            timestamp
        )
        VALUES (?1, ?2, ?3, datetime(?4, 'unixepoch'))
        "#,
    )
    .bind(event.salted_fingerprint.to_vec())
    .bind(event.raw_phash.to_vec())
    .bind(event.creator.to_string())
    .bind(event.timestamp)
    .execute(pool)
    .await
    .context("failed to upsert registered content event")?;

    Ok(())
}

fn short_hex(bytes: &[u8; 32]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}
