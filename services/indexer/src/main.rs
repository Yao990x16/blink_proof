use std::{
    collections::HashMap,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anchor_lang::{
    prelude::{borsh, Pubkey},
    AnchorDeserialize,
};
use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Query, State},
    http::{header::CONTENT_TYPE, Method, StatusCode},
    response::Html,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use solana_client::{
    pubsub_client::PubsubClient,
    rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter},
};
use solana_sdk::commitment_config::CommitmentConfig;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use tower_http::cors::{Any, CorsLayer};

const BLINK_PROOF_PROGRAM_ID: &str = "Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ";
const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_WEBSOCKET_URL: &str = "ws://127.0.0.1:8900";
const DEFAULT_HTTP_BIND_ADDR: &str = "0.0.0.0:3001";
const DEFAULT_DATABASE_PATH_FROM_WORKSPACE: &str = "services/indexer/hashes.db";
const DEFAULT_DATABASE_PATH_FROM_SERVICE: &str = "hashes.db";
const CONTENT_REGISTERED_DISCRIMINATOR: [u8; 8] = [234, 59, 220, 137, 21, 89, 2, 148];
const RATE_LIMIT_REQUESTS: u32 = 30;
const RATE_LIMIT_WINDOW_SECS: u64 = 60;

#[derive(Debug, AnchorDeserialize)]
struct ContentRegistered {
    creator: Pubkey,
    salted_fingerprint: [u8; 32],
    raw_phash: [u8; 8],
    timestamp: i64,
}

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    rate_limiter: Arc<RateLimiter>,
}

struct RateLimiter {
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, key: &str) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        let now = Instant::now();
        let entry = buckets.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0).as_secs() >= RATE_LIMIT_WINDOW_SECS {
            *entry = (now, 1);
            return true;
        }
        if entry.1 >= RATE_LIMIT_REQUESTS {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[derive(Serialize)]
struct StatsResponse {
    total_fingerprints: i64,
    unique_creators_count: i64,
    latest_registration: Option<LatestRegistration>,
    top_issuers: Vec<IssuerReputation>,
}

#[derive(Serialize)]
struct LatestRegistration {
    timestamp: String,
    hash_prefix: String,
}

#[derive(Serialize)]
struct IssuerReputation {
    issuer: String,
    total_attestations: i64,
    first_seen: String,
    last_seen: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    database: bool,
}

/// Response for GET /lookup?fingerprint=<hex-encoded salted_fingerprint>
#[derive(Serialize)]
struct LookupResponse {
    found: bool,
    /// Approximate leaf index (rowid - 1). Accurate only if no rows were deleted.
    leaf_index: Option<i64>,
    creator: Option<String>,
    timestamp: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::from_path("services/indexer/.env").ok();
    let websocket_url = env::var("SOLANA_WEBSOCKET_URL").unwrap_or_else(|_| DEFAULT_WEBSOCKET_URL.to_string());

    let database_path = resolve_database_path();
    let pool = init_database(&database_path).await?;
    spawn_http_server(pool.clone(), Arc::new(RateLimiter::new()));

    println!("SQLite 本地库：{}", database_path.display());
    if let Err(error) = backfill_historical_events(&pool).await {
        eprintln!("历史事件回填失败（非致命）：{error:#}");
    }

    let mut retry_delay = Duration::from_secs(1);
    let max_retry_delay = Duration::from_secs(60);

    loop {
        println!("连接 Solana 日志流：{websocket_url}");

        match run_subscription_loop(&pool, &websocket_url).await {
            Ok(()) => {
                println!("日志订阅正常结束，准备重连...");
                retry_delay = Duration::from_secs(1);
            }
            Err(error) => {
                eprintln!("日志订阅中断：{error:#}，{retry_delay:?} 后重连...");
            }
        }

        tokio::time::sleep(retry_delay).await;
        retry_delay = (retry_delay * 2).min(max_retry_delay);
    }
}

async fn run_subscription_loop(pool: &SqlitePool, websocket_url: &str) -> Result<()> {
    let filter = RpcTransactionLogsFilter::Mentions(vec![BLINK_PROOF_PROGRAM_ID.to_string()]);
    let config = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::confirmed()),
    };

    let (_subscription, receiver) =
        PubsubClient::logs_subscribe(websocket_url, filter, config)
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
            upsert_registered_content(pool, &event).await?;
            println!(
                "监听到新存证：{} 已同步至本地库",
                short_hex(&event.salted_fingerprint)
            );
        }
    }
}

async fn backfill_historical_events(pool: &SqlitePool) -> Result<()> {
    use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
    use solana_sdk::{pubkey::Pubkey, signature::Signature};
    use std::str::FromStr;

    println!("检查是否需要历史事件回填...");
    let rpc_url = env::var("SOLANA_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.to_string());

    // Phase 1 — blocking RPC calls: collect all missed events without .await.
    // spawn_blocking runs on a dedicated thread-pool, so long RPC calls won't
    // starve the async runtime.
    let rpc_url_clone = rpc_url.clone();
    let events: Vec<ContentRegistered> =
        tokio::task::spawn_blocking(move || -> Vec<ContentRegistered> {
            let rpc = solana_client::rpc_client::RpcClient::new_with_commitment(
                rpc_url_clone,
                CommitmentConfig::confirmed(),
            );

            let program_id = match Pubkey::from_str(BLINK_PROOF_PROGRAM_ID) {
                Ok(pk) => pk,
                Err(err) => {
                    eprintln!("回填跳过：无法解析 program ID：{err}");
                    return vec![];
                }
            };

            // Pull up to 1000 recent signatures.
            let config = GetConfirmedSignaturesForAddress2Config {
                limit: Some(1000),
                commitment: Some(CommitmentConfig::confirmed()),
                ..Default::default()
            };

            let signatures =
                match rpc.get_signatures_for_address_with_config(&program_id, config) {
                    Ok(sigs) => sigs,
                    Err(err) => {
                        // Local Surfpool may not support this method; degrade gracefully.
                        println!("历史回填跳过（RPC 不支持 getSignaturesForAddress）：{err}");
                        return vec![];
                    }
                };

            println!("获取到 {} 条历史签名，开始解析...", signatures.len());
            let mut collected: Vec<ContentRegistered> = Vec::new();

            // Process from oldest to newest so DB write order is chronological.
            for sig_info in signatures.iter().rev() {
                if sig_info.err.is_some() {
                    continue;
                }
                let sig = match Signature::from_str(&sig_info.signature) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let tx = match rpc.get_transaction_with_config(
                    &sig,
                    solana_client::rpc_config::RpcTransactionConfig {
                        encoding: Some(
                            solana_transaction_status::UiTransactionEncoding::Base64,
                        ),
                        commitment: Some(CommitmentConfig::confirmed()),
                        max_supported_transaction_version: Some(0),
                    },
                ) {
                    Ok(tx) => tx,
                    Err(_) => continue, // Pruned or unavailable; skip.
                };

                let logs: Vec<String> = tx
                    .transaction
                    .meta
                    .and_then(|m| m.log_messages.into())
                    .unwrap_or_default();

                collected.extend(parse_content_registered_events(&logs));
            }
            collected
        })
        .await
        .unwrap_or_default();

    // Phase 2 — async DB writes: run in the outer async context where .await is allowed.
    let mut backfilled = 0usize;
    for event in &events {
        // Skip fingerprints already in the DB to avoid double-counting on restart.
        let already_indexed: bool = sqlx::query(
            "SELECT 1 FROM registered_content WHERE salted_fingerprint = ?1 LIMIT 1",
        )
        .bind(event.salted_fingerprint.to_vec())
        .fetch_optional(pool)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false);

        if !already_indexed {
            if let Err(err) = upsert_registered_content(pool, event).await {
                eprintln!("回填写入失败（跳过）：{err:#}");
            } else {
                backfilled += 1;
                println!(
                    "回填存证：{} 已写入本地库",
                    short_hex(&event.salted_fingerprint)
                );
            }
        }
    }

    println!("历史事件回填完成，共补录 {backfilled} 条存证记录。");
    Ok(())
}


fn spawn_http_server(pool: SqlitePool, rate_limiter: Arc<RateLimiter>) {
    tokio::spawn(async move {
        if let Err(error) = run_http_server(pool, rate_limiter).await {
            eprintln!("Indexer HTTP server exited: {error:#}");
        }
    });
}

async fn run_http_server(pool: SqlitePool, rate_limiter: Arc<RateLimiter>) -> Result<()> {
    let bind_addr = env::var("BLINK_INDEXER_HTTP_ADDR")
        .unwrap_or_else(|_| DEFAULT_HTTP_BIND_ADDR.to_string())
        .parse::<SocketAddr>()
        .context("BLINK_INDEXER_HTTP_ADDR must be a valid socket address")?;
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE]);

    let app = Router::new()
        .route("/", get(index_dashboard))
        .route("/health", get(health_check))
        .route("/stats", get(get_stats))
        .route("/lookup", get(lookup_fingerprint))
        .with_state(AppState { pool, rate_limiter })
        .layer(cors)
        // Enable ConnectInfo so handlers can read the client socket address.
        .into_make_service_with_connect_info::<SocketAddr>();

    println!("Indexer HTTP 看板：http://{bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .context("failed to bind indexer HTTP listener")?;
    axum::serve(listener, app)
        .await
        .context("indexer HTTP server failed")?;

    Ok(())
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
            raw_phash_int INTEGER NOT NULL DEFAULT 0,
            creator TEXT NOT NULL,
            timestamp DATETIME NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("failed to create registered_content table")?;

    sqlx::query(
        r#"
        ALTER TABLE registered_content
        ADD COLUMN raw_phash_int INTEGER NOT NULL DEFAULT 0
        "#,
    )
    .execute(&pool)
    .await
    .ok();

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_phash_int
        ON registered_content(raw_phash_int)
        "#,
    )
    .execute(&pool)
    .await
    .context("failed to create raw_phash_int index")?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS issuer_reputation (
            issuer TEXT PRIMARY KEY,
            total_attestations INTEGER NOT NULL DEFAULT 0,
            first_seen DATETIME NOT NULL,
            last_seen DATETIME NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .context("failed to create issuer_reputation table")?;

    backfill_raw_phash_int(&pool).await?;

    Ok(pool)
}

async fn backfill_raw_phash_int(pool: &SqlitePool) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT id, raw_phash
        FROM registered_content
        WHERE raw_phash_int = 0
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to load raw_phash values for backfill")?;

    for row in rows {
        let id = row.get::<i64, _>("id");
        let raw_phash = row
            .try_get::<Vec<u8>, _>("raw_phash")
            .context("failed to read raw_phash for backfill")?;
        let raw_phash: [u8; 8] = raw_phash
            .try_into()
            .map_err(|_| anyhow::anyhow!("raw_phash must be exactly 8 bytes"))?;

        sqlx::query(
            r#"
            UPDATE registered_content
            SET raw_phash_int = ?1
            WHERE id = ?2
            "#,
        )
        .bind(phash_bytes_to_i64(&raw_phash))
        .bind(id)
        .execute(pool)
        .await
        .context("failed to backfill raw_phash_int")?;
    }

    Ok(())
}

async fn get_stats(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> std::result::Result<Json<StatsResponse>, (StatusCode, String)> {
    if !state.rate_limiter.check(&addr.ip().to_string()) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded".to_string(),
        ));
    }
    load_stats(&state.pool)
        .await
        .map(Json)
        .map_err(internal_error)
}

// /health is intentionally left unrestricted so uptime monitors are never blocked.
async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let db_ok = sqlx::query("SELECT 1").fetch_one(&state.pool).await.is_ok();

    Json(HealthResponse {
        status: if db_ok { "ok" } else { "degraded" },
        database: db_ok,
    })
}

async fn index_dashboard(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> std::result::Result<Html<String>, (StatusCode, String)> {
    if !state.rate_limiter.check(&addr.ip().to_string()) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded".to_string(),
        ));
    }
    let stats = load_stats(&state.pool).await.map_err(internal_error)?;
    Ok(Html(render_dashboard(&stats)))
}

/// GET /lookup?fingerprint=<hex-encoded 32-byte salted_fingerprint>
///
/// Returns the indexed record for the given fingerprint, including an approximate
/// leaf_index that a future `verify_content` call would need. Rate-limited.
async fn lookup_fingerprint(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<LookupResponse> {
    if !state.rate_limiter.check(&addr.ip().to_string()) {
        return Json(LookupResponse {
            found: false,
            leaf_index: None,
            creator: None,
            timestamp: None,
        });
    }

    let hex = params.get("fingerprint").cloned().unwrap_or_default();
    // Accept 64-char hex (32 bytes). Return not-found for malformed input.
    let fingerprint: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();

    if fingerprint.len() != 32 {
        return Json(LookupResponse {
            found: false,
            leaf_index: None,
            creator: None,
            timestamp: None,
        });
    }

    let row = sqlx::query(
        r#"
        SELECT id, creator, timestamp
        FROM registered_content
        WHERE salted_fingerprint = ?1
        LIMIT 1
        "#,
    )
    .bind(fingerprint)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    match row {
        Some(row) => {
            let id = row.get::<i64, _>("id");
            Json(LookupResponse {
                found: true,
                // rowid is 1-based; leaf_index in the Merkle tree is 0-based.
                // This approximation is valid as long as no rows have been deleted.
                leaf_index: Some(id - 1),
                creator: Some(row.get("creator")),
                timestamp: Some(row.get("timestamp")),
            })
        }
        None => Json(LookupResponse {
            found: false,
            leaf_index: None,
            creator: None,
            timestamp: None,
        }),
    }
}


async fn load_stats(pool: &SqlitePool) -> Result<StatsResponse> {
    let total_fingerprints = sqlx::query("SELECT COUNT(*) AS count FROM registered_content")
        .fetch_one(pool)
        .await
        .context("failed to count registered fingerprints")?
        .get::<i64, _>("count");

    let unique_creators_count =
        sqlx::query("SELECT COUNT(DISTINCT creator) AS count FROM registered_content")
            .fetch_one(pool)
            .await
            .context("failed to count unique creators")?
            .get::<i64, _>("count");

    let latest_registration = sqlx::query(
        r#"
        SELECT salted_fingerprint, timestamp
        FROM registered_content
        ORDER BY timestamp DESC, id DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("failed to fetch latest registration")?
    .and_then(|row| {
        let fingerprint = row.try_get::<Vec<u8>, _>("salted_fingerprint").ok()?;
        let timestamp = row.try_get::<String, _>("timestamp").ok()?;
        Some(LatestRegistration {
            timestamp,
            hash_prefix: short_blob_hex(&fingerprint),
        })
    });

    let top_issuers = sqlx::query(
        r#"
        SELECT issuer, total_attestations, first_seen, last_seen
        FROM issuer_reputation
        ORDER BY total_attestations DESC
        LIMIT 10
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to fetch top issuers")?
    .into_iter()
    .map(|row| IssuerReputation {
        issuer: row.get("issuer"),
        total_attestations: row.get("total_attestations"),
        first_seen: row.get("first_seen"),
        last_seen: row.get("last_seen"),
    })
    .collect();

    Ok(StatsResponse {
        total_fingerprints,
        unique_creators_count,
        latest_registration,
        top_issuers,
    })
}

fn internal_error(error: anyhow::Error) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("failed to load indexer stats: {error:#}"),
    )
}

fn render_dashboard(stats: &StatsResponse) -> String {
    let latest_timestamp = stats
        .latest_registration
        .as_ref()
        .map(|latest| latest.timestamp.as_str())
        .unwrap_or("暂无存证");
    let latest_hash = stats
        .latest_registration
        .as_ref()
        .map(|latest| latest.hash_prefix.as_str())
        .unwrap_or("--");
    let issuer_rows = render_issuer_rows(&stats.top_issuers);

    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>BlinkProof Indexer</title>
  <style>
    :root {{
      --ink: #18150f;
      --paper: #f4efe2;
      --card: rgba(255, 252, 243, 0.78);
      --line: rgba(24, 21, 15, 0.18);
      --accent: #e85d2a;
      --mint: #2a8c76;
      --shadow: 0 24px 70px rgba(38, 29, 13, 0.16);
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      min-height: 100vh;
      color: var(--ink);
      font-family: Georgia, "Times New Roman", serif;
      background:
        radial-gradient(circle at 14% 16%, rgba(232, 93, 42, 0.24), transparent 32rem),
        radial-gradient(circle at 86% 8%, rgba(42, 140, 118, 0.22), transparent 28rem),
        linear-gradient(135deg, #fbf4df 0%, #e7dac1 100%);
      padding: 48px;
    }}
    .shell {{
      max-width: 1120px;
      margin: 0 auto;
    }}
    .eyebrow {{
      display: inline-flex;
      gap: 10px;
      align-items: center;
      border: 1px solid var(--line);
      border-radius: 999px;
      padding: 8px 14px;
      background: rgba(255, 255, 255, 0.36);
      font: 700 12px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace;
      letter-spacing: .16em;
      text-transform: uppercase;
    }}
    h1 {{
      max-width: 820px;
      margin: 28px 0 12px;
      font-size: clamp(48px, 7vw, 94px);
      line-height: .9;
      letter-spacing: -0.07em;
    }}
    .sub {{
      max-width: 660px;
      color: rgba(24, 21, 15, 0.66);
      font: 18px/1.7 ui-serif, Georgia, serif;
    }}
    .grid {{
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 18px;
      margin-top: 42px;
    }}
    .card {{
      min-height: 220px;
      padding: 26px;
      border: 1px solid var(--line);
      border-radius: 30px;
      background: var(--card);
      box-shadow: var(--shadow);
      backdrop-filter: blur(16px);
      position: relative;
      overflow: hidden;
    }}
    .card::after {{
      content: "";
      position: absolute;
      width: 150px;
      height: 150px;
      right: -52px;
      bottom: -60px;
      border-radius: 999px;
      background: rgba(232, 93, 42, 0.14);
    }}
    .label {{
      font: 800 12px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace;
      letter-spacing: .14em;
      text-transform: uppercase;
      color: rgba(24, 21, 15, 0.55);
    }}
    .value {{
      margin-top: 34px;
      font-size: clamp(42px, 5vw, 76px);
      line-height: .85;
      letter-spacing: -0.06em;
    }}
    .note {{
      margin-top: 18px;
      color: rgba(24, 21, 15, 0.58);
      font: 15px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
      word-break: break-all;
    }}
    .wide {{
      grid-column: span 3;
      min-height: 160px;
      display: grid;
      grid-template-columns: 1.1fr 1fr;
      gap: 24px;
      align-items: end;
    }}
    .hash {{
      color: var(--mint);
      font-size: clamp(36px, 5vw, 68px);
    }}
    @media (max-width: 840px) {{
      body {{ padding: 26px; }}
      .grid {{ grid-template-columns: 1fr; }}
      .wide {{ grid-column: span 1; grid-template-columns: 1fr; }}
    }}
  </style>
</head>
<body>
  <main class="shell">
    <span class="eyebrow">BlinkProof Indexer</span>
    <h1>Content provenance signal board.</h1>
    <p class="sub">实时读取本地 SQLite 索引库，展示 Solana 上 BlinkProof 内容存证事件的聚合状态。</p>
    <section class="grid">
      <article class="card">
        <div class="label">Total fingerprints</div>
        <div class="value">{}</div>
        <div class="note">链上已索引的加盐指纹总数</div>
      </article>
      <article class="card">
        <div class="label">Unique creators</div>
        <div class="value">{}</div>
        <div class="note">去重后的内容创作者地址数量</div>
      </article>
      <article class="card">
        <div class="label">Latest time</div>
        <div class="value" style="font-size: clamp(22px, 3vw, 38px); line-height: 1.04;">{}</div>
        <div class="note">最近一笔存证时间</div>
      </article>
      <article class="card wide">
        <div>
          <div class="label">Latest fingerprint</div>
          <div class="hash">{}</div>
        </div>
        <div class="note">接口：<strong>/stats</strong> 返回同源 JSON 数据，供监控面板或调试脚本调用。</div>
      </article>
      <article class="card wide">
        <div>
          <div class="label">Top issuers</div>
          <table style="width:100%;margin-top:16px;border-collapse:collapse;font:14px/1.6 ui-monospace,monospace;">
            <tr style="color:rgba(24,21,15,0.55);text-align:left;">
              <th>Issuer</th><th>Attestations</th><th>Last seen</th>
            </tr>
            {}
          </table>
        </div>
      </article>
    </section>
  </main>
</body>
</html>"#,
        stats.total_fingerprints,
        stats.unique_creators_count,
        escape_html(latest_timestamp),
        escape_html(latest_hash),
        issuer_rows,
    )
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
            raw_phash_int,
            creator,
            timestamp
        )
        VALUES (?1, ?2, ?3, ?4, datetime(?5, 'unixepoch'))
        "#,
    )
    .bind(event.salted_fingerprint.to_vec())
    .bind(event.raw_phash.to_vec())
    .bind(phash_bytes_to_i64(&event.raw_phash))
    .bind(event.creator.to_string())
    .bind(event.timestamp)
    .execute(pool)
    .await
    .context("failed to upsert registered content event")?;

    sqlx::query(
        r#"
        INSERT INTO issuer_reputation (issuer, total_attestations, first_seen, last_seen)
        VALUES (?1, 1, datetime(?2, 'unixepoch'), datetime(?2, 'unixepoch'))
        ON CONFLICT(issuer) DO UPDATE SET
            total_attestations = total_attestations + 1,
            last_seen = datetime(?2, 'unixepoch')
        "#,
    )
    .bind(event.creator.to_string())
    .bind(event.timestamp)
    .execute(pool)
    .await
    .context("failed to upsert issuer reputation")?;

    Ok(())
}

fn phash_bytes_to_i64(bytes: &[u8; 8]) -> i64 {
    i64::from_be_bytes(*bytes)
}

fn short_hex(bytes: &[u8; 32]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn short_blob_hex(bytes: &[u8]) -> String {
    let prefix = bytes
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("");

    if prefix.is_empty() {
        "--".to_string()
    } else {
        prefix
    }
}

fn render_issuer_rows(issuers: &[IssuerReputation]) -> String {
    if issuers.is_empty() {
        return r#"<tr><td colspan="3" style="padding-top:12px;color:rgba(24,21,15,0.55);">暂无发行人数据</td></tr>"#
            .to_string();
    }

    issuers
        .iter()
        .map(|issuer| {
            format!(
                r#"<tr><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                escape_html(&short_address(&issuer.issuer)),
                issuer.total_attestations,
                escape_html(&issuer.last_seen),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn short_address(address: &str) -> String {
    if address.len() <= 10 {
        return address.to_string();
    }

    format!("{}...{}", &address[..4], &address[address.len() - 4..])
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
