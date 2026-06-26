//! Single-binary persistent-connection daemon.
//!
//! `speak daemon` runs a long-lived process holding ONE warm pooled
//! [`SpeechClient`], listening on a Unix socket. CLI invocations forward their
//! request to it (length-prefixed framing) so the HTTP rides a connection that
//! survives across separate `speak` runs; when no daemon is up, callers fall
//! back to a direct one-shot client. `daemon stop` / `daemon status` manage it.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::client::{Field, ProxyReply, SpeechClient};
use crate::config::Config;

/// `daemon` subcommands (absent => start the server).
#[derive(clap::Subcommand, Debug)]
pub enum DaemonCmd {
    /// Stop a running daemon.
    Stop,
    /// Print daemon status JSON.
    Status,
}

/// `daemon` arguments.
#[derive(clap::Args, Debug)]
pub struct DaemonArgs {
    /// Run attached in the foreground (also the current default).
    #[arg(long)]
    pub foreground: bool,
    #[command(subcommand)]
    action: Option<DaemonCmd>,
}

/// Wire request: a proxied HTTP call or a control op.
#[derive(Debug, Serialize, Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    json: Option<Value>,
    #[serde(default)]
    fields: Vec<Field>,
    #[serde(default)]
    has_file: bool,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    file_part: String,
}

/// Wire reply header (the raw body follows in a second frame).
#[derive(Debug, Serialize, Deserialize)]
struct ReplyHeader {
    ok: bool,
    error: Option<String>,
    status: u16,
    content_type: String,
    rtf: Option<String>,
    audio_seconds: Option<String>,
}

/// Dispatch `daemon` subcommands.
pub async fn run(cfg: &Config, args: DaemonArgs) -> Result<()> {
    match args.action {
        None => start(cfg, args.foreground).await,
        Some(DaemonCmd::Stop) => stop(cfg).await,
        Some(DaemonCmd::Status) => status(cfg).await,
    }
}

/// Forward a JSON/bodyless proxy request to the daemon at `socket`.
pub async fn forward_json(
    socket: &Path,
    method: &str,
    endpoint: &str,
    json_body: Option<Value>,
) -> Result<ProxyReply> {
    let request = Request {
        op: "proxy".into(),
        method: method.to_owned(),
        endpoint: endpoint.to_owned(),
        json: json_body,
        fields: Vec::new(),
        has_file: false,
        filename: String::new(),
        file_part: String::new(),
    };
    forward(socket, request, None).await
}

/// Forward a multipart proxy request to the daemon at `socket`.
pub async fn forward_multipart(
    socket: &Path,
    endpoint: &str,
    fields: &[Field],
    file: Option<(Vec<u8>, String)>,
    file_part: &str,
) -> Result<ProxyReply> {
    let (has_file, filename, body) = match file {
        Some((bytes, name)) => (true, name, Some(bytes)),
        None => (false, String::new(), None),
    };
    let request = Request {
        op: "multipart".into(),
        method: "POST".into(),
        endpoint: endpoint.to_owned(),
        json: None,
        fields: fields.to_vec(),
        has_file,
        filename,
        file_part: file_part.to_owned(),
    };
    forward(socket, request, body).await
}

/// True when a daemon is accepting connections on `socket`.
pub async fn is_running(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

async fn forward(socket: &Path, request: Request, body: Option<Vec<u8>>) -> Result<ProxyReply> {
    let mut stream = UnixStream::connect(socket).await.context("connecting to daemon socket")?;
    write_frame(&mut stream, &serde_json::to_vec(&request)?).await?;
    if let Some(bytes) = body {
        write_frame(&mut stream, &bytes).await?;
    }
    let header: ReplyHeader = serde_json::from_slice(&read_frame(&mut stream).await?)?;
    let body = read_frame(&mut stream).await?;
    if !header.ok {
        bail!("daemon error: {}", header.error.unwrap_or_default());
    }
    Ok(ProxyReply {
        status: header.status,
        content_type: header.content_type,
        rtf: header.rtf,
        audio_seconds: header.audio_seconds,
        body,
    })
}

// --------------------------------------------------------------------------
// Server
// --------------------------------------------------------------------------

struct State {
    client: SpeechClient,
    started: Instant,
    requests: AtomicU64,
    socket: std::path::PathBuf,
    host: String,
    idle_timeout: u64,
    shutdown: Notify,
    last_active: std::sync::Mutex<Instant>,
}

async fn start(cfg: &Config, _foreground: bool) -> Result<()> {
    let socket = cfg.daemon.socket.clone();
    if is_running(&socket).await {
        bail!("daemon already running at {}", socket.display());
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    let state = Arc::new(State {
        client: SpeechClient::new(cfg)?,
        started: Instant::now(),
        requests: AtomicU64::new(0),
        socket: socket.clone(),
        host: cfg.server.host.clone(),
        idle_timeout: cfg.daemon.idle_timeout,
        shutdown: Notify::new(),
        last_active: std::sync::Mutex::new(Instant::now()),
    });
    tracing::info!(socket = %socket.display(), host = %state.host, "daemon listening");
    if !cfg.general.quiet {
        eprintln!("speak daemon listening at {} (host {})", socket.display(), state.host);
    }
    accept_loop(&listener, &state).await;
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

async fn accept_loop(listener: &UnixListener, state: &Arc<State>) {
    spawn_idle_watch(state);
    loop {
        tokio::select! {
            biased;
            () = state.shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let state = Arc::clone(state);
                    tokio::spawn(async move {
                        if let Err(e) = serve(stream, &state).await {
                            tracing::warn!("daemon connection error: {e:#}");
                        }
                    });
                }
            }
        }
    }
}

fn spawn_idle_watch(state: &Arc<State>) {
    if state.idle_timeout == 0 {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let idle = state.last_active.lock().map(|t| t.elapsed().as_secs()).unwrap_or(0);
            if idle >= state.idle_timeout {
                state.shutdown.notify_one();
                break;
            }
        }
    });
}

async fn serve(mut stream: UnixStream, state: &Arc<State>) -> Result<()> {
    let request: Request = serde_json::from_slice(&read_frame(&mut stream).await?)?;
    if let Ok(mut t) = state.last_active.lock() {
        *t = Instant::now();
    }
    match request.op.as_str() {
        "stop" => {
            write_ok(&mut stream, &json!({"stopped": true})).await?;
            state.shutdown.notify_one();
        }
        "status" => write_ok(&mut stream, &status_body(state)).await?,
        "proxy" => serve_proxy(&mut stream, state, &request).await?,
        "multipart" => serve_multipart(&mut stream, state, request).await?,
        other => write_err(&mut stream, &format!("unknown op '{other}'")).await?,
    }
    Ok(())
}

async fn serve_proxy(stream: &mut UnixStream, state: &Arc<State>, request: &Request) -> Result<()> {
    state.requests.fetch_add(1, Ordering::Relaxed);
    let result = state.client.proxy(&request.method, &request.endpoint, request.json.clone()).await;
    write_reply(stream, result).await
}

async fn serve_multipart(stream: &mut UnixStream, state: &Arc<State>, request: Request) -> Result<()> {
    state.requests.fetch_add(1, Ordering::Relaxed);
    let file = if request.has_file {
        Some((read_frame(stream).await?, request.filename.clone()))
    } else {
        None
    };
    let result = state
        .client
        .proxy_multipart(&request.endpoint, &request.fields, file, &request.file_part)
        .await;
    write_reply(stream, result).await
}

fn status_body(state: &State) -> Value {
    json!({
        "pid": std::process::id(),
        "uptime_secs": state.started.elapsed().as_secs(),
        "requests": state.requests.load(Ordering::Relaxed),
        "socket": state.socket.display().to_string(),
        "host": state.host,
    })
}

async fn write_reply(stream: &mut UnixStream, result: Result<ProxyReply>) -> Result<()> {
    match result {
        Ok(reply) => {
            let header = ReplyHeader {
                ok: true,
                error: None,
                status: reply.status,
                content_type: reply.content_type,
                rtf: reply.rtf,
                audio_seconds: reply.audio_seconds,
            };
            write_frame(stream, &serde_json::to_vec(&header)?).await?;
            write_frame(stream, &reply.body).await
        }
        Err(e) => write_err(stream, &format!("{e:#}")).await,
    }
}

async fn write_ok(stream: &mut UnixStream, body: &Value) -> Result<()> {
    let header = ReplyHeader {
        ok: true,
        error: None,
        status: 200,
        content_type: "application/json".into(),
        rtf: None,
        audio_seconds: None,
    };
    write_frame(stream, &serde_json::to_vec(&header)?).await?;
    write_frame(stream, &serde_json::to_vec(body)?).await
}

async fn write_err(stream: &mut UnixStream, message: &str) -> Result<()> {
    let header = ReplyHeader {
        ok: false,
        error: Some(message.to_owned()),
        status: 0,
        content_type: String::new(),
        rtf: None,
        audio_seconds: None,
    };
    write_frame(stream, &serde_json::to_vec(&header)?).await?;
    write_frame(stream, &[]).await
}

async fn stop(cfg: &Config) -> Result<()> {
    let socket = &cfg.daemon.socket;
    if !is_running(socket).await {
        println!("no daemon running at {}", socket.display());
        return Ok(());
    }
    let mut stream = UnixStream::connect(socket).await?;
    write_frame(&mut stream, &serde_json::to_vec(&control("stop"))?).await?;
    let _ = read_frame(&mut stream).await;
    println!("stopped daemon at {}", socket.display());
    Ok(())
}

async fn status(cfg: &Config) -> Result<()> {
    let socket = &cfg.daemon.socket;
    if !is_running(socket).await {
        println!("{}", json!({"running": false, "socket": socket.display().to_string()}));
        return Ok(());
    }
    let mut stream = UnixStream::connect(socket).await?;
    write_frame(&mut stream, &serde_json::to_vec(&control("status"))?).await?;
    let _header = read_frame(&mut stream).await?;
    let body = read_frame(&mut stream).await?;
    let mut value: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("running".into(), json!(true));
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn control(op: &str) -> Request {
    Request {
        op: op.to_owned(),
        method: String::new(),
        endpoint: String::new(),
        json: None,
        fields: Vec::new(),
        has_file: false,
        filename: String::new(),
        file_part: String::new(),
    }
}

async fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).context("frame too large")?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let len = stream.read_u32().await?;
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}
