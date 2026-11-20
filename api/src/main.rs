use std::{env, net::SocketAddr, path::Path, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use uuid::Uuid;

const DEFAULT_BIND: &str = "0.0.0.0:21114";
const DEFAULT_DB_PATH: &str = "/data/api.sqlite3";
const DEFAULT_SESSION_HOURS: i64 = 24 * 30;
const MAX_ADDRESS_BOOK_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    session_seconds: i64,
}

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    Unauthorized,
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
            }
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "Usuario, contraseña o sesión no válidos" })),
            )
                .into_response(),
            Self::Internal(err) => {
                error!(error = %err, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Error interno del servidor" })),
                )
                    .into_response()
            }
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        Self::Internal(value.into())
    }
}

#[derive(Debug, FromRow)]
struct UserRecord {
    id: i64,
    username: String,
    password_hash: String,
    display_name: String,
    is_admin: bool,
    status: i64,
}

#[derive(Debug, Serialize)]
struct UserPayload<'a> {
    name: &'a str,
    display_name: &'a str,
    avatar: &'static str,
    email: &'static str,
    note: &'static str,
    status: i64,
    is_admin: bool,
    info: Value,
}

impl<'a> From<&'a UserRecord> for UserPayload<'a> {
    fn from(user: &'a UserRecord) -> Self {
        Self {
            name: &user.username,
            display_name: &user.display_name,
            avatar: "",
            email: "",
            note: "",
            status: user.status,
            is_admin: user.is_admin,
            info: json!({}),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: Option<String>,
    password: Option<String>,
    id: Option<String>,
    uuid: Option<String>,
    #[serde(rename = "deviceInfo")]
    device_info: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct SessionDevice {
    id: Option<String>,
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddressBookUpdate {
    data: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rustdesk_owned_api=info,tower_http=info".into()),
        )
        .init();

    let bind = env::var("API_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let bind_addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("API_BIND no es una dirección válida: {bind}"))?;
    let db_path = env::var("API_DB_PATH").unwrap_or_else(|_| DEFAULT_DB_PATH.to_owned());
    let admin_user = required_env("API_ADMIN_USER")?;
    let admin_password = required_env("API_ADMIN_PASSWORD")?;
    let session_hours = env::var("API_SESSION_HOURS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SESSION_HOURS);

    ensure_parent_directory(&db_path)?;
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{db_path}"))?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(options).await?;
    migrate(&pool).await?;
    ensure_admin(&pool, &admin_user, &admin_password).await?;

    let state = Arc::new(AppState {
        pool,
        session_seconds: session_hours.saturating_mul(60 * 60),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/login-options", get(login_options))
        .route("/api/login", post(login))
        .route("/api/currentUser", post(current_user))
        .route("/api/logout", post(logout))
        .route("/api/ab", get(get_address_book).post(put_address_book))
        .route(
            "/api/ab/personal",
            post(personal_address_book_not_supported),
        )
        .layer(DefaultBodyLimit::max(MAX_ADDRESS_BOOK_BYTES + 64 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(address = %bind_addr, "RustDesk owned API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("Falta la variable {name}"))?;
    if value.trim().is_empty() {
        anyhow::bail!("La variable {name} no puede estar vacía");
    }
    Ok(value)
}

fn ensure_parent_directory(db_path: &str) -> Result<()> {
    let parent = Path::new(db_path)
        .parent()
        .context("API_DB_PATH no tiene directorio padre")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("No se pudo crear {}", parent.display()))?;
    Ok(())
}

async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            display_name TEXT NOT NULL DEFAULT '',
            is_admin INTEGER NOT NULL DEFAULT 0,
            status INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            rustdesk_id TEXT,
            device_uuid TEXT,
            device_info TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            expires_at INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS address_books (
            user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            data TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS sessions_user_id_idx ON sessions(user_id)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS sessions_expires_at_idx ON sessions(expires_at)")
        .execute(pool)
        .await?;
    Ok(())
}

async fn ensure_admin(pool: &SqlitePool, username: &str, password: &str) -> Result<()> {
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .to_string();
    sqlx::query(
        r#"
        INSERT INTO users (username, password_hash, display_name, is_admin, status)
        VALUES (?, ?, ?, 1, 1)
        ON CONFLICT(username) DO UPDATE SET
            password_hash = excluded.password_hash,
            display_name = excluded.display_name,
            is_admin = 1,
            status = 1
        "#,
    )
    .bind(username)
    .bind(password_hash)
    .bind(username)
    .execute(pool)
    .await?;
    info!(username, "administrator synchronized");
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html lang="es"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>RustDesk API</title><style>body{font-family:system-ui;background:#111827;color:#f9fafb;max-width:760px;margin:10vh auto;padding:32px}code{background:#1f2937;padding:.2rem .4rem;border-radius:.3rem}.ok{color:#34d399}</style></head><body><h1>RustDesk API propia</h1><p class="ok">● Servicio activo</p><p>API compatible inicial para cuentas y libreta personal de RustDesk.</p><p>Estado: <code>/health</code></p></body></html>"#,
    )
}

async fn health() -> Json<Value> {
    Json(
        json!({ "status": "ok", "service": "rustdesk-owned-api", "version": env!("CARGO_PKG_VERSION") }),
    )
}

async fn login_options() -> Json<Value> {
    Json(json!([]))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(input): Json<LoginRequest>,
) -> Result<Json<Value>, AppError> {
    let username = input
        .username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("Falta el usuario".to_owned()))?;
    let password = input
        .password
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("Falta la contraseña".to_owned()))?;

    let user = sqlx::query_as::<_, UserRecord>(
        "SELECT id, username, password_hash, display_name, is_admin, status FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Unauthorized)?;

    if user.status != 1 || !verify_password(password, &user.password_hash)? {
        return Err(AppError::Unauthorized);
    }

    let token = Uuid::new_v4().to_string();
    let now = unix_timestamp().map_err(AppError::Internal)?;
    let expires_at = now.saturating_add(state.session_seconds);
    let device_info = input.device_info.map(|value| value.to_string());
    sqlx::query(
        "INSERT INTO sessions (token, user_id, rustdesk_id, device_uuid, device_info, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(user.id)
    .bind(input.id)
    .bind(input.uuid)
    .bind(device_info)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "access_token": token,
        "type": "access_token",
        "tfa_type": "",
        "secret": "",
        "user": UserPayload::from(&user)
    })))
}

async fn current_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_device): Json<SessionDevice>,
) -> Result<Json<Value>, AppError> {
    let user = authenticated_user(&state, &headers).await?;
    Ok(Json(json!(UserPayload::from(&user))))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Option<Json<SessionDevice>>,
) -> Result<Json<Value>, AppError> {
    if let Some(Json(device)) = request {
        let _ = (device.id, device.uuid);
    }
    let token = bearer_token(&headers)?;
    sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(&state.pool)
        .await?;
    Ok(Json(Value::Null))
}

async fn personal_address_book_not_supported() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "legacy address book mode" })),
    )
}

async fn get_address_book(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let user = authenticated_user(&state, &headers).await?;
    let data: Option<String> =
        sqlx::query_scalar("SELECT data FROM address_books WHERE user_id = ?")
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?;
    match data {
        Some(data) => Ok(Json(json!({ "data": data, "licensed_devices": 0 }))),
        None => Ok(Json(Value::Null)),
    }
}

async fn put_address_book(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<AddressBookUpdate>,
) -> Result<Json<Value>, AppError> {
    let user = authenticated_user(&state, &headers).await?;
    if input.data.len() > MAX_ADDRESS_BOOK_BYTES {
        return Err(AppError::BadRequest(
            "La libreta es demasiado grande".to_owned(),
        ));
    }
    let decoded: Value = serde_json::from_str(&input.data)
        .map_err(|_| AppError::BadRequest("La libreta no contiene JSON válido".to_owned()))?;
    if !decoded.is_object() {
        return Err(AppError::BadRequest(
            "La libreta debe ser un objeto JSON".to_owned(),
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO address_books (user_id, data, updated_at)
        VALUES (?, ?, unixepoch())
        ON CONFLICT(user_id) DO UPDATE SET data = excluded.data, updated_at = unixepoch()
        "#,
    )
    .bind(user.id)
    .bind(input.data)
    .execute(&state.pool)
    .await?;
    Ok(Json(Value::Null))
}

async fn authenticated_user(state: &AppState, headers: &HeaderMap) -> Result<UserRecord, AppError> {
    let token = bearer_token(headers)?;
    let now = unix_timestamp().map_err(AppError::Internal)?;
    let user = sqlx::query_as::<_, UserRecord>(
        r#"
        SELECT u.id, u.username, u.password_hash, u.display_name, u.is_admin, u.status
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        WHERE s.token = ? AND s.expires_at > ? AND u.status = 1
        "#,
    )
    .bind(token)
    .bind(now)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::Unauthorized)?;
    Ok(user)
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    value
        .strip_prefix("Bearer ")
        .filter(|token| !token.trim().is_empty())
        .ok_or(AppError::Unauthorized)
}

fn verify_password(password: &str, encoded_hash: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(encoded_hash)
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

fn unix_timestamp() -> Result<i64> {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("El reloj del sistema está antes de UNIX_EPOCH")?;
    i64::try_from(duration.as_secs()).context("La hora del sistema no cabe en i64")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => error!(error = %err, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
