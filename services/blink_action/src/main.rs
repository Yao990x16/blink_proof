mod action_types;
mod blinkproof;
mod phash;

use dotenv::dotenv;

use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    extract::rejection::JsonRejection,
    extract::DefaultBodyLimit,
    extract::{Query, State},
    http::{
        header::{ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE},
        HeaderMap, HeaderName, Method, StatusCode,
    },
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{error, info, warn};

use crate::{
    action_types::{
        ActionGetResponse, ActionPostQuery, ActionPostRequest, ActionPostResponse, ActionRule,
        ActionsJsonResponse, ApiError,
    },
    blinkproof::{build_register_content_transaction, BlinkProofConfig},
    phash::calculate_phash,
};

const BLINK_PROOF_PROGRAM_ID: &str = "Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ";
const SPL_ACCOUNT_COMPRESSION_PROGRAM_ID: &str = "cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK";
const SPL_NOOP_PROGRAM_ID: &str = "noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV";
const PLACEHOLDER_ICON_URL: &str = "https://blinkproof.xyz/logo.png";
const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:3000";
const DEFAULT_MERKLE_TREE: &str = "11111111111111111111111111111111";
const DEFAULT_INDEXER_DB_PATH_FROM_WORKSPACE: &str = "services/indexer/hashes.db";
const DEFAULT_INDEXER_DB_PATH_FROM_SERVICE: &str = "../indexer/hashes.db";
const PHASH_BIT_LENGTH: u32 = 64;
const SIMILARITY_THRESHOLD: u32 = 5;
const MAX_IMAGE_URL_LENGTH: usize = 10 * 1024 * 1024;
const RATE_LIMIT_REQUESTS: u32 = 10;
const RATE_LIMIT_WINDOW_SECS: u64 = 60;

#[derive(Clone)]
struct AppState {
    rpc_client: Arc<RpcClient>,
    blinkproof: BlinkProofConfig,
    index_pool: Option<SqlitePool>,
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

struct IndexedContent {
    creator: String,
    timestamp: String,
    total_attestations: i64,
}

struct SimilarContent {
    creator: String,
    timestamp: String,
    distance: u32,
    total_attestations: i64,
}

enum VerificationMatch {
    Exact(IndexedContent),
    Similar(SimilarContent),
}

#[tokio::main]
async fn main() {
    // 强制从服务目录下加载 .env，防止在 workspace 根目录运行找不到文件
    dotenv::from_path("services/blink_action/.env").ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            env::var("RUST_LOG").unwrap_or_else(|_| "blink_action=info,tower_http=info".into()),
        )
        .init();

    let state = match build_state_from_env().await {
        Ok(state) => state,
        Err(error) => {
            eprintln!("failed to initialize Blink Action service: {error}");
            std::process::exit(1);
        }
    };

    let app = Router::new()
        .route(
            "/api/actions/verify",
            get(get_verify_action)
                .post(post_verify_action)
                .options(options_verify_action),
        )
        .route(
            "/actions.json",
            get(get_actions_json).options(options_verify_action),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_IMAGE_URL_LENGTH + 1024))
        .layer(TraceLayer::new_for_http())
        .layer(build_cors_layer());

    let bind_addr = env::var("BLINK_ACTION_BIND_ADDR")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
        .parse::<SocketAddr>()
        .expect("BLINK_ACTION_BIND_ADDR must be a valid socket address");

    info!("BlinkProof Action backend listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .expect("failed to bind Blink Action listener");

    axum::serve(listener, app)
        .await
        .expect("Blink Action server exited unexpectedly");
}

async fn build_state_from_env() -> Result<AppState, String> {
    let rpc_url = env::var("SOLANA_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.to_string());
    let merkle_tree = env::var("BLINK_MERKLE_TREE").unwrap_or_else(|_| DEFAULT_MERKLE_TREE.into());
    let index_pool = init_index_pool().await;

    Ok(AppState {
        rpc_client: Arc::new(RpcClient::new(rpc_url)),
        blinkproof: BlinkProofConfig {
            program_id: parse_pubkey("BLINK_PROOF_PROGRAM_ID", BLINK_PROOF_PROGRAM_ID)?,
            compression_program_id: parse_pubkey(
                "SPL_ACCOUNT_COMPRESSION_PROGRAM_ID",
                SPL_ACCOUNT_COMPRESSION_PROGRAM_ID,
            )?,
            noop_program_id: parse_pubkey("SPL_NOOP_PROGRAM_ID", SPL_NOOP_PROGRAM_ID)?,
            merkle_tree: parse_pubkey("BLINK_MERKLE_TREE", &merkle_tree)?,
        },
        index_pool,
        rate_limiter: Arc::new(RateLimiter::new()),
    })
}

fn parse_pubkey(env_name: &str, value: &str) -> Result<Pubkey, String> {
    Pubkey::from_str(value)
        .map_err(|error| format!("{env_name} must be a valid Solana pubkey: {error}"))
}

fn build_cors_layer() -> CorsLayer {
    let x_blockchain_ids = HeaderName::from_static("x-blockchain-ids");

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
        .allow_headers([
            CONTENT_TYPE,
            AUTHORIZATION,
            ACCEPT_ENCODING,
            CONTENT_ENCODING,
            x_blockchain_ids,
        ])
}

async fn options_verify_action() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn get_verify_action() -> Json<ActionGetResponse> {
    Json(ActionGetResponse {
        action_type: "action",
        icon: PLACEHOLDER_ICON_URL,
        title: "BlinkProof: 内容指纹核验",
        description: "一键核验该媒体内容是否已在 Solana 链上存证，确保内容真实性。",
        label: "存证/核验此媒体",
    })
}

async fn get_actions_json() -> Json<ActionsJsonResponse> {
    Json(ActionsJsonResponse {
        rules: vec![
            ActionRule {
                path_pattern: "/verify",
                api_path: "/api/actions/verify",
            },
            ActionRule {
                path_pattern: "/api/actions/**",
                api_path: "/api/actions/**",
            },
        ],
    })
}

async fn post_verify_action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ActionPostQuery>,
    request: Result<Json<ActionPostRequest>, JsonRejection>,
) -> Result<Json<ActionPostResponse>, ApiError> {
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");

    if !state.rate_limiter.check(client_ip) {
        return Err(ApiError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "请求过于频繁，请稍后再试".to_string(),
        });
    }

    let request = match request {
        Ok(Json(request)) => request,
        Err(JsonRejection::MissingJsonContentType(_)) | Err(JsonRejection::BytesRejection(_)) => {
            ActionPostRequest {
                account: None,
                image_url: None,
            }
        }
        Err(rejection) => return Err(ApiError::from_json_rejection(rejection)),
    };

    let account_value = request
        .account
        .or(query.account)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required account parameter"))?;

    let account = Pubkey::from_str(&account_value)
        .map_err(|error| ApiError::bad_request(format!("invalid account pubkey: {error}")))?;

    let image_url = request
        .image_url
        .or(query.image_url)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| PLACEHOLDER_ICON_URL.to_string());

    if image_url.len() > MAX_IMAGE_URL_LENGTH {
        return Err(ApiError::bad_request(
            "image data exceeds maximum allowed size (10 MB)",
        ));
    }

    if !image_url.starts_with("https://")
        && !image_url.starts_with("http://")
        && !image_url.starts_with("data:")
    {
        return Err(ApiError::bad_request(
            "image_url must use http://, https://, or data: protocol",
        ));
    }

    info!("正在为图片 {image_url} 构造双重索引交易");

    let (salted_fingerprint, raw_phash) =
        calculate_phash(image_url.clone()).await.map_err(|error| {
            error!("failed to calculate perceptual hash for {image_url}: {error:#}");
            ApiError::bad_request("failed to download or decode the image for pHash generation")
        })?;

    if let Some(indexed) =
        find_registered_content(&state.index_pool, &salted_fingerprint, &raw_phash).await
    {
        let message = match indexed {
            VerificationMatch::Exact(content) => format!(
                "✅ 官方原图存证。该内容已由 {} 于 {} 存证（累计 {} 次存证记录）。",
                content.creator, content.timestamp, content.total_attestations
            ),
            VerificationMatch::Similar(content) if content.distance == 0 => format!(
                "✅ 官方原图存证。该内容已由 {} 于 {} 存证（累计 {} 次存证记录）。",
                content.creator, content.timestamp, content.total_attestations
            ),
            VerificationMatch::Similar(content) => format!(
                "⚠️ 发现高度相似内容（相似度 {}%），疑似二次创作或搬运。原作者：{}（累计 {} 次存证记录）。",
                similarity_percent(content.distance),
                content.creator,
                content.total_attestations
            ),
        };

        return Ok(Json(ActionPostResponse {
            transaction: String::new(),
            message,
        }));
    }

    let recent_blockhash = state
        .rpc_client
        .get_latest_blockhash()
        .await
        .map_err(|error| {
            error!("failed to fetch latest blockhash: {error}");
            ApiError::internal("failed to fetch latest blockhash from the Solana RPC")
        })?;

    let transaction = build_register_content_transaction(
        &state.blinkproof,
        account,
        salted_fingerprint,
        raw_phash,
        recent_blockhash,
    );

    let serialized = bincode::serialize(&transaction).map_err(|error| {
        error!("failed to serialize transaction: {error}");
        ApiError::internal("failed to serialize the signable transaction")
    })?;

    Ok(Json(ActionPostResponse {
        transaction: BASE64.encode(serialized),
        message: "模拟构造 BlinkProof register_content 交易，请在钱包中确认以完成媒体存证/核验。"
            .to_string(),
    }))
}

async fn init_index_pool() -> Option<SqlitePool> {
    let path = resolve_index_db_path();

    if !path.exists() {
        info!(
            "Indexer database not found at {}; duplicate check will be skipped",
            path.display()
        );
        return None;
    }

    let options = match SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display())) {
        Ok(options) => options.read_only(true),
        Err(error) => {
            warn!(
                "failed to build SQLite options for {}: {error}",
                path.display()
            );
            return None;
        }
    };

    match SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
    {
        Ok(pool) => {
            info!("Connected to indexer database at {}", path.display());
            Some(pool)
        }
        Err(error) => {
            warn!(
                "failed to open indexer database at {}; duplicate check will be skipped: {error}",
                path.display()
            );
            None
        }
    }
}

fn resolve_index_db_path() -> PathBuf {
    if let Ok(path) = env::var("BLINK_INDEXER_DB_PATH") {
        return PathBuf::from(path);
    }

    let workspace_path = Path::new(DEFAULT_INDEXER_DB_PATH_FROM_WORKSPACE);
    if workspace_path.exists() {
        return workspace_path.to_path_buf();
    }

    PathBuf::from(DEFAULT_INDEXER_DB_PATH_FROM_SERVICE)
}

async fn find_registered_content(
    pool: &Option<SqlitePool>,
    salted_fingerprint: &[u8; 32],
    raw_phash: &[u8; 8],
) -> Option<VerificationMatch> {
    let pool = pool.as_ref()?;

    match sqlx::query(
        r#"
        SELECT
            rc.creator,
            rc.timestamp,
            COALESCE(ir.total_attestations, 0) AS total_attestations
        FROM registered_content rc
        LEFT JOIN issuer_reputation ir ON rc.creator = ir.issuer
        WHERE rc.salted_fingerprint = ?1
        LIMIT 1
        "#,
    )
    .bind(salted_fingerprint.to_vec())
    .fetch_optional(pool)
    .await
    {
        Ok(Some(row)) => {
            return Some(VerificationMatch::Exact(IndexedContent {
                creator: row.get("creator"),
                timestamp: row.get("timestamp"),
                total_attestations: row.get("total_attestations"),
            }));
        }
        Ok(None) => {}
        Err(error) => {
            warn!("duplicate check failed; continuing with attestation transaction: {error}");
            return None;
        }
    }

    find_similar_registered_content(pool, raw_phash)
        .await
        .map(VerificationMatch::Similar)
}

async fn find_similar_registered_content(
    pool: &SqlitePool,
    target_raw_phash: &[u8; 8],
) -> Option<SimilarContent> {
    let target_int = phash_bytes_to_i64(target_raw_phash);

    let rows = sqlx::query(
        r#"
        SELECT
            rc.raw_phash,
            rc.creator,
            rc.timestamp,
            COALESCE(ir.total_attestations, 0) AS total_attestations
        FROM registered_content rc
        LEFT JOIN issuer_reputation ir ON rc.creator = ir.issuer
        ORDER BY ABS(rc.raw_phash_int - ?1)
        LIMIT 500
        "#,
    )
    .bind(target_int)
    .fetch_all(pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            warn!("similarity scan failed; continuing with attestation transaction: {error}");
            return None;
        }
    };

    rows.into_iter()
        .filter_map(|row| {
            let raw_phash = row.try_get::<Vec<u8>, _>("raw_phash").ok()?;
            let raw_phash = raw_phash.try_into().ok()?;
            let distance = phash_hamming_distance(target_raw_phash, &raw_phash);

            (distance <= SIMILARITY_THRESHOLD).then(|| SimilarContent {
                creator: row.get("creator"),
                timestamp: row.get("timestamp"),
                distance,
                total_attestations: row.get("total_attestations"),
            })
        })
        .min_by_key(|content| content.distance)
}

fn phash_hamming_distance(left: &[u8; 8], right: &[u8; 8]) -> u32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| (left ^ right).count_ones())
        .sum()
}

fn phash_bytes_to_i64(bytes: &[u8; 8]) -> i64 {
    i64::from_be_bytes(*bytes)
}

fn similarity_percent(distance: u32) -> u32 {
    PHASH_BIT_LENGTH
        .saturating_sub(distance)
        .saturating_mul(100)
        / PHASH_BIT_LENGTH
}
