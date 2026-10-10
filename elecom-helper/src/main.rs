mod keyboard;
#[cfg(target_os = "macos")]
mod macos;
mod model;

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use model::Config;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(not(target_os = "macos"))]
mod macos {
    use serde::Serialize;
    #[derive(Clone, Serialize)]
    pub struct DeviceStatus {}
    pub fn permissions() -> (bool, bool) {
        (false, false)
    }
    pub fn request_permissions() {}
    pub fn cancel_shortcuts() {}
    pub fn perform_action(_: &crate::model::Action) -> Result<(), String> {
        Err("macOS is required".into())
    }
}

struct Shared {
    config: Mutex<Config>,
    devices: Mutex<BTreeMap<usize, macos::DeviceStatus>>,
    error: Mutex<Option<String>>,
    path: PathBuf,
    ui_dir: Option<PathBuf>,
    quit: tokio::sync::Notify,
    identify_until: AtomicU64,
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
impl Shared {
    fn identifying(&self) -> bool {
        self.identify_until.load(Ordering::Relaxed) > now_ms()
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    version: &'static str,
    accessibility: bool,
    input_monitoring: bool,
    config: Config,
    devices: Vec<macos::DeviceStatus>,
    error: Option<String>,
    identifying: bool,
}

async fn status(State(shared): State<Arc<Shared>>) -> Result<Json<Status>, StatusCode> {
    let (accessibility, input_monitoring) = macos::permissions();
    Ok(Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        identifying: shared.identifying(),
        accessibility,
        input_monitoring,
        config: shared
            .config
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .clone(),
        devices: shared
            .devices
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .values()
            .cloned()
            .collect(),
        error: shared
            .error
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .clone(),
    }))
}
async fn save(
    State(shared): State<Arc<Shared>>,
    Json(config): Json<Config>,
) -> Result<Json<Config>, (StatusCode, String)> {
    config
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, e.into()))?;
    // Serialise save operations, persist before applying, and never write
    // partial JSON to the file that will be read on the next launch.
    let mut active = shared.config.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration lock failed".into(),
        )
    })?;
    let persist = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = shared.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = shared.path.with_extension("pending");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&config)?)?;
        std::fs::rename(temporary, &shared.path)?;
        Ok(())
    };
    persist().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    macos::cancel_shortcuts();
    *active = config.clone();
    Ok(Json(config))
}
async fn permissions() -> Json<serde_json::Value> {
    macos::request_permissions();
    Json(serde_json::json!({"ok":true}))
}
async fn quit(State(shared): State<Arc<Shared>>) -> Json<serde_json::Value> {
    shared.identify_until.store(u64::MAX, Ordering::Relaxed);
    macos::cancel_shortcuts();
    shared.quit.notify_one();
    Json(serde_json::json!({"ok":true}))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentifyRequest {
    active: bool,
}
async fn identify(
    State(shared): State<Arc<Shared>>,
    Json(request): Json<IdentifyRequest>,
) -> Json<serde_json::Value> {
    shared.identify_until.store(
        model::identify_until(request.active, now_ms()),
        Ordering::Relaxed,
    );
    if request.active {
        macos::cancel_shortcuts();
    }
    Json(serde_json::json!({"identifying": shared.identifying()}))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TestRequest {
    family: String,
    button: u8,
}
async fn test_binding(
    State(shared): State<Arc<Shared>>,
    Json(request): Json<TestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !matches!(request.family.as_str(), "mouse" | "trackball")
        || !(6..=8).contains(&request.button)
    {
        return Err((StatusCode::BAD_REQUEST, "Unsupported button".into()));
    }
    if shared.identifying() {
        return Err((
            StatusCode::CONFLICT,
            "ボタン確認を終了してから試してください。".into(),
        ));
    }
    let action = {
        let config = shared.config.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Configuration lock failed".into(),
            )
        })?;
        if !config.enabled {
            return Err((
                StatusCode::CONFLICT,
                "割り当てを有効にして保存してください。".into(),
            ));
        }
        config
            .mappings
            .get(&request.family)
            .and_then(|bindings| bindings.get(&request.button))
            .cloned()
            .ok_or((
                StatusCode::CONFLICT,
                "このボタンに操作を割り当てて保存してください。".into(),
            ))?
    };
    macos::perform_action(&action).map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

// Exact loopback Host + Origin checks apply to every endpoint, including
// state-changing requests. CORS alone is not authorisation or CSRF protection.
async fn local_only(request: Request, next: Next) -> Response {
    if !valid_headers(request.headers(), request.method()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            response
                .headers_mut()
                .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                "GET, PUT, OPTIONS".parse().unwrap(),
            );
            response.headers_mut().insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                "Content-Type".parse().unwrap(),
            );
        }
        return response;
    }
    let origin = request.headers().get(header::ORIGIN).cloned();
    let mut response = next.run(request).await;
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
fn valid_headers(headers: &HeaderMap, method: &Method) -> bool {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !matches!(host, Some("127.0.0.1:17847" | "localhost:17847")) {
        return false;
    }
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let allowed = matches!(
        origin,
        Some("http://127.0.0.1:17847" | "http://localhost:17847")
    );
    if origin.is_some() {
        return allowed;
    }
    if *method != Method::GET && *method != Method::HEAD {
        return false;
    }
    headers
        .get("sec-fetch-site")
        .is_none_or(|v| v == "same-origin" || v == "none")
}
async fn static_ui(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    if request.method() != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(root) = &shared.ui_dir else {
        return (
            StatusCode::NOT_FOUND,
            "Build OpenMouse's offline EX-G PRO UI and pass --ui-dir",
        )
            .into_response();
    };
    let name = match request.uri().path() {
        "/" => "elecom.html",
        path => path.trim_start_matches('/'),
    };
    if name
        .split('/')
        .any(|part| part == ".." || part.contains('\\'))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(path) = root.join(name).canonicalize() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !path.starts_with(root) || !path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let content_type = match path.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut response =
                ([(header::CONTENT_TYPE, content_type)], Body::from(bytes)).into_response();
            response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; connect-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'".parse().unwrap());
            response
                .headers_mut()
                .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/v1/elecom", get(status).put(save))
        .route("/v1/elecom/permissions", put(permissions))
        .route("/v1/elecom/quit", put(quit))
        .route("/v1/elecom/identify", put(identify))
        .route("/v1/elecom/test", put(test_binding))
        .fallback(static_ui)
        .layer(middleware::from_fn(local_only))
        .with_state(shared)
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut ui_dir = None;
    let mut safe_start = false;
    // A Finder-launched bundle uses its own offline resources. CLI use remains
    // explicit and never opens a browser without a bundled UI.
    let bundled_ui = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.parent().map(|p| p.join("Resources/ui")))
        .filter(|p| p.join("elecom.html").is_file());
    let open_browser = bundled_ui.is_some();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--safe-start" => safe_start = true,
            "--config" => {
                path = Some(PathBuf::from(
                    args.next().ok_or("--config requires a path")?,
                ))
            }
            "--ui-dir" => {
                ui_dir = Some(
                    PathBuf::from(args.next().ok_or("--ui-dir requires a path")?).canonicalize()?,
                )
            }
            "--help" => {
                println!(
                    "openmouse-elecom [--config PATH] [--ui-dir OpenMouse/dist-elecom] [--safe-start]\n--safe-start: load saved mappings with actions disabled until saved from the UI.\nLocal UI: http://127.0.0.1:17847\nNo updates, telemetry, game monitoring or outbound network requests."
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    let path = path.unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Application Support/OpenMouse/elecom.json")
    });
    let mut config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Config>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e.into()),
    };
    config.validate()?;
    if safe_start {
        config.enabled = false;
    }
    let ui_dir = ui_dir
        .or(bundled_ui)
        .map(|path| path.canonicalize())
        .transpose()?;
    let shared = Arc::new(Shared {
        config: Mutex::new(config),
        devices: Mutex::new(BTreeMap::new()),
        error: Mutex::new(None),
        path,
        ui_dir,
        quit: tokio::sync::Notify::new(),
        identify_until: AtomicU64::new(0),
    });
    // Bind first so two copies never install duplicate input listeners.
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:17847").await {
        Ok(listener) => listener,
        Err(error) => {
            if open_browser && error.kind() == std::io::ErrorKind::AddrInUse {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("http://127.0.0.1:17847")
                    .spawn();
                return Ok(());
            }
            return Err(error.into());
        }
    };
    #[cfg(target_os = "macos")]
    let _native = macos::Listener::start(shared.clone());
    #[cfg(not(target_os = "macos"))]
    return Err("The EX-G PRO input listener requires macOS".into());
    #[cfg(target_os = "macos")]
    {
        println!("OpenMouse EX-G PRO: http://127.0.0.1:17847 (local only)");
        if open_browser {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("http://127.0.0.1:17847")
                .spawn();
        }
        let shutdown = shared.clone();
        axum::serve(listener, router(shared)).with_graceful_shutdown(async move {
            let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {}, _ = shutdown.quit.notified() => {} }
        }).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[test]
    fn rejects_dns_rebinding_and_cross_origin_mutations() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "127.0.0.1:17847".parse().unwrap());
        assert!(valid_headers(&h, &Method::GET));
        assert!(!valid_headers(&h, &Method::PUT));
        h.insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
        assert!(!valid_headers(&h, &Method::PUT));
        h.insert(header::ORIGIN, "http://127.0.0.1:17847".parse().unwrap());
        assert!(valid_headers(&h, &Method::PUT));
        h.insert(header::HOST, "rebound.example:17847".parse().unwrap());
        assert!(!valid_headers(&h, &Method::GET));
    }
    #[tokio::test]
    async fn identify_mode_expires_and_never_changes_persisted_bindings() {
        let shared = Arc::new(Shared {
            config: Mutex::new(Config::default()),
            devices: Mutex::new(BTreeMap::new()),
            error: Mutex::new(None),
            path: std::env::temp_dir().join("openmouse-identify-no-write.json"),
            ui_dir: None,
            quit: tokio::sync::Notify::new(),
            identify_until: AtomicU64::new(0),
        });
        for active in [true, false] {
            let request = Request::builder()
                .method("PUT")
                .uri("/v1/elecom/identify")
                .header("host", "127.0.0.1:17847")
                .header("origin", "http://127.0.0.1:17847")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"active":{active}}}"#)))
                .unwrap();
            assert_eq!(
                router(shared.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
            assert_eq!(shared.identifying(), active);
            assert_eq!(*shared.config.lock().unwrap(), Config::default());
        }
        shared
            .identify_until
            .store(now_ms().saturating_sub(1), Ordering::Relaxed);
        assert!(!shared.identifying());
    }
    #[tokio::test]
    async fn invalid_mapping_never_updates_active_or_saved_config() {
        let path = std::env::temp_dir().join(format!(
            "openmouse-elecom-invalid-{}.json",
            std::process::id()
        ));
        let shared = Arc::new(Shared {
            config: Mutex::new(Config::default()),
            devices: Mutex::new(BTreeMap::new()),
            error: Mutex::new(None),
            path: path.clone(),
            ui_dir: None,
            quit: tokio::sync::Notify::new(),
            identify_until: AtomicU64::new(0),
        });
        let request = Request::builder().method("PUT").uri("/v1/elecom")
            .header("host", "127.0.0.1:17847").header("origin", "http://127.0.0.1:17847").header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true,"mappings":{"mouse":{"1":{"keyCode":8,"modifiers":{"command":true,"control":false,"option":false,"shift":false}}}}}"#)).unwrap();
        let response = router(shared.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(*shared.config.lock().unwrap(), Config::default());
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn action_test_rejects_unmapped_disabled_and_untrusted_requests_without_posting() {
        let shared = Arc::new(Shared {
            config: Mutex::new(Config::default()),
            devices: Mutex::new(BTreeMap::new()),
            error: Mutex::new(None),
            path: std::env::temp_dir().join("openmouse-test-no-write.json"),
            ui_dir: None,
            quit: tokio::sync::Notify::new(),
            identify_until: AtomicU64::new(0),
        });
        for (body, origin, expected) in [
            (
                r#"{"family":"mouse","button":7}"#,
                "https://attacker.example",
                StatusCode::FORBIDDEN,
            ),
            (
                r#"{"family":"other","button":7}"#,
                "http://127.0.0.1:17847",
                StatusCode::BAD_REQUEST,
            ),
            (
                r#"{"family":"mouse","button":1}"#,
                "http://127.0.0.1:17847",
                StatusCode::BAD_REQUEST,
            ),
            (
                r#"{"family":"mouse","button":7,"systemAction":"missionControl"}"#,
                "http://127.0.0.1:17847",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                r#"{"family":"mouse","button":7}"#,
                "http://127.0.0.1:17847",
                StatusCode::CONFLICT,
            ),
        ] {
            let request = Request::builder()
                .method("PUT")
                .uri("/v1/elecom/test")
                .header("host", "127.0.0.1:17847")
                .header("origin", origin)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            assert_eq!(
                router(shared.clone())
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
        shared.config.lock().unwrap().enabled = true;
        let request = || {
            Request::builder()
                .method("PUT")
                .uri("/v1/elecom/test")
                .header("host", "127.0.0.1:17847")
                .header("origin", "http://127.0.0.1:17847")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"family":"mouse","button":7}"#))
                .unwrap()
        };
        assert_eq!(
            router(shared.clone())
                .oneshot(request())
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        shared
            .identify_until
            .store(now_ms() + 30_000, Ordering::Relaxed);
        assert_eq!(
            router(shared.clone())
                .oneshot(request())
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert!(shared.config.lock().unwrap().mappings.is_empty());
        assert!(shared.error.lock().unwrap().is_none());
    }
}
