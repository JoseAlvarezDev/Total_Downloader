use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    io::ErrorKind,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    process::Command,
    sync::{Mutex, Semaphore},
    time::{Duration, timeout},
};
use tokio_util::io::ReaderStream;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing::{debug, info, warn};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    history: Arc<Mutex<Vec<HistoryEntry>>>,
    history_path: PathBuf,
    rate_limits: Arc<Mutex<RateLimitMap>>,
    rate_limit_path: PathBuf,
    anti_bot_challenges: Arc<Mutex<AntiBotChallengeMap>>,
    download_semaphore: Arc<Semaphore>,
    trust_proxy_headers: bool,
    turnstile_secret_key: Option<String>,
    http_client: reqwest::Client,
    transfer_dir: PathBuf,
}

type RateLimitMap = HashMap<String, Vec<DateTime<Utc>>>;
type AntiBotChallengeMap = HashMap<String, AntiBotChallenge>;

const DOWNLOAD_LIMIT_PER_DAY: usize = 10;
const DOWNLOAD_WINDOW_HOURS: i64 = 24;
const ANTIBOT_DIFFICULTY_HEX_PREFIX: usize = 3;
const ANTIBOT_CHALLENGE_TTL_SECONDS: i64 = 5 * 60;
const ANTIBOT_MIN_ELAPSED_MS: u64 = 900;
const MAX_ANTIBOT_CHALLENGES: usize = 20_000;
const DEFAULT_MAX_CONCURRENT_DOWNLOADS: usize = 3;
const YT_DLP_TIMEOUT_SECONDS: u64 = 180;
const MAX_DOWNLOAD_BYTES: u64 = 250 * 1024 * 1024;
const TURNSTILE_TIMEOUT_SECONDS: u64 = 10;
const DOWNLOAD_JOB_RETENTION_SECONDS: u64 = 20 * 60;
const STALE_DOWNLOAD_JOB_SECONDS: u64 = 2 * 60 * 60;
const HISTORY_PER_IP_LIMIT: usize = 10;
const HISTORY_MAX_ENTRIES: usize = 2_000;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
enum DownloadMode {
    Video,
    Audio,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
enum DownloadStatus {
    Success,
    Failed,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    id: Uuid,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing)]
    requester_ip: String,
    url: String,
    title: Option<String>,
    thumbnail: Option<String>,
    mode: DownloadMode,
    format: String,
    status: DownloadStatus,
    saved_path: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FormatsRequest {
    url: String,
}

#[derive(Debug, Serialize)]
struct FormatsResponse {
    title: String,
    thumbnail: Option<String>,
    video_options: Vec<FormatOption>,
    audio_options: Vec<FormatOption>,
}

#[derive(Debug, Serialize)]
struct FormatOption {
    format_id: String,
    label: String,
    resolution: Option<String>,
    ext: String,
    has_audio: bool,
}

#[derive(Debug, Deserialize)]
struct DownloadRequest {
    url: String,
    title: Option<String>,
    thumbnail: Option<String>,
    mode: DownloadMode,
    format_id: Option<String>,
    format_label: Option<String>,
    has_audio: Option<bool>,
    antibot_challenge_id: Option<String>,
    antibot_solution: Option<u64>,
    antibot_honey: Option<String>,
    antibot_elapsed_ms: Option<u64>,
    turnstile_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<u64>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
    retry_after_seconds: Option<u64>,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    fn daily_limit_exceeded(retry_after_seconds: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: format!(
                "Has superado el limite de {DOWNLOAD_LIMIT_PER_DAY} descargas por IP en 24 horas."
            ),
            code: Some("DAILY_LIMIT_EXCEEDED"),
            retry_after_seconds: Some(retry_after_seconds),
        }
    }

    fn bot_check_failed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            code: Some("BOT_CHECK_FAILED"),
            retry_after_seconds: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: self.message,
            code: self.code,
            retry_after_seconds: self.retry_after_seconds,
        });

        let mut response = (self.status, body).into_response();
        if let Some(seconds) = self.retry_after_seconds
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }

        response
    }
}

#[derive(Debug, Deserialize)]
struct YtDlpVideoInfo {
    title: Option<String>,
    thumbnail: Option<String>,
    formats: Vec<YtDlpFormat>,
}

#[derive(Debug, Deserialize)]
struct YtDlpFormat {
    format_id: String,
    ext: Option<String>,
    vcodec: Option<String>,
    acodec: Option<String>,
    height: Option<u32>,
    fps: Option<f32>,
    format_note: Option<String>,
    tbr: Option<f32>,
    filesize: Option<f64>,
    filesize_approx: Option<f64>,
    abr: Option<f32>,
}

#[derive(Debug, Clone)]
struct AntiBotChallenge {
    nonce: String,
    created_at: DateTime<Utc>,
    ip: String,
}

#[derive(Debug, Serialize)]
struct AntiBotChallengeResponse {
    challenge_id: String,
    nonce: String,
    difficulty: usize,
    expires_in_seconds: i64,
}

#[derive(Debug, Deserialize)]
struct TurnstileVerifyResponse {
    success: bool,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "backend=info,tower_http=info".to_string()),
        )
        .init();

    if let Err(error) = run().await {
        eprintln!("Server error: {}", error.message);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ApiError> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let data_dir = root.join("data");
    let transfer_dir = root.join("temp_downloads");
    let history_path = data_dir.join("history.json");
    let rate_limit_path = data_dir.join("rate_limits.json");

    tokio::fs::create_dir_all(&data_dir)
        .await
        .map_err(|error| {
            ApiError::internal(format!("No se pudo crear la carpeta de datos: {error}"))
        })?;
    tokio::fs::create_dir_all(&transfer_dir)
        .await
        .map_err(|error| {
            ApiError::internal(format!(
                "No se pudo crear la carpeta temporal de descargas: {error}"
            ))
        })?;

    let history = load_history(&history_path).await?;
    let rate_limits = load_rate_limits(&rate_limit_path).await?;
    let max_concurrent_downloads = read_usize_env("MAX_CONCURRENT_DOWNLOADS")
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_DOWNLOADS);
    let trust_proxy_headers = read_bool_env("TRUST_PROXY_HEADERS").unwrap_or(false);
    let turnstile_secret_key = std::env::var("TURNSTILE_SECRET_KEY")
        .ok()
        .and_then(|value| non_empty(&value).map(ToString::to_string));
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(TURNSTILE_TIMEOUT_SECONDS))
        .build()
        .map_err(|error| ApiError::internal(format!("No se pudo crear cliente HTTP: {error}")))?;

    if !trust_proxy_headers {
        warn!(
            "TRUST_PROXY_HEADERS=false: se usara la IP del socket para limitar descargas y anti-bot."
        );
    }
    if turnstile_secret_key.is_some() {
        info!("Turnstile habilitado para verificacion anti-bot.");
    } else {
        warn!("TURNSTILE_SECRET_KEY no configurado. Se usara anti-bot local PoW como fallback.");
    }

    let state = AppState {
        history: Arc::new(Mutex::new(history)),
        history_path,
        rate_limits: Arc::new(Mutex::new(rate_limits)),
        rate_limit_path,
        anti_bot_challenges: Arc::new(Mutex::new(HashMap::new())),
        download_semaphore: Arc::new(Semaphore::new(max_concurrent_downloads)),
        trust_proxy_headers,
        turnstile_secret_key,
        http_client,
        transfer_dir,
    };

    cleanup_stale_download_jobs(&state.transfer_dir, STALE_DOWNLOAD_JOB_SECONDS).await;

    let cors = build_cors_layer()?;

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/antibot/challenge", get(create_antibot_challenge))
        .route("/api/formats", post(fetch_formats))
        .route("/api/download", post(start_download))
        .route("/api/history", get(get_history).delete(clear_history))
        .with_state(state)
        .layer(cors);

    let addr = resolve_bind_addr();
    let listener = TcpListener::bind(&addr).await.map_err(|error| {
        ApiError::internal(format!("No se pudo iniciar el puerto {addr}: {error}"))
    })?;

    info!("Backend listo en http://{addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|error| ApiError::internal(format!("Error del servidor HTTP: {error}")))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

async fn get_history(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let client_ip = client_ip_for_request(&state, &headers, addr);
    let history = state
        .history
        .lock()
        .await
        .iter()
        .filter(|entry| entry.requester_ip == client_ip)
        .take(HISTORY_PER_IP_LIMIT)
        .cloned()
        .collect();
    Ok(Json(history))
}

async fn clear_history(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let client_ip = client_ip_for_request(&state, &headers, addr);

    let snapshot = {
        let mut history = state.history.lock().await;
        history.retain(|entry| entry.requester_ip != client_ip);
        history.clone()
    };

    persist_history(&state.history_path, &snapshot).await?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

async fn create_antibot_challenge(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<AntiBotChallengeResponse>, ApiError> {
    let client_ip = client_ip_for_request(&state, &headers, addr);
    let now = Utc::now();
    let challenge_id = Uuid::new_v4().to_string();
    let nonce = Uuid::new_v4().simple().to_string();

    {
        let mut challenges = state.anti_bot_challenges.lock().await;
        prune_antibot_challenges(&mut challenges, now);
        challenges.insert(
            challenge_id.clone(),
            AntiBotChallenge {
                nonce: nonce.clone(),
                created_at: now,
                ip: client_ip,
            },
        );
        trim_antibot_challenges(&mut challenges);
    }

    Ok(Json(AntiBotChallengeResponse {
        challenge_id,
        nonce,
        difficulty: ANTIBOT_DIFFICULTY_HEX_PREFIX,
        expires_in_seconds: ANTIBOT_CHALLENGE_TTL_SECONDS,
    }))
}

async fn fetch_formats(
    State(_state): State<AppState>,
    Json(payload): Json<FormatsRequest>,
) -> Result<Json<FormatsResponse>, ApiError> {
    let url = payload.url.trim();
    if url.is_empty() {
        return Err(ApiError::bad_request("Ingresa una URL valida."));
    }
    if !is_supported_download_url(url) {
        return Err(ApiError::bad_request(
            "URL no soportada. Usa una URL de X, Facebook, TikTok, YouTube, Instagram o Bluesky.",
        ));
    }

    let output = match run_yt_dlp(vec![
        "-J".to_string(),
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        url.to_string(),
    ])
    .await
    {
        Ok(output) => output,
        Err(error) => {
            if should_use_automatic_formats_fallback(url, &error.message) {
                warn!(
                    "yt-dlp fallo cargando metadatos para URL {:?}. Se devolvera fallback automatico. Error: {}",
                    url, error.message
                );
                return Ok(Json(build_automatic_formats_response(url)));
            }
            return Err(error);
        }
    };

    let info: YtDlpVideoInfo = match serde_json::from_slice(&output.stdout) {
        Ok(info) => info,
        Err(error) => {
            warn!(
                "No se pudo interpretar JSON de yt-dlp para URL {:?}. Se devolvera fallback automatico. Error: {error}",
                url
            );
            return Ok(Json(build_automatic_formats_response(url)));
        }
    };

    let mut video_options = build_video_options(&info.formats);
    let mut audio_options = build_audio_options(&info.formats);

    if video_options.is_empty() {
        video_options.push(FormatOption {
            format_id: "bestvideo+bestaudio/best".to_string(),
            label: "Mejor calidad automatica".to_string(),
            resolution: Some("Auto".to_string()),
            ext: "mp4".to_string(),
            has_audio: true,
        });
    }

    if audio_options.is_empty() {
        audio_options.push(FormatOption {
            format_id: "bestaudio".to_string(),
            label: "Mejor audio disponible".to_string(),
            resolution: None,
            ext: "mp3".to_string(),
            has_audio: true,
        });
    }

    Ok(Json(FormatsResponse {
        title: info
            .title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Sin titulo".to_string()),
        thumbnail: info.thumbnail,
        video_options,
        audio_options,
    }))
}

async fn start_download(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(payload): Json<DownloadRequest>,
) -> Result<Response, ApiError> {
    struct PreparedDownload {
        body: Body,
        filename: String,
        content_type: &'static str,
        content_length: u64,
        job_dir: PathBuf,
    }

    let url = payload.url.trim();
    if url.is_empty() {
        return Err(ApiError::bad_request(
            "Ingresa una URL valida antes de descargar.",
        ));
    }
    if !is_supported_download_url(url) {
        return Err(ApiError::bad_request(
            "URL no soportada. Usa una URL de X, Facebook, TikTok, YouTube, Instagram o Bluesky.",
        ));
    }

    let client_ip = client_ip_for_request(&state, &headers, addr);
    verify_request_protection(&state, &client_ip, &payload).await?;
    register_download_attempt(&state, &client_ip).await?;
    let _download_permit = state
        .download_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("No se pudo reservar capacidad de descarga."))?;
    cleanup_stale_download_jobs(&state.transfer_dir, STALE_DOWNLOAD_JOB_SECONDS).await;

    let selected_format = payload
        .format_label
        .clone()
        .or_else(|| payload.format_id.clone())
        .unwrap_or_else(|| "Mejor calidad automatica".to_string());
    let selected_title = payload.title.clone().and_then(normalize_optional_text);
    let selected_thumbnail = payload.thumbnail.clone().and_then(normalize_optional_text);

    let job_dir = state.transfer_dir.join(Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&job_dir).await.map_err(|error| {
        ApiError::internal(format!("No se pudo preparar la descarga temporal: {error}"))
    })?;

    let output_template = format!("{}/%(title).140B-%(id)s.%(ext)s", job_dir.to_string_lossy());

    let mut args = vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--newline".to_string(),
        "--print".to_string(),
        "after_move:filepath".to_string(),
        "-o".to_string(),
        output_template,
    ];

    match payload.mode.clone() {
        DownloadMode::Video => {
            let selector = payload
                .format_id
                .as_deref()
                .and_then(non_empty)
                .map(|format_id| {
                    if payload.has_audio.unwrap_or(false) {
                        format_id.to_string()
                    } else {
                        format!("{format_id}+bestaudio/best")
                    }
                })
                .unwrap_or_else(|| "bestvideo+bestaudio/best".to_string());

            args.push("-f".to_string());
            args.push(selector);
        }
        DownloadMode::Audio => {
            let selector = payload
                .format_id
                .as_deref()
                .and_then(non_empty)
                .unwrap_or("bestaudio")
                .to_string();

            args.push("-f".to_string());
            args.push(selector);
            args.push("-x".to_string());
            args.push("--audio-format".to_string());
            args.push("mp3".to_string());
            args.push("--audio-quality".to_string());
            args.push("0".to_string());
        }
    }

    args.push(url.to_string());

    let preparation_result: Result<PreparedDownload, ApiError> = async {
        let output = run_yt_dlp(args).await?;
        let printed_path = extract_printed_path(&output.stdout);
        let resolved_path = resolve_downloaded_file(&job_dir, printed_path.as_deref()).await?;

        let filename = resolved_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
            .unwrap_or_else(|| "download.bin".to_string());
        let metadata = tokio::fs::metadata(&resolved_path).await.map_err(|error| {
            ApiError::internal(format!(
                "No se pudo leer metadata del archivo temporal: {error}"
            ))
        })?;
        if metadata.len() > MAX_DOWNLOAD_BYTES {
            let max_mb = MAX_DOWNLOAD_BYTES / 1_048_576;
            return Err(ApiError::bad_request(format!(
                "El archivo supera el limite permitido de {max_mb} MB."
            )));
        }

        let file = tokio::fs::File::open(&resolved_path)
            .await
            .map_err(|error| {
                ApiError::internal(format!("No se pudo leer el archivo temporal: {error}"))
            })?;
        let stream = ReaderStream::new(file);
        let body = Body::from_stream(stream);

        Ok(PreparedDownload {
            body,
            filename: filename.clone(),
            content_type: content_type_for_filename(&filename),
            content_length: metadata.len(),
            job_dir: job_dir.clone(),
        })
    }
    .await;

    match preparation_result {
        Ok(prepared) => {
            let entry = HistoryEntry {
                id: Uuid::new_v4(),
                created_at: Utc::now(),
                requester_ip: client_ip.clone(),
                url: url.to_string(),
                title: selected_title,
                thumbnail: selected_thumbnail,
                mode: payload.mode,
                format: selected_format,
                status: DownloadStatus::Success,
                saved_path: Some(prepared.filename.clone()),
                error: None,
            };

            if let Err(error) = push_history(&state, entry).await {
                cleanup_download_job(&prepared.job_dir).await;
                return Err(error);
            }

            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static(prepared.content_type),
            );
            headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&prepared.content_length.to_string())
                    .map_err(|_| ApiError::internal("No se pudo crear el tamano de descarga."))?,
            );

            let content_disposition = build_content_disposition(&prepared.filename);
            headers.insert(
                CONTENT_DISPOSITION,
                HeaderValue::from_str(&content_disposition)
                    .map_err(|_| ApiError::internal("No se pudo crear la cabecera de descarga."))?,
            );

            let safe_header_filename = sanitize_ascii_filename(&prepared.filename);
            headers.insert(
                HeaderName::from_static("x-download-filename"),
                HeaderValue::from_str(&safe_header_filename)
                    .map_err(|_| ApiError::internal("No se pudo crear el nombre del archivo."))?,
            );

            schedule_cleanup_download_job(prepared.job_dir);
            Ok((headers, prepared.body).into_response())
        }
        Err(error) => {
            cleanup_download_job(&job_dir).await;
            let entry = HistoryEntry {
                id: Uuid::new_v4(),
                created_at: Utc::now(),
                requester_ip: client_ip,
                url: url.to_string(),
                title: selected_title,
                thumbnail: selected_thumbnail,
                mode: payload.mode,
                format: selected_format,
                status: DownloadStatus::Failed,
                saved_path: None,
                error: Some(error.message.clone()),
            };

            push_history(&state, entry).await?;
            Err(error)
        }
    }
}

fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    let check_header = |key: &str| {
        headers
            .get(key)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    };

    if let Some(forwarded) = check_header("x-forwarded-for") {
        let first_ip = forwarded
            .split(',')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        if first_ip.is_some() {
            return first_ip;
        }
    }

    check_header("cf-connecting-ip").or_else(|| check_header("x-real-ip"))
}

fn client_ip_for_request(state: &AppState, headers: &HeaderMap, addr: SocketAddr) -> String {
    if state.trust_proxy_headers {
        extract_client_ip(headers).unwrap_or_else(|| addr.ip().to_string())
    } else {
        addr.ip().to_string()
    }
}

fn read_bool_env(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn read_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
}

fn resolve_bind_addr() -> String {
    if let Some(configured) = std::env::var("APP_ADDR")
        .ok()
        .and_then(|value| non_empty(&value).map(ToString::to_string))
    {
        return configured;
    }

    if let Some(port) = std::env::var("PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
    {
        return format!("0.0.0.0:{port}");
    }

    "127.0.0.1:8787".to_string()
}

fn build_cors_layer() -> Result<CorsLayer, ApiError> {
    let configured = std::env::var("ALLOWED_ORIGINS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let origins = if configured.is_empty() {
        warn!("ALLOWED_ORIGINS no esta configurado. Se usaran origenes de desarrollo por defecto.");
        vec![
            "http://127.0.0.1:5173".to_string(),
            "http://localhost:5173".to_string(),
        ]
    } else {
        configured
    };

    let normalized_origins = origins
        .iter()
        .map(|origin| {
            normalize_origin(origin).ok_or_else(|| {
                ApiError::internal(format!(
                    "Origen invalido en ALLOWED_ORIGINS: {origin}. Usa valores tipo https://dominio.com"
                ))
            })
        })
        .collect::<Result<HashSet<_>, _>>()?;
    let allowed_origins = Arc::new(normalized_origins);
    let allow_origin = AllowOrigin::predicate({
        let allowed_origins = Arc::clone(&allowed_origins);
        move |origin: &HeaderValue, _| {
            let normalized = origin.to_str().ok().and_then(normalize_origin);
            let allowed = normalized
                .as_ref()
                .is_some_and(|value| allowed_origins.contains(value));
            debug!(
                "CORS origin check raw={:?} normalized={:?} allowed={}",
                origin, normalized, allowed
            );
            allowed
        }
    });
    let configured_origin_list = allowed_origins.iter().cloned().collect::<Vec<_>>();
    info!(
        "CORS allow-list cargada con {} origen(es): {:?}",
        configured_origin_list.len(),
        configured_origin_list
    );

    Ok(CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(Any)
        .expose_headers([
            CONTENT_DISPOSITION,
            HeaderName::from_static("x-download-filename"),
        ]))
}

fn normalize_origin(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let scheme = parsed.scheme();
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let port = parsed.port();

    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return None;
    }

    let include_port = port.is_some_and(|explicit| explicit != default_port);

    if include_port {
        Some(format!("{scheme}://{host}:{}", port?))
    } else {
        Some(format!("{scheme}://{host}"))
    }
}

async fn register_download_attempt(state: &AppState, ip: &str) -> Result<(), ApiError> {
    let now = Utc::now();
    let window_start = now - chrono::Duration::hours(DOWNLOAD_WINDOW_HOURS);

    let (snapshot, retry_after_seconds) = {
        let mut rate_limits = state.rate_limits.lock().await;
        let entries = rate_limits.entry(ip.to_string()).or_default();
        entries.sort();
        entries.retain(|timestamp| *timestamp > window_start);

        let retry_after_seconds = if entries.len() >= DOWNLOAD_LIMIT_PER_DAY {
            let reset_at = entries
                .first()
                .cloned()
                .map(|value| value + chrono::Duration::hours(DOWNLOAD_WINDOW_HOURS))
                .unwrap_or_else(|| now + chrono::Duration::hours(DOWNLOAD_WINDOW_HOURS));
            Some((reset_at - now).num_seconds().max(1) as u64)
        } else {
            entries.push(now);
            entries.sort();
            None
        };

        (rate_limits.clone(), retry_after_seconds)
    };

    persist_rate_limits(&state.rate_limit_path, &snapshot).await?;

    if let Some(retry_after_seconds) = retry_after_seconds {
        return Err(ApiError::daily_limit_exceeded(retry_after_seconds));
    }

    Ok(())
}

async fn verify_request_protection(
    state: &AppState,
    client_ip: &str,
    payload: &DownloadRequest,
) -> Result<(), ApiError> {
    if payload
        .antibot_honey
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(ApiError::bot_check_failed(
            "Solicitud bloqueada por filtro anti-bot.",
        ));
    }

    if state.turnstile_secret_key.is_some() {
        let token = payload
            .turnstile_token
            .as_deref()
            .and_then(non_empty)
            .ok_or_else(|| {
                ApiError::bot_check_failed(
                    "Completa la verificacion anti-bot para continuar con la descarga.",
                )
            })?;
        verify_turnstile_token(state, token, client_ip).await
    } else {
        validate_antibot(state, client_ip, payload).await
    }
}

async fn verify_turnstile_token(
    state: &AppState,
    token: &str,
    client_ip: &str,
) -> Result<(), ApiError> {
    let secret = state
        .turnstile_secret_key
        .as_deref()
        .ok_or_else(|| ApiError::internal("Turnstile no esta configurado en el backend."))?;

    let response = state
        .http_client
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form(&[
            ("secret", secret),
            ("response", token),
            ("remoteip", client_ip),
        ])
        .send()
        .await
        .map_err(|error| {
            warn!("Error consultando Turnstile: {error}");
            ApiError::bot_check_failed("No se pudo validar anti-bot. Intenta nuevamente.")
        })?;

    if !response.status().is_success() {
        warn!(
            "Turnstile respondio con estado HTTP no exitoso: {}",
            response.status()
        );
        return Err(ApiError::bot_check_failed(
            "No se pudo validar anti-bot. Intenta nuevamente.",
        ));
    }

    let verification = response
        .json::<TurnstileVerifyResponse>()
        .await
        .map_err(|error| {
            warn!("Respuesta invalida de Turnstile: {error}");
            ApiError::bot_check_failed("No se pudo validar anti-bot. Intenta nuevamente.")
        })?;

    if !verification.success {
        warn!(
            "Turnstile rechazo la solicitud para IP {}: {:?}",
            client_ip, verification.error_codes
        );
        return Err(ApiError::bot_check_failed(
            "Turnstile rechazo la verificacion anti-bot. Recarga la pagina y reintenta.",
        ));
    }

    Ok(())
}

async fn validate_antibot(
    state: &AppState,
    client_ip: &str,
    payload: &DownloadRequest,
) -> Result<(), ApiError> {
    if payload
        .antibot_honey
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(ApiError::bot_check_failed(
            "Solicitud bloqueada por filtro anti-bot.",
        ));
    }

    if payload.antibot_elapsed_ms.unwrap_or_default() < ANTIBOT_MIN_ELAPSED_MS {
        return Err(ApiError::bot_check_failed(
            "No se pudo validar el tiempo minimo anti-bot. Espera un momento y reintenta.",
        ));
    }

    let challenge_id = payload
        .antibot_challenge_id
        .as_deref()
        .and_then(non_empty)
        .ok_or_else(|| ApiError::bot_check_failed("Falta challenge anti-bot."))?;

    let solution = payload
        .antibot_solution
        .ok_or_else(|| ApiError::bot_check_failed("Falta solucion anti-bot."))?;

    let challenge = {
        let mut challenges = state.anti_bot_challenges.lock().await;
        prune_antibot_challenges(&mut challenges, Utc::now());
        challenges.remove(challenge_id)
    }
    .ok_or_else(|| {
        ApiError::bot_check_failed("Challenge anti-bot invalido o expirado. Actualiza y reintenta.")
    })?;

    if challenge.ip != client_ip {
        return Err(ApiError::bot_check_failed(
            "Challenge anti-bot no coincide con el origen de la solicitud.",
        ));
    }

    if !is_pow_solution_valid(challenge_id, &challenge.nonce, solution) {
        return Err(ApiError::bot_check_failed(
            "No se pudo validar la prueba anti-bot. Intenta nuevamente.",
        ));
    }

    Ok(())
}

async fn push_history(state: &AppState, entry: HistoryEntry) -> Result<(), ApiError> {
    let snapshot = {
        let mut history = state.history.lock().await;
        history.insert(0, entry);
        trim_history_limits(&mut history);
        history.clone()
    };

    persist_history(&state.history_path, &snapshot).await
}

async fn load_history(path: &Path) -> Result<Vec<HistoryEntry>, ApiError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => {
            let mut entries: Vec<HistoryEntry> =
                serde_json::from_str(&contents).map_err(|error| {
                    ApiError::internal(format!("No se pudo leer el historial local: {error}"))
                })?;
            trim_history_limits(&mut entries);
            Ok(entries)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(ApiError::internal(format!(
            "No se pudo abrir el historial local: {error}"
        ))),
    }
}

async fn persist_history(path: &Path, history: &[HistoryEntry]) -> Result<(), ApiError> {
    let payload = serde_json::to_string_pretty(history).map_err(|error| {
        ApiError::internal(format!("No se pudo serializar el historial local: {error}"))
    })?;

    tokio::fs::write(path, payload)
        .await
        .map_err(|error| ApiError::internal(format!("No se pudo guardar el historial: {error}")))
}

fn trim_history_limits(entries: &mut Vec<HistoryEntry>) {
    let mut counters: HashMap<String, usize> = HashMap::new();
    entries.retain(|entry| {
        let counter = counters.entry(entry.requester_ip.clone()).or_insert(0);
        if *counter >= HISTORY_PER_IP_LIMIT {
            false
        } else {
            *counter += 1;
            true
        }
    });

    if entries.len() > HISTORY_MAX_ENTRIES {
        entries.truncate(HISTORY_MAX_ENTRIES);
    }
}

async fn load_rate_limits(path: &Path) -> Result<RateLimitMap, ApiError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => {
            let mut map: RateLimitMap = serde_json::from_str(&contents).map_err(|error| {
                ApiError::internal(format!("No se pudo leer limites de descarga: {error}"))
            })?;

            let now = Utc::now();
            let window_start = now - chrono::Duration::hours(DOWNLOAD_WINDOW_HOURS);
            map.retain(|_, timestamps| {
                timestamps.sort();
                timestamps.retain(|timestamp| *timestamp > window_start);
                !timestamps.is_empty()
            });

            Ok(map)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(ApiError::internal(format!(
            "No se pudo abrir archivo de limites de descarga: {error}"
        ))),
    }
}

async fn persist_rate_limits(path: &Path, rate_limits: &RateLimitMap) -> Result<(), ApiError> {
    let payload = serde_json::to_string_pretty(rate_limits).map_err(|error| {
        ApiError::internal(format!(
            "No se pudo serializar limites de descarga: {error}"
        ))
    })?;

    tokio::fs::write(path, payload).await.map_err(|error| {
        ApiError::internal(format!("No se pudo guardar limites de descarga: {error}"))
    })
}

fn build_video_options(formats: &[YtDlpFormat]) -> Vec<FormatOption> {
    let mut options: Vec<(u32, f32, f32, FormatOption)> = formats
        .iter()
        .filter(|item| has_video(item))
        .map(|item| {
            let ext = item.ext.clone().unwrap_or_else(|| "mp4".to_string());
            let resolution = item
                .height
                .map(|height| format!("{height}p"))
                .or_else(|| item.format_note.clone())
                .unwrap_or_else(|| "Video".to_string());

            let has_audio = has_audio(item);
            let size_label = item
                .filesize
                .or(item.filesize_approx)
                .map(format_filesize_mb)
                .unwrap_or_else(|| "tamano variable".to_string());
            let fps_label = item
                .fps
                .filter(|fps| *fps > 0.0)
                .map(|fps| format!("{}fps", fps.round() as u32))
                .unwrap_or_else(|| "fps variable".to_string());

            let label = format!(
                "{resolution} · {} · {fps_label} · {size_label} · {}",
                ext.to_uppercase(),
                if has_audio { "con audio" } else { "sin audio" }
            );

            let option = FormatOption {
                format_id: item.format_id.clone(),
                label,
                resolution: Some(resolution),
                ext,
                has_audio,
            };

            (
                item.height.unwrap_or_default(),
                item.fps.unwrap_or_default(),
                item.tbr.unwrap_or_default(),
                option,
            )
        })
        .collect();

    options.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal))
    });

    let mut deduped = Vec::new();
    let mut seen_ids = HashSet::new();

    for (_, _, _, option) in options {
        if seen_ids.insert(option.format_id.clone()) {
            deduped.push(option);
        }
    }

    deduped
}

fn build_audio_options(formats: &[YtDlpFormat]) -> Vec<FormatOption> {
    let mut options: Vec<(f32, f32, FormatOption)> = formats
        .iter()
        .filter(|item| has_audio_only(item))
        .map(|item| {
            let ext = item.ext.clone().unwrap_or_else(|| "m4a".to_string());
            let bitrate = item.abr.unwrap_or(item.tbr.unwrap_or_default());
            let bitrate_label = if bitrate > 0.0 {
                format!("{} kbps", bitrate.round() as u32)
            } else {
                "bitrate variable".to_string()
            };
            let size_label = item
                .filesize
                .or(item.filesize_approx)
                .map(format_filesize_mb)
                .unwrap_or_else(|| "tamano variable".to_string());

            let label = format!(
                "Audio · {} · {bitrate_label} · {size_label}",
                ext.to_uppercase()
            );

            (
                item.abr.unwrap_or_default(),
                item.tbr.unwrap_or_default(),
                FormatOption {
                    format_id: item.format_id.clone(),
                    label,
                    resolution: None,
                    ext,
                    has_audio: true,
                },
            )
        })
        .collect();

    options.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
    });

    let mut deduped = Vec::new();
    let mut seen_ids = HashSet::new();

    for (_, _, option) in options {
        if seen_ids.insert(option.format_id.clone()) {
            deduped.push(option);
        }
    }

    deduped
}

fn run_error_message(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()
        .unwrap_or("yt-dlp no pudo completar la operacion")
        .to_string();
    let lower = message.to_ascii_lowercase();

    if lower.contains("unsupported url") {
        "URL no soportada o invalida para descarga.".to_string()
    } else if lower.contains("json object must be str, bytes or bytearray, not nonetype")
        || lower.contains("nonetype")
    {
        "No se pudieron obtener metadatos de la URL. Intenta con formato automatico o reintenta mas tarde.".to_string()
    } else {
        message
    }
}

async fn run_yt_dlp(args: Vec<String>) -> Result<std::process::Output, ApiError> {
    let command_future = Command::new("yt-dlp").args(args).output();
    let output = timeout(Duration::from_secs(YT_DLP_TIMEOUT_SECONDS), command_future)
        .await
        .map_err(|_| {
            ApiError::bad_request(
                "La descarga excedio el tiempo limite. Intenta con otra URL o formato.",
            )
        })?
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                ApiError::internal(
                    "yt-dlp no esta instalado en el sistema. Instala yt-dlp y reinicia el backend.",
                )
            } else {
                ApiError::internal(format!("No se pudo ejecutar yt-dlp: {error}"))
            }
        })?;

    if !output.status.success() {
        return Err(ApiError::bad_request(run_error_message(&output.stderr)));
    }

    Ok(output)
}

fn extract_printed_path(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()
        .map(ToString::to_string)
}

async fn resolve_downloaded_file(
    job_dir: &Path,
    printed_path: Option<&str>,
) -> Result<PathBuf, ApiError> {
    let canonical_job_dir = tokio::fs::canonicalize(job_dir).await.map_err(|error| {
        ApiError::internal(format!("No se pudo resolver carpeta temporal: {error}"))
    })?;

    if let Some(path_value) = printed_path {
        let path = PathBuf::from(path_value);
        if let Some(valid_path) = resolve_download_candidate(&canonical_job_dir, &path).await? {
            return Ok(valid_path);
        }

        let relative_candidate = job_dir.join(path_value);
        if let Some(valid_path) =
            resolve_download_candidate(&canonical_job_dir, &relative_candidate).await?
        {
            return Ok(valid_path);
        }
    }

    let mut entries = tokio::fs::read_dir(job_dir).await.map_err(|error| {
        ApiError::internal(format!("No se pudo abrir la carpeta temporal: {error}"))
    })?;

    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        ApiError::internal(format!("No se pudo leer archivos temporales: {error}"))
    })? {
        let path = entry.path();
        if let Some(valid_path) = resolve_download_candidate(&canonical_job_dir, &path).await? {
            return Ok(valid_path);
        }
    }

    Err(ApiError::internal(
        "No se encontro el archivo descargado para transferir al dispositivo.",
    ))
}

async fn resolve_download_candidate(
    canonical_job_dir: &Path,
    candidate_path: &Path,
) -> Result<Option<PathBuf>, ApiError> {
    let metadata = match tokio::fs::metadata(candidate_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ApiError::internal(format!(
                "No se pudo leer archivo temporal descargado: {error}"
            )));
        }
    };

    if !metadata.is_file() {
        return Ok(None);
    }

    let canonical_candidate = tokio::fs::canonicalize(candidate_path)
        .await
        .map_err(|error| {
            ApiError::internal(format!(
                "No se pudo resolver ruta temporal descargada: {error}"
            ))
        })?;

    if !canonical_candidate.starts_with(canonical_job_dir) {
        warn!(
            "Se bloqueo un archivo fuera de la carpeta temporal esperada: {:?}",
            canonical_candidate
        );
        return Ok(None);
    }

    Ok(Some(canonical_candidate))
}

async fn cleanup_download_job(job_dir: &Path) {
    if let Err(error) = tokio::fs::remove_dir_all(job_dir).await {
        if error.kind() != ErrorKind::NotFound {
            info!("No se pudo limpiar carpeta temporal: {error}");
        }
    }
}

fn schedule_cleanup_download_job(job_dir: PathBuf) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(DOWNLOAD_JOB_RETENTION_SECONDS)).await;
        cleanup_download_job(&job_dir).await;
    });
}

async fn cleanup_stale_download_jobs(transfer_dir: &Path, older_than_secs: u64) {
    if older_than_secs == 0 {
        return;
    }

    let mut entries = match tokio::fs::read_dir(transfer_dir).await {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != ErrorKind::NotFound {
                warn!("No se pudo abrir carpeta temporal para limpieza: {error}");
            }
            return;
        }
    };

    let max_age = Duration::from_secs(older_than_secs);
    let now = std::time::SystemTime::now();

    loop {
        let maybe_entry = match entries.next_entry().await {
            Ok(value) => value,
            Err(error) => {
                warn!("No se pudo iterar carpeta temporal para limpieza: {error}");
                break;
            }
        };

        let Some(entry) = maybe_entry else {
            break;
        };

        let path = entry.path();
        let metadata = match entry.metadata().await {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!("No se pudo leer metadata de {:?}: {error}", path);
                continue;
            }
        };

        let modified_at = match metadata.modified() {
            Ok(value) => value,
            Err(error) => {
                warn!(
                    "No se pudo leer fecha de modificacion de {:?}: {error}",
                    path
                );
                continue;
            }
        };

        let age = match now.duration_since(modified_at) {
            Ok(value) => value,
            Err(_) => Duration::from_secs(0),
        };

        if age < max_age {
            continue;
        }

        if metadata.is_dir() {
            if let Err(error) = tokio::fs::remove_dir_all(&path).await
                && error.kind() != ErrorKind::NotFound
            {
                warn!("No se pudo eliminar carpeta temporal {:?}: {error}", path);
            }
        } else if metadata.is_file()
            && let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != ErrorKind::NotFound
        {
            warn!("No se pudo eliminar archivo temporal {:?}: {error}", path);
        }
    }
}

fn prune_antibot_challenges(challenges: &mut AntiBotChallengeMap, now: DateTime<Utc>) {
    challenges.retain(|_, challenge| {
        (now - challenge.created_at).num_seconds() <= ANTIBOT_CHALLENGE_TTL_SECONDS
    });
}

fn trim_antibot_challenges(challenges: &mut AntiBotChallengeMap) {
    if challenges.len() <= MAX_ANTIBOT_CHALLENGES {
        return;
    }

    let overflow = challenges.len() - MAX_ANTIBOT_CHALLENGES;
    let mut oldest = challenges
        .iter()
        .map(|(id, challenge)| (id.clone(), challenge.created_at))
        .collect::<Vec<_>>();
    oldest.sort_by_key(|(_, created_at)| *created_at);

    for (id, _) in oldest.into_iter().take(overflow) {
        challenges.remove(&id);
    }
}

fn is_supported_download_url(input: &str) -> bool {
    let parsed = match Url::parse(input) {
        Ok(url) => url,
        Err(_) => return false,
    };

    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }

    let host = match parsed.host_str() {
        Some(host) => host.to_ascii_lowercase(),
        None => return false,
    };

    const SUPPORTED_DOMAINS: [&str; 14] = [
        "youtube.com",
        "youtu.be",
        "x.com",
        "twitter.com",
        "facebook.com",
        "fb.watch",
        "instagram.com",
        "bsky.app",
        "tiktok.com",
        "vm.tiktok.com",
        "vt.tiktok.com",
        "m.youtube.com",
        "music.youtube.com",
        "m.facebook.com",
    ];

    SUPPORTED_DOMAINS
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn is_domain_match(input: &str, domain: &str) -> bool {
    Url::parse(input)
        .ok()
        .and_then(|parsed| parsed.host_str().map(ToString::to_string))
        .map(|host| {
            let host = host.to_ascii_lowercase();
            host == domain || host.ends_with(&format!(".{domain}"))
        })
        .unwrap_or(false)
}

fn should_use_automatic_formats_fallback(url: &str, message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let looks_like_extractor_metadata_error = lower
        .contains("json object must be str, bytes or bytearray, not nonetype")
        || (lower.contains("failed to extract") && lower.contains("json"))
        || lower.contains("unable to extract")
        || lower.contains("nonetype")
        || lower.contains("no se pudieron obtener metadatos");

    if !looks_like_extractor_metadata_error {
        return false;
    }

    is_domain_match(url, "tiktok.com")
        || is_domain_match(url, "vm.tiktok.com")
        || is_domain_match(url, "vt.tiktok.com")
        || is_domain_match(url, "bsky.app")
}

fn build_automatic_formats_response(url: &str) -> FormatsResponse {
    let source = Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(ToString::to_string))
        .unwrap_or_else(|| "fuente-desconocida".to_string());

    FormatsResponse {
        title: format!("Modo automatico ({source})"),
        thumbnail: None,
        video_options: vec![FormatOption {
            format_id: "bestvideo+bestaudio/best".to_string(),
            label: "Mejor calidad automatica".to_string(),
            resolution: Some("Auto".to_string()),
            ext: "mp4".to_string(),
            has_audio: true,
        }],
        audio_options: vec![FormatOption {
            format_id: "bestaudio".to_string(),
            label: "Mejor audio disponible".to_string(),
            resolution: None,
            ext: "mp3".to_string(),
            has_audio: true,
        }],
    }
}

fn is_pow_solution_valid(challenge_id: &str, nonce: &str, solution: u64) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(challenge_id.as_bytes());
    hasher.update(b":");
    hasher.update(nonce.as_bytes());
    hasher.update(b":");
    hasher.update(solution.to_string().as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    let prefix = "0".repeat(ANTIBOT_DIFFICULTY_HEX_PREFIX);
    hex.starts_with(&prefix)
}

fn content_type_for_filename(filename: &str) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

fn build_content_disposition(filename: &str) -> String {
    let safe_ascii = sanitize_ascii_filename(filename);
    format!(
        "attachment; filename=\"{safe_ascii}\"; filename*=UTF-8''{}",
        urlencoding::encode(filename)
    )
}

fn sanitize_ascii_filename(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());

    for character in value.chars() {
        if character.is_ascii_alphanumeric()
            || matches!(character, '.' | '-' | '_' | ' ' | '(' | ')')
        {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }

    let compact = sanitized.trim();
    if compact.is_empty() {
        "download.bin".to_string()
    } else {
        compact.to_string()
    }
}

fn has_video(format: &YtDlpFormat) -> bool {
    matches!(format.vcodec.as_deref(), Some(value) if value != "none")
}

fn has_audio(format: &YtDlpFormat) -> bool {
    matches!(format.acodec.as_deref(), Some(value) if value != "none")
}

fn has_audio_only(format: &YtDlpFormat) -> bool {
    !has_video(format) && has_audio(format)
}

fn format_filesize_mb(bytes: f64) -> String {
    let mb = bytes / 1_048_576.0;
    if mb > 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{mb:.1} MB")
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_optional_text(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
