//! Opt-in, loopback-only API for local debugging and AI tool integrations.
//!
//! The API deliberately exposes only terminal metadata, a bounded screen
//! snapshot, and terminal input. Saved sessions and credentials are never part
//! of this state. Every route requires a bearer token, including `/v1/health`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use anyhow::{bail, Context, Result};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use zeroize::Zeroize;

use crate::ssh::SessionCommand;

/// Stable default port shown in settings. Passing port 0 to [`DebugApiController::start`]
/// asks the OS for an ephemeral port, which is useful in tests.
pub const DEFAULT_PORT: u16 = 24817;
pub const DEFAULT_MAX_SCREEN_LINES: usize = 200;
pub const MAX_SCREEN_LINES: usize = 1000;

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const MAX_INPUT_BYTES: usize = 16 * 1024;
const MAX_OUTSTANDING_INPUTS: usize = 4;
const INPUT_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const SCREENSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const MAX_SCREENSHOT_EDGE: u32 = 4096;
const MAX_SCREENSHOT_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_POINTER_COORDINATE: i32 = 4095;
// JSON escaping can expand a control byte to six ASCII bytes. Keeping raw text
// at 128 KiB guarantees the encoded screen response remains below 1 MiB.
const MAX_SCREEN_TEXT_BYTES: usize = 128 * 1024;
const MIN_TOKEN_BYTES: usize = 24;
const MAX_TOKEN_BYTES: usize = 256;

/// Public, non-secret information about one open terminal.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TerminalMetadata {
    pub id: String,
    pub title: String,
    pub host: String,
    /// Application-defined state, normally `connecting`, `connected`, or
    /// `disconnected`.
    pub state: String,
}

impl TerminalMetadata {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        host: impl Into<String>,
        state: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            host: host.into(),
            state: state.into(),
        }
    }
}

type ScreenReader = Arc<dyn Fn(&str, usize) -> Option<Vec<String>> + Send + Sync + 'static>;
type PointerEncoder =
    Arc<dyn Fn(&str, DebugPointerEvent) -> Option<Vec<u8>> + Send + Sync + 'static>;
type ScreenshotRequester = Arc<
    dyn Fn(oneshot::Sender<Result<ScreenshotFrame, String>>) -> Result<(), String>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DebugPointerEventKind {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DebugPointerEvent {
    pub(crate) kind: DebugPointerEventKind,
    pub(crate) button: u16,
    pub(crate) col: i32,
    pub(crate) row: i32,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
}

/// A renderer snapshot copied into an owned RGBA buffer before it leaves the
/// Slint event loop. PNG encoding and optional downscaling happen off-thread.
pub(crate) struct ScreenshotFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

#[derive(Clone)]
struct InputTarget {
    sender: UnboundedSender<SessionCommand>,
    permits: Arc<Semaphore>,
}

struct DebugApiStateInner {
    terminals: Mutex<HashMap<String, TerminalMetadata>>,
    input_senders: Mutex<HashMap<String, InputTarget>>,
    screen_reader: ScreenReader,
    pointer_encoder: RwLock<Option<PointerEncoder>>,
    screenshot_requester: RwLock<Option<ScreenshotRequester>>,
    screenshot_permits: Arc<Semaphore>,
}

/// Thread-safe state shared by the UI/session layer and the HTTP handlers.
///
/// The screen callback receives a terminal id and a maximum number of lines.
/// It should return the newest lines in display order. Handlers apply their own
/// line and byte caps as a second line of defence.
#[derive(Clone)]
pub struct DebugApiState {
    inner: Arc<DebugApiStateInner>,
}

impl DebugApiState {
    pub fn new<F>(screen_reader: F) -> Self
    where
        F: Fn(&str, usize) -> Option<Vec<String>> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(DebugApiStateInner {
                terminals: Mutex::new(HashMap::new()),
                input_senders: Mutex::new(HashMap::new()),
                screen_reader: Arc::new(screen_reader),
                pointer_encoder: RwLock::new(None),
                screenshot_requester: RwLock::new(None),
                screenshot_permits: Arc::new(Semaphore::new(1)),
            }),
        }
    }

    /// Register the terminal-state bridge used by pointer injection. The
    /// callback returns `None` while the target application has mouse tracking
    /// disabled, so API pointer requests can never become shell text.
    pub(crate) fn set_pointer_encoder<F>(&self, encoder: F)
    where
        F: Fn(&str, DebugPointerEvent) -> Option<Vec<u8>> + Send + Sync + 'static,
    {
        *self
            .inner
            .pointer_encoder
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(encoder));
    }

    /// Register the UI-thread bridge used by `GET /v1/screenshot`.
    pub(crate) fn set_screenshot_requester<F>(&self, requester: F)
    where
        F: Fn(oneshot::Sender<Result<ScreenshotFrame, String>>) -> Result<(), String>
            + Send
            + Sync
            + 'static,
    {
        *self
            .inner
            .screenshot_requester
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(requester));
    }

    pub fn upsert_terminal(&self, terminal: TerminalMetadata) {
        if terminal.id.is_empty() {
            return;
        }
        lock(&self.inner.terminals).insert(terminal.id.clone(), terminal);
    }

    /// Mutate an existing terminal entry. Returns false when the id is unknown.
    pub fn update_terminal(&self, id: &str, update: impl FnOnce(&mut TerminalMetadata)) -> bool {
        let mut terminals = lock(&self.inner.terminals);
        let Some(terminal) = terminals.get_mut(id) else {
            return false;
        };
        update(terminal);
        // Keep the map key authoritative if a caller accidentally changes `id`.
        terminal.id.clear();
        terminal.id.push_str(id);
        true
    }

    pub fn set_input_sender(&self, id: impl Into<String>, sender: UnboundedSender<SessionCommand>) {
        let id = id.into();
        if !id.is_empty() {
            lock(&self.inner.input_senders).insert(
                id,
                InputTarget {
                    sender,
                    permits: Arc::new(Semaphore::new(MAX_OUTSTANDING_INPUTS)),
                },
            );
        }
    }

    /// Remove only the live input channel, retaining metadata/screen access for
    /// a disconnected terminal that remains visible in the UI.
    pub fn remove_input_sender(&self, id: &str) {
        lock(&self.inner.input_senders).remove(id);
    }

    /// Remove all API-visible state for a closed tab.
    pub fn remove_terminal(&self, id: &str) {
        lock(&self.inner.terminals).remove(id);
        self.remove_input_sender(id);
    }

    pub fn terminals(&self) -> Vec<TerminalMetadata> {
        let mut terminals: Vec<_> = lock(&self.inner.terminals).values().cloned().collect();
        terminals.sort_by(|a, b| a.id.cmp(&b.id));
        terminals
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone)]
struct ApiContext {
    state: DebugApiState,
}

#[derive(Clone)]
struct AuthState {
    token: Arc<RwLock<AuthToken>>,
}

struct AuthToken(Vec<u8>);

impl Drop for AuthToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

struct RunningServer {
    address: SocketAddr,
    auth: AuthState,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// Owns the listener task. This type is intended to live on the Slint/UI thread;
/// request handling itself runs on the application's shared Tokio runtime.
pub struct DebugApiController {
    runtime: Handle,
    state: DebugApiState,
    running: Option<RunningServer>,
}

impl DebugApiController {
    pub fn new(runtime: Handle, state: DebugApiState) -> Self {
        Self {
            runtime,
            state,
            running: None,
        }
    }

    /// Start the API on `127.0.0.1:port`.
    ///
    /// Calling this again on the same port updates the token without dropping
    /// the listener. A different port replaces the existing listener.
    pub fn start(&mut self, port: u16, token: impl AsRef<str>) -> Result<SocketAddr> {
        validate_token(token.as_ref())?;

        if let Some(running) = &self.running {
            if !running.task.is_finished() && port != 0 && running.address.port() == port {
                set_auth_token(&running.auth, token.as_ref());
                return Ok(running.address);
            }
        }

        self.stop();

        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .with_context(|| format!("bind Debug API to 127.0.0.1:{port}"))?;
        listener
            .set_nonblocking(true)
            .context("set Debug API listener non-blocking")?;
        let address = listener
            .local_addr()
            .context("read Debug API listener address")?;

        let listener = {
            let _runtime_guard = self.runtime.enter();
            tokio::net::TcpListener::from_std(listener)
                .context("attach Debug API listener to Tokio")?
        };
        let auth = AuthState {
            token: Arc::new(RwLock::new(AuthToken(token.as_ref().as_bytes().to_vec()))),
        };
        let app = build_router(self.state.clone(), auth.clone());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = self.runtime.spawn(async move {
            let server =
                axum::serve(listener, app.into_make_service()).with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                });
            if let Err(error) = server.await {
                tracing::warn!("Debug API server stopped with an error: {error}");
            }
        });

        self.running = Some(RunningServer {
            address,
            auth,
            shutdown: shutdown_tx,
            task,
        });
        Ok(address)
    }

    pub fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = running.shutdown.send(());
            // The shutdown signal normally completes the task immediately. Abort
            // as well so dropping/toggling the controller cannot leave a listener
            // alive behind a stalled connection.
            running.task.abort();
        }
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| !running.task.is_finished())
    }
}

impl Drop for DebugApiController {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Generate a 256-bit URL-safe bearer token without padding.
pub fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    bytes.zeroize();
    token
}

fn validate_token(token: &str) -> Result<()> {
    let len = token.len();
    if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&len) {
        bail!("Debug API token must be {MIN_TOKEN_BYTES}..={MAX_TOKEN_BYTES} bytes");
    }
    if token
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        bail!("Debug API token must not contain whitespace or control characters");
    }
    Ok(())
}

fn set_auth_token(auth: &AuthState, token: &str) {
    let mut current = auth
        .token
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    current.0.zeroize();
    current.0.extend_from_slice(token.as_bytes());
}

fn build_router(state: DebugApiState, auth: AuthState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/screenshot", get(screenshot))
        .route("/v1/terminals", get(terminals))
        .route("/v1/terminals/:id/screen", get(screen))
        .route("/v1/terminals/:id/input", post(input))
        .route("/v1/terminals/:id/pointer", post(pointer))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(auth, authorize))
        .with_state(ApiContext { state })
}

async fn authorize(State(auth): State<AuthState>, request: Request, next: Next) -> Response {
    let supplied = bearer_token(request.headers());
    let accepted = supplied.is_some_and(|token| {
        let expected = auth
            .token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        constant_time_eq(token.as_bytes(), &expected.0)
    });

    if accepted {
        let mut response = next.run(request).await;
        mark_sensitive(&mut response);
        response
    } else {
        let mut response = api_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Bearer"),
        );
        mark_sensitive(&mut response);
        response
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && !token.contains(' '))
        .then_some(token)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut different = left.len() ^ right.len();
    for index in 0..max_len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        different |= usize::from(l ^ r);
    }
    different == 0
}

fn mark_sensitive(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct TerminalsResponse {
    terminals: Vec<TerminalMetadata>,
}

async fn terminals(State(context): State<ApiContext>) -> Json<TerminalsResponse> {
    Json(TerminalsResponse {
        terminals: context.state.terminals(),
    })
}

#[derive(Default, Deserialize)]
struct ScreenshotQuery {
    max_width: Option<u32>,
    max_height: Option<u32>,
}

async fn screenshot(
    State(context): State<ApiContext>,
    Query(query): Query<ScreenshotQuery>,
) -> Response {
    if query.max_width == Some(0) || query.max_height == Some(0) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_dimensions",
            "screenshot dimensions must be greater than zero",
        );
    }
    let max_width = query
        .max_width
        .unwrap_or(MAX_SCREENSHOT_EDGE)
        .min(MAX_SCREENSHOT_EDGE);
    let max_height = query
        .max_height
        .unwrap_or(MAX_SCREENSHOT_EDGE)
        .min(MAX_SCREENSHOT_EDGE);

    let requester = context
        .state
        .inner
        .screenshot_requester
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let Some(requester) = requester else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "screenshot_unavailable",
            "window screenshot capture is unavailable",
        );
    };
    let Ok(_permit) = context
        .state
        .inner
        .screenshot_permits
        .clone()
        .try_acquire_owned()
    else {
        return api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "screenshot_busy",
            "another screenshot is still being captured",
        );
    };

    let started = std::time::Instant::now();
    let (reply_tx, reply_rx) = oneshot::channel();
    if let Err(error) = requester(reply_tx) {
        tracing::warn!("failed to schedule Debug API screenshot: {error}");
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "screenshot_unavailable",
            "window screenshot capture is unavailable",
        );
    }
    let frame = match tokio::time::timeout(SCREENSHOT_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(frame))) => frame,
        Ok(Ok(Err(error))) => {
            tracing::warn!("Debug API screenshot capture failed: {error}");
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "screenshot_failed",
                "window screenshot capture failed",
            );
        }
        Ok(Err(_)) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "screenshot_unavailable",
                "window screenshot capture is unavailable",
            );
        }
        Err(_) => {
            return api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "screenshot_timeout",
                "window screenshot capture timed out",
            );
        }
    };

    let Some(remaining) = SCREENSHOT_TIMEOUT.checked_sub(started.elapsed()) else {
        return api_error(
            StatusCode::GATEWAY_TIMEOUT,
            "screenshot_timeout",
            "window screenshot capture timed out",
        );
    };
    let encoded =
        tokio::task::spawn_blocking(move || encode_screenshot(frame, max_width, max_height));
    let png = match tokio::time::timeout(remaining, encoded).await {
        Ok(Ok(Ok(png))) => png,
        Ok(Ok(Err(error))) => {
            tracing::warn!("Debug API screenshot encoding failed: {error}");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "screenshot_failed",
                "window screenshot encoding failed",
            );
        }
        Ok(Err(error)) => {
            tracing::warn!("Debug API screenshot worker failed: {error}");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "screenshot_failed",
                "window screenshot encoding failed",
            );
        }
        Err(_) => {
            return api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "screenshot_timeout",
                "window screenshot encoding timed out",
            );
        }
    };

    ([(header::CONTENT_TYPE, "image/png")], png).into_response()
}

fn encode_screenshot(frame: ScreenshotFrame, max_width: u32, max_height: u32) -> Result<Vec<u8>> {
    let expected_len = u64::from(frame.width)
        .checked_mul(u64::from(frame.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .context("screenshot dimensions overflow")?;
    if frame.width == 0
        || frame.height == 0
        || expected_len != u64::try_from(frame.rgba.len()).unwrap_or(u64::MAX)
    {
        bail!("invalid RGBA screenshot buffer");
    }

    let pixel_scale =
        (MAX_SCREENSHOT_PIXELS as f64 / (f64::from(frame.width) * f64::from(frame.height))).sqrt();
    let scale = 1.0_f64
        .min(f64::from(max_width) / f64::from(frame.width))
        .min(f64::from(max_height) / f64::from(frame.height))
        .min(pixel_scale);
    let output_width = (f64::from(frame.width) * scale).floor().max(1.0) as u32;
    let output_height = (f64::from(frame.height) * scale).floor().max(1.0) as u32;

    let pixels = if output_width == frame.width && output_height == frame.height {
        frame.rgba
    } else {
        let source = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            .context("invalid RGBA screenshot buffer")?;
        image::imageops::resize(
            &source,
            output_width,
            output_height,
            image::imageops::FilterType::Triangle,
        )
        .into_raw()
    };

    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &pixels,
        output_width,
        output_height,
        image::ExtendedColorType::Rgba8,
    )
    .context("encode screenshot as PNG")?;
    Ok(png)
}

#[derive(Default, Deserialize)]
struct ScreenQuery {
    max_lines: Option<usize>,
}

#[derive(Serialize)]
struct ScreenResponse {
    terminal_id: String,
    line_count: usize,
    truncated: bool,
    text: String,
}

async fn screen(
    State(context): State<ApiContext>,
    Path(id): Path<String>,
    Query(query): Query<ScreenQuery>,
) -> Response {
    if !lock(&context.state.inner.terminals).contains_key(&id) {
        return api_error(StatusCode::NOT_FOUND, "not_found", "terminal not found");
    }

    let max_lines = query
        .max_lines
        .unwrap_or(DEFAULT_MAX_SCREEN_LINES)
        .clamp(1, MAX_SCREEN_LINES);
    let provider_limit = max_lines.saturating_add(1);
    let Some(lines) = (context.state.inner.screen_reader)(&id, provider_limit) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "screen_unavailable",
            "terminal screen is unavailable",
        );
    };
    let (text, line_count, truncated) = cap_screen(lines, max_lines);
    Json(ScreenResponse {
        terminal_id: id,
        line_count,
        truncated,
        text,
    })
    .into_response()
}

#[derive(Deserialize)]
struct InputRequest {
    text: String,
    #[serde(default)]
    submit: bool,
}

#[derive(Serialize)]
struct InputResponse {
    accepted: bool,
    bytes: usize,
}

fn terminal_input_target(state: &DebugApiState, id: &str) -> Option<InputTarget> {
    lock(&state.inner.input_senders).get(id).cloned()
}

fn terminal_unavailable(state: &DebugApiState, id: &str) -> Response {
    let status = if lock(&state.inner.terminals).contains_key(id) {
        StatusCode::CONFLICT
    } else {
        StatusCode::NOT_FOUND
    };
    api_error(status, "terminal_unavailable", "terminal is not connected")
}

async fn dispatch_debug_bytes(
    state: &DebugApiState,
    id: &str,
    target: InputTarget,
    bytes: Vec<u8>,
) -> std::result::Result<usize, Response> {
    let Ok(_permit) = target.permits.clone().try_acquire_owned() else {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "input_busy",
            "too many terminal inputs are still pending",
        ));
    };

    let byte_count = bytes.len();
    let (ack_tx, ack_rx) = oneshot::channel();
    if target
        .sender
        .send(SessionCommand::DebugInput { bytes, ack: ack_tx })
        .is_err()
    {
        state.remove_input_sender(id);
        return Err(api_error(
            StatusCode::CONFLICT,
            "terminal_unavailable",
            "terminal input channel is closed",
        ));
    }

    match tokio::time::timeout(INPUT_ACK_TIMEOUT, ack_rx).await {
        Ok(Ok(Ok(()))) => Ok(byte_count),
        Ok(Ok(Err(_))) | Ok(Err(_)) => {
            state.remove_input_sender(id);
            Err(api_error(
                StatusCode::CONFLICT,
                "terminal_unavailable",
                "terminal failed to accept input",
            ))
        }
        Err(_) => {
            state.remove_input_sender(id);
            Err(api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "input_timeout",
                "terminal did not acknowledge input in time",
            ))
        }
    }
}

async fn input(
    State(context): State<ApiContext>,
    Path(id): Path<String>,
    Json(request): Json<InputRequest>,
) -> Response {
    let extra = usize::from(request.submit);
    if request.text.len().saturating_add(extra) > MAX_INPUT_BYTES {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "input_too_large",
            "terminal input exceeds the 16 KiB limit",
        );
    }

    let Some(target) = terminal_input_target(&context.state, &id) else {
        return terminal_unavailable(&context.state, &id);
    };

    let mut bytes = request.text.into_bytes();
    if request.submit {
        bytes.push(b'\r');
    }
    match dispatch_debug_bytes(&context.state, &id, target, bytes).await {
        Ok(byte_count) => Json(InputResponse {
            accepted: true,
            bytes: byte_count,
        })
        .into_response(),
        Err(response) => response,
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerRequestKind {
    Click,
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerButton {
    Left,
    Middle,
    Right,
}

impl PointerButton {
    fn protocol_button(self) -> u16 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

#[derive(Deserialize)]
struct PointerRequest {
    kind: PointerRequestKind,
    button: PointerButton,
    col: i32,
    row: i32,
    clicks: Option<u8>,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    alt: bool,
}

#[derive(Serialize)]
struct PointerResponse {
    accepted: bool,
    events: usize,
    bytes: usize,
}

async fn pointer(
    State(context): State<ApiContext>,
    Path(id): Path<String>,
    Json(request): Json<PointerRequest>,
) -> Response {
    if !(0..=MAX_POINTER_COORDINATE).contains(&request.col)
        || !(0..=MAX_POINTER_COORDINATE).contains(&request.row)
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_pointer",
            "pointer coordinates must be between 0 and 4095",
        );
    }

    let clicks = request.clicks.unwrap_or(1);
    if matches!(request.kind, PointerRequestKind::Click) {
        if !(1..=2).contains(&clicks) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_pointer",
                "clicks must be 1 or 2",
            );
        }
    } else if request.clicks.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_pointer",
            "clicks is only valid for click events",
        );
    }

    let Some(target) = terminal_input_target(&context.state, &id) else {
        return terminal_unavailable(&context.state, &id);
    };
    let encoder = context
        .state
        .inner
        .pointer_encoder
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let Some(encoder) = encoder else {
        return api_error(
            StatusCode::CONFLICT,
            "pointer_unavailable",
            "terminal pointer injection is unavailable",
        );
    };

    let event = |kind| DebugPointerEvent {
        kind,
        button: request.button.protocol_button(),
        col: request.col,
        row: request.row,
        ctrl: request.ctrl,
        alt: request.alt,
    };
    let events = match request.kind {
        PointerRequestKind::Click => {
            let mut events = Vec::with_capacity(usize::from(clicks) * 2);
            for _ in 0..clicks {
                events.push(event(DebugPointerEventKind::Press));
                events.push(event(DebugPointerEventKind::Release));
            }
            events
        }
        PointerRequestKind::Press => vec![event(DebugPointerEventKind::Press)],
        PointerRequestKind::Release => vec![event(DebugPointerEventKind::Release)],
        PointerRequestKind::Motion => vec![event(DebugPointerEventKind::Motion)],
    };

    let mut bytes = Vec::with_capacity(events.len() * 16);
    for event in &events {
        let Some(encoded) = encoder(&id, *event) else {
            return api_error(
                StatusCode::CONFLICT,
                "mouse_tracking_disabled",
                "terminal application has not enabled mouse tracking",
            );
        };
        bytes.extend_from_slice(&encoded);
    }

    let byte_count = if bytes.is_empty() {
        0
    } else {
        match dispatch_debug_bytes(&context.state, &id, target, bytes).await {
            Ok(byte_count) => byte_count,
            Err(response) => return response,
        }
    };
    Json(PointerResponse {
        accepted: true,
        events: events.len(),
        bytes: byte_count,
    })
    .into_response()
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: &'static str,
}

fn api_error(status: StatusCode, error: &'static str, message: &'static str) -> Response {
    (status, Json(ErrorBody { error, message })).into_response()
}

/// Keep the newest lines and, if necessary, the newest UTF-8 text within the
/// response byte cap. Keeping the tail matches what an agent needs after running
/// a command: the latest prompt/output rather than stale scrollback.
fn cap_screen(lines: Vec<String>, max_lines: usize) -> (String, usize, bool) {
    let original_line_count = lines.len();
    let skip = original_line_count.saturating_sub(max_lines);
    let selected: Vec<_> = lines.into_iter().skip(skip).collect();
    let mut truncated = skip > 0;
    let mut remaining = MAX_SCREEN_TEXT_BYTES;
    let mut kept_reversed = Vec::new();

    for line in selected.into_iter().rev() {
        let separator_bytes = usize::from(!kept_reversed.is_empty());
        if remaining <= separator_bytes {
            truncated = true;
            break;
        }
        remaining -= separator_bytes;

        if line.len() <= remaining {
            remaining -= line.len();
            kept_reversed.push(line);
            continue;
        }

        let mut start = line.len().saturating_sub(remaining);
        while start < line.len() && !line.is_char_boundary(start) {
            start += 1;
        }
        kept_reversed.push(line[start..].to_string());
        truncated = true;
        break;
    }

    kept_reversed.reverse();
    let line_count = kept_reversed.len();
    (kept_reversed.join("\n"), line_count, truncated)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::time::Duration;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const ROTATED_TOKEN: &str = "fedcba9876543210fedcba9876543210";

    fn request_bytes(address: SocketAddr, request: &str) -> Vec<u8> {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    fn request(address: SocketAddr, request: &str) -> String {
        String::from_utf8(request_bytes(address, request)).unwrap()
    }

    #[test]
    fn generated_token_is_256_bit_url_safe_value() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 43);
        assert!(!first.contains('='));
        assert_ne!(first, second);
        assert!(validate_token(&first).is_ok());
    }

    #[test]
    fn screen_cap_keeps_the_latest_lines() {
        let lines = vec!["one".into(), "two".into(), "three".into()];
        let (text, count, truncated) = cap_screen(lines, 2);
        assert_eq!(text, "two\nthree");
        assert_eq!(count, 2);
        assert!(truncated);
    }

    #[test]
    fn screenshot_encoder_preserves_aspect_ratio_and_caps_dimensions() {
        let frame = ScreenshotFrame {
            width: 4,
            height: 2,
            rgba: vec![255; 4 * 2 * 4],
        };
        let png = encode_screenshot(frame, 2, 2).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
    }

    #[test]
    fn input_target_caps_outstanding_requests() {
        let state = DebugApiState::new(|_, _| None);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        state.set_input_sender("term-1", tx);
        let target = lock(&state.inner.input_senders)
            .get("term-1")
            .cloned()
            .unwrap();
        let permits: Vec<_> = (0..MAX_OUTSTANDING_INPUTS)
            .map(|_| target.permits.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(target.permits.clone().try_acquire_owned().is_err());
        drop(permits);
        assert!(target.permits.clone().try_acquire_owned().is_ok());
    }

    #[test]
    fn live_server_authenticates_and_routes_terminal_io() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let state = DebugApiState::new(|id, _max_lines| {
            (id == "term-1").then(|| vec!["old".into(), "new".into()])
        });
        state.set_pointer_encoder(|id, event| {
            if id != "term-1" {
                return None;
            }
            let kind = match event.kind {
                DebugPointerEventKind::Press => 'P',
                DebugPointerEventKind::Release => 'R',
                DebugPointerEventKind::Motion => 'M',
            };
            Some(format!("{kind}{}:{};", event.col, event.row).into_bytes())
        });
        state.set_screenshot_requester(|reply| {
            reply
                .send(Ok(ScreenshotFrame {
                    width: 2,
                    height: 1,
                    rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
                }))
                .map_err(|_| "screenshot receiver closed".to_string())
        });
        state.upsert_terminal(TerminalMetadata::new(
            "term-1",
            "server",
            "root@example.test",
            "connected",
        ));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        state.set_input_sender("term-1", tx);
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let command = rx.blocking_recv().unwrap();
                match command {
                    SessionCommand::DebugInput { bytes, ack } => {
                        observed_tx.send(bytes).unwrap();
                        let _ = ack.send(Ok(()));
                    }
                    other => panic!("unexpected command: {other:?}"),
                }
            }
        });

        let mut controller = DebugApiController::new(runtime.handle().clone(), state);
        let address = controller.start(0, TOKEN).unwrap();

        let unauthenticated = request(
            address,
            "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            unauthenticated.starts_with("HTTP/1.1 401"),
            "unexpected unauthenticated response: {unauthenticated:?}"
        );

        let health = request(
            address,
            &format!(
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(health.starts_with("HTTP/1.1 200"));
        assert!(health.contains("\"status\":\"ok\""));

        let screen = request(
            address,
            &format!(
                "GET /v1/terminals/term-1/screen?max_lines=1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(screen.starts_with("HTTP/1.1 200"));
        assert!(screen.contains("\"text\":\"new\""));

        let screenshot = request_bytes(
            address,
            &format!(
                "GET /v1/screenshot?max_width=1&max_height=1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
            ),
        );
        let body_start = screenshot
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        let headers = std::str::from_utf8(&screenshot[..body_start]).unwrap();
        assert!(headers.starts_with("HTTP/1.1 200"));
        assert!(headers.contains("content-type: image/png"));
        let decoded = image::load_from_memory(&screenshot[body_start..]).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1, 1));

        let body = r#"{"text":"pwd","submit":true}"#;
        let input = request(
            address,
            &format!(
                "POST /v1/terminals/term-1/input HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(input.starts_with("HTTP/1.1 200"));
        assert_eq!(observed_rx.recv().unwrap(), b"pwd\r");

        let body = r#"{"kind":"click","button":"left","col":4,"row":5,"clicks":2}"#;
        let pointer = request(
            address,
            &format!(
                "POST /v1/terminals/term-1/pointer HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(pointer.starts_with("HTTP/1.1 200"));
        assert!(pointer.contains("\"events\":4"));
        assert_eq!(observed_rx.recv().unwrap(), b"P4:5;R4:5;P4:5;R4:5;");

        controller.start(address.port(), ROTATED_TOKEN).unwrap();
        let old_token = request(
            address,
            &format!(
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(old_token.starts_with("HTTP/1.1 401"));
        let new_token = request(
            address,
            &format!(
                "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {ROTATED_TOKEN}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(new_token.starts_with("HTTP/1.1 200"));

        controller.stop();
        assert!(!controller.is_running());
    }
}
