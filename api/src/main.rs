use std::{env, net::SocketAddr, path::Path, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{DefaultBodyLimit, Path as RoutePath, State},
    http::{header::AUTHORIZATION, header::CONTENT_TYPE, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
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
    root_username: String,
}

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict(String),
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
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "Se requieren permisos administrativos" })),
            )
                .into_response(),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "El recurso solicitado no existe" })),
            )
                .into_response(),
            Self::Conflict(message) => {
                (StatusCode::CONFLICT, Json(json!({ "error": message }))).into_response()
            }
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

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    username: String,
    password: String,
    display_name: String,
    #[serde(default)]
    is_admin: bool,
}

#[derive(Debug, Deserialize)]
struct UserStatusRequest {
    active: bool,
}

#[derive(Debug, Deserialize)]
struct UserPasswordRequest {
    password: String,
}

#[derive(Debug, Serialize, FromRow)]
struct AdminUserRecord {
    id: i64,
    username: String,
    display_name: String,
    is_admin: bool,
    status: i64,
    created_at: i64,
    active_sessions: i64,
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
        root_username: admin_user,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/login", get(login_page))
        .route("/admin", get(admin_page))
        .route("/assets/styles.css", get(styles))
        .route("/assets/login.js", get(login_script))
        .route("/assets/admin.js", get(admin_script))
        .route("/favicon.ico", get(favicon))
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
        .route("/admin/api/overview", get(admin_overview))
        .route("/admin/api/users", get(admin_users).post(admin_create_user))
        .route(
            "/admin/api/users/{id}",
            delete(admin_delete_user),
        )
        .route(
            "/admin/api/users/{id}/status",
            put(admin_set_user_status),
        )
        .route(
            "/admin/api/users/{id}/password",
            put(admin_set_user_password),
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
    Html(include_str!("../web/index.html"))
}

async fn login_page() -> Html<&'static str> {
    Html(include_str!("../web/login.html"))
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("../web/admin.html"))
}

async fn styles() -> impl IntoResponse {
    static_asset("text/css; charset=utf-8", include_str!("../web/styles.css"))
}

async fn login_script() -> impl IntoResponse {
    static_asset(
        "text/javascript; charset=utf-8",
        include_str!("../web/login.js"),
    )
}

async fn admin_script() -> impl IntoResponse {
    static_asset(
        "text/javascript; charset=utf-8",
        include_str!("../web/admin.js"),
    )
}

async fn favicon() -> impl IntoResponse {
    static_asset(
        "image/svg+xml",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="17" fill="#b9f227"/><path d="M15 18h7v22h12v6H15zm21 0h7v28h-7zm10 0h7v28h-7z" fill="#090c0b"/></svg>"##,
    )
}

fn static_asset(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
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

async fn admin_overview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let admin = authenticated_admin(&state, &headers).await?;
    let now = unix_timestamp().map_err(AppError::Internal)?;
    let total_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    let active_users: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE status = 1")
            .fetch_one(&state.pool)
            .await?;
    let active_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE expires_at > ?")
            .bind(now)
            .fetch_one(&state.pool)
            .await?;
    let address_books: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM address_books")
        .fetch_one(&state.pool)
        .await?;

    Ok(Json(json!({
        "total_users": total_users,
        "active_users": active_users,
        "active_sessions": active_sessions,
        "address_books": address_books,
        "current_user": admin.display_name,
        "version": env!("CARGO_PKG_VERSION")
    })))
}

async fn admin_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let admin = authenticated_admin(&state, &headers).await?;
    let now = unix_timestamp().map_err(AppError::Internal)?;
    let users = sqlx::query_as::<_, AdminUserRecord>(
        r#"
        SELECT
            u.id,
            u.username,
            u.display_name,
            u.is_admin,
            u.status,
            u.created_at,
            COALESCE(SUM(CASE WHEN s.expires_at > ? THEN 1 ELSE 0 END), 0) AS active_sessions
        FROM users u
        LEFT JOIN sessions s ON s.user_id = u.id
        GROUP BY u.id
        ORDER BY u.is_admin DESC, u.created_at ASC, u.username ASC
        "#,
    )
    .bind(now)
    .fetch_all(&state.pool)
    .await?;
    let payload: Vec<Value> = users
        .into_iter()
        .map(|user| {
            json!({
                "id": user.id,
                "username": user.username,
                "display_name": user.display_name,
                "is_admin": user.is_admin,
                "status": user.status,
                "created_at": user.created_at,
                "active_sessions": user.active_sessions,
                "is_self": user.id == admin.id,
                "is_root": user.username == state.root_username
            })
        })
        .collect();
    Ok(Json(json!({ "users": payload })))
}

async fn admin_create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    authenticated_admin(&state, &headers).await?;
    let username = validate_username(&input.username)?;
    let display_name = validate_display_name(&input.display_name)?;
    validate_new_password(&input.password)?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?)")
        .bind(&username)
        .fetch_one(&state.pool)
        .await?;
    if exists {
        return Err(AppError::Conflict("Ese nombre de usuario ya existe".to_owned()));
    }

    let password_hash = hash_password(&input.password)?;
    let result = sqlx::query(
        "INSERT INTO users (username, password_hash, display_name, is_admin, status) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(&display_name)
    .bind(input.is_admin)
    .execute(&state.pool)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": result.last_insert_rowid(),
            "username": username,
            "display_name": display_name,
            "is_admin": input.is_admin,
            "status": 1
        })),
    ))
}

async fn admin_set_user_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RoutePath(user_id): RoutePath<i64>,
    Json(input): Json<UserStatusRequest>,
) -> Result<Json<Value>, AppError> {
    let admin = authenticated_admin(&state, &headers).await?;
    protect_managed_account(&state, &admin, user_id).await?;
    let status = if input.active { 1 } else { 0 };
    let result = sqlx::query("UPDATE users SET status = ? WHERE id = ?")
        .bind(status)
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    if !input.active {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(json!({ "status": status })))
}

async fn admin_set_user_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RoutePath(user_id): RoutePath<i64>,
    Json(input): Json<UserPasswordRequest>,
) -> Result<Json<Value>, AppError> {
    let admin = authenticated_admin(&state, &headers).await?;
    protect_managed_account(&state, &admin, user_id).await?;
    validate_new_password(&input.password)?;
    let password_hash = hash_password(&input.password)?;
    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "updated": true })))
}

async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RoutePath(user_id): RoutePath<i64>,
) -> Result<Json<Value>, AppError> {
    let admin = authenticated_admin(&state, &headers).await?;
    protect_managed_account(&state, &admin, user_id).await?;
    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "deleted": true })))
}

async fn authenticated_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<UserRecord, AppError> {
    let user = authenticated_user(state, headers).await?;
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    Ok(user)
}

async fn protect_managed_account(
    state: &AppState,
    admin: &UserRecord,
    user_id: i64,
) -> Result<(), AppError> {
    if admin.id == user_id {
        return Err(AppError::BadRequest(
            "No puedes suspender, eliminar ni restablecer tu propia cuenta activa".to_owned(),
        ));
    }
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;
    let username = username.ok_or(AppError::NotFound)?;
    if username == state.root_username {
        return Err(AppError::BadRequest(
            "La cuenta raíz se administra desde la configuración segura del servidor".to_owned(),
        ));
    }
    Ok(())
}

fn validate_username(value: &str) -> Result<String, AppError> {
    let username = value.trim();
    if !(3..=32).contains(&username.len()) {
        return Err(AppError::BadRequest(
            "El usuario debe tener entre 3 y 32 caracteres".to_owned(),
        ));
    }
    if !username
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(AppError::BadRequest(
            "El usuario sólo puede contener letras, números, punto, guion y guion bajo"
                .to_owned(),
        ));
    }
    Ok(username.to_owned())
}

fn validate_display_name(value: &str) -> Result<String, AppError> {
    let display_name = value.trim();
    if display_name.is_empty() || display_name.chars().count() > 80 {
        return Err(AppError::BadRequest(
            "El nombre visible debe tener entre 1 y 80 caracteres".to_owned(),
        ));
    }
    Ok(display_name.to_owned())
}

fn validate_new_password(password: &str) -> Result<(), AppError> {
    if password.chars().count() < 8 || password.len() > 256 {
        return Err(AppError::BadRequest(
            "La contraseña debe tener entre 8 y 256 caracteres".to_owned(),
        ));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))
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
