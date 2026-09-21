// Package-run MCP servers — the registry's long tail (io.github.* and
// friends) ships as npm/pypi packages or oci images speaking stdio, not
// hosted endpoints. Each install becomes ONE HARDENED `docker run` child of
// this process: stdio packages run `docker run -i` with JSON-RPC piped
// through here; oci packages declaring an http transport run detached and
// are relayed like any remote — reached by container DNS when this api is
// itself in a container (the app-db pattern), or a loopback-published port
// on the host. Credentials are declared exactly like remote headers (the
// registry's same Input schema) but live as SEALED ENV, materialized into
// the child's environment only at spawn — the gateway stays the only
// egress, agent configs stay credential-free, and nothing env-shaped ever
// answers a GET.
//
// SECURITY POSTURE: a community package is third-party code that will hold
// real credentials. The container gets the Hermes chassis's shape minus its
// cap_adds (those exist for s6-overlay boot; stock node/uv images need
// none, so packages run strictly stricter): no-new-privileges, cap-drop
// ALL, pids/memory/cpus ceilings, pinned DNS, default bridge network —
// outbound for npx/uvx, no path to the fleet network or postgres, no
// docker socket. Runtime arguments arrive from the registry as LITERAL
// `docker run` flags, so an allowlist (volumes/env/mounts/add-host/publish)
// is the gate, never verbatim appends.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::registry::McpServer;
use talaria_agent_auth::now_ms;
use talaria_mcp_jsonrpc::MCP_PROTOCOL_VERSION;
use talaria_secretbox::SecretBox;

/// Named `docker run` flags a registry package may ask for. Anything else —
/// `--privileged`, `--pid=host`, `--network=host` are all expressible in a
/// package's runtimeArguments — is refused at install (the POST parser
/// checks the declaration; pkg_argv re-checks at spawn — defense in depth).
pub const ALLOWED_FLAGS: &[&str] = &[
    "-v",
    "--volume",
    "-e",
    "--env",
    "--mount",
    "--add-host",
    "-p",
    "--publish",
];

/// Pending-request bound: a package server that busy-loops its caller gets
/// refused past this many concurrent calls rather than buffered forever.
const MAX_PENDING: usize = 64;

/// One respawn attempt per server per window — a crash-looping package must
/// not turn every gateway call into a spawn.
const RESPAWN_DEBOUNCE_MS: u64 = 10_000;

// ── naming + topology ───────────────────────────────────────────────────────

/// The deployment instance this api belongs to — worktrees/devboxes on a
/// shared daemon must never see each other's containers (the app-db
/// `talaria-appdb-<inst>-<slug>` rule).
pub fn instance() -> String {
    std::env::var("TALARIA_WORKTREE")
        .or_else(|_| std::env::var("TALARIA_DEVBOX"))
        .unwrap_or_else(|_| "talaria".into())
}

pub fn container_name(server_name: &str) -> String {
    format!("talaria-mcp-{}-{}", instance(), server_name)
}

/// Is THIS api running inside the product image? In-image, published
/// loopback ports are unreachable (they bind the host); the app-db answer
/// applies: join the api's own network and dial by container DNS.
fn in_image() -> bool {
    std::env::var("TALARIA_INSTALL").ok().as_deref() == Some("image")
        || std::env::var("TALARIA_RUNTIME").ok().as_deref() == Some("prod-server")
}

async fn api_network() -> Option<String> {
    if !in_image() {
        return None;
    }
    if let Ok(named) = std::env::var("TALARIA_APP_DB_NETWORK") {
        return Some(named);
    }
    let self_id = std::env::var("HOSTNAME").ok()?;
    let (out, _) = talaria_fleet_docker::docker(
        &[
            "inspect",
            "-f",
            "{{range $k,$v := .NetworkSettings.Networks}}{{$k}}\n{{end}}",
            &self_id,
        ],
        Duration::from_secs(10),
    )
    .await
    .ok()?;
    out.lines()
        .map(str::trim)
        .find(|n| n.ends_with("_internal") || *n == "internal")
        .or_else(|| out.lines().map(str::trim).find(|n| !n.is_empty()))
        .map(str::to_string)
}

// ── the spec (typed view over the row's `package` jsonb) ────────────────────

pub struct PkgSpec {
    pub kind: String,
    pub identifier: String,
    pub version: Option<String>,
    pub runtime_hint: Option<String>,
    pub transport: String,
    pub container_port: Option<u64>,
    pub transport_path: Option<String>,
    pub image: String,
    pub image_digest: Option<String>,
    pub run_args: Vec<Value>,
    pub pull_state: String,
}

impl PkgSpec {
    /// None when the row isn't a package server (or the document is broken —
    /// callers treat that as "not a package" rather than a thrown error).
    pub fn of(package: &Value) -> Option<PkgSpec> {
        let obj = package.as_object()?;
        Some(PkgSpec {
            kind: obj.get("kind")?.as_str()?.to_string(),
            identifier: obj.get("identifier")?.as_str()?.to_string(),
            version: obj
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            runtime_hint: obj
                .get("runtimeHint")
                .and_then(Value::as_str)
                .map(str::to_string),
            transport: obj
                .get("transport")
                .and_then(Value::as_str)
                .unwrap_or("stdio")
                .to_string(),
            container_port: obj.get("containerPort").and_then(Value::as_u64),
            transport_path: obj
                .get("transportPath")
                .and_then(Value::as_str)
                .map(str::to_string),
            image: obj.get("image")?.as_str()?.to_string(),
            image_digest: obj
                .get("imageDigest")
                .and_then(Value::as_str)
                .map(str::to_string),
            run_args: obj
                .get("runArgs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            pull_state: obj
                .get("pull")
                .and_then(|p| p.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("pulling")
                .to_string(),
        })
    }

    /// The image reference spawns pin to — the RepoDigest when the pull
    /// captured one (the app-db rule), the tag until then.
    fn pinned_image(&self) -> String {
        self.image_digest
            .clone()
            .unwrap_or_else(|| self.image.clone())
    }
}

// args: {"<runArg index>": "value"}}`.
#[derive(Default)]
pub struct SealedDoc {
    pub env: std::collections::BTreeMap<String, String>,
    pub args: HashMap<usize, String>,
}

impl SealedDoc {
    pub fn of(v: &Value) -> SealedDoc {
        let mut doc = SealedDoc::default();
        if let Some(env) = v.get("env").and_then(Value::as_object) {
            for (k, val) in env {
                if let Some(s) = val.as_str() {
                    doc.env.insert(k.clone(), s.to_string());
                }
            }
        }
        if let Some(args) = v.get("args").and_then(Value::as_object) {
            for (k, val) in args {
                if let (Ok(idx), Some(s)) = (k.parse::<usize>(), val.as_str()) {
                    doc.args.insert(idx, s.to_string());
                }
            }
        }
        doc
    }

    /// Seal the install-time capture.
    pub fn seal(
        sb: &SecretBox,
        env: Map<String, Value>,
        args: HashMap<usize, String>,
    ) -> Result<String, String> {
        let args: Map<String, Value> = args
            .into_iter()
            .map(|(k, v)| (k.to_string(), Value::String(v)))
            .collect();
        let doc = json!({ "env": env, "args": args });
        sb.seal(&doc.to_string()).map_err(|e| e.to_string())
    }

    pub fn open(sb: &SecretBox, enc: &str) -> Option<SealedDoc> {
        let opened = sb.open(enc).ok()?;
        serde_json::from_str::<Value>(&opened)
            .ok()
            .map(|v| SealedDoc::of(&v))
    }
}

// ── the argv builder (pure; the tests drive it directly) ────────────────────

async fn pinned_dns() -> Vec<String> {
    let text = tokio::fs::read_to_string(talaria_fleet_layout::fleet_env())
        .await
        .unwrap_or_default();
    let pick = |key: &str, fallback: &str| {
        regex::Regex::new(&format!(r"(?m)^{key}=(\S+)"))
            .ok()
            .and_then(|re| {
                re.captures(&text)
                    .and_then(|c| c.get(1).map(|m| m.as_str()))
            })
            .unwrap_or(fallback)
            .to_string()
    };
    vec![
        pick("AGENT_DNS_1", "1.1.1.1"),
        pick("AGENT_DNS_2", "1.0.0.1"),
    ]
}

fn hardening(argv: &mut Vec<String>, dns: &[String]) {
    argv.push("--security-opt".into());
    argv.push("no-new-privileges:true".into());
    argv.push("--cap-drop".into());
    argv.push("ALL".into());
    argv.push("--pids-limit".into());
    argv.push("256".into());
    argv.push("--memory".into());
    argv.push("1g".into());
    argv.push("--cpus".into());
    argv.push("1".into());
    for d in dns {
        argv.push("--dns".into());
        argv.push(d.clone());
    }
}

/// The full `docker run` argv for a package. Errors carry the route's 400
/// sentence (allowlist violations name the flag). Pure.
pub fn pkg_argv(
    spec: &PkgSpec,
    sealed: &SealedDoc,
    container: &str,
    dns: &[String],
    http_detached: bool,
) -> Result<Vec<String>, String> {
    let mut argv = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container.to_string(),
    ];
    if http_detached {
        argv.push("-d".into());
    } else {
        argv.push("-i".into());
    }
    hardening(&mut argv, dns);
    // npx/uvx re-download their package on EVERY spawn without these — the
    // named volumes make the second spawn instant and the first the only
    // slow one.
    match spec.kind.as_str() {
        "npm" => {
            argv.push("-v".into());
            argv.push(format!("talaria-mcp-npm-cache-{}:/root/.npm", instance()));
        }
        "pypi" => {
            argv.push("-v".into());
            argv.push(format!(
                "talaria-mcp-uv-cache-{}:/root/.cache/uv",
                instance()
            ));
        }
        _ => {}
    }
    for (k, v) in &sealed.env {
        argv.push("-e".into());
        argv.push(format!("{k}={v}"));
    }
    // The registry's declared run flags — allowlisted NAMED flags only;
    // positionals are appended after the image below.
    for (i, arg) in spec.run_args.iter().enumerate() {
        if arg.get("type").and_then(Value::as_str) != Some("named") {
            continue;
        }
        let flag = arg.get("name").and_then(Value::as_str).unwrap_or_default();
        if !ALLOWED_FLAGS.contains(&flag) {
            return Err(format!(
                "this package asks for the docker flag \"{flag}\", which is not allowed (volumes, env, mounts, hosts and ports only)"
            ));
        }
        let Some(v) = arg_value(arg, sealed, i) else {
            continue; // declared but unfilled and unfillable — skip
        };
        argv.push(flag.to_string());
        argv.push(v);
    }
    if http_detached {
        // oci-http: the transport names its in-container port; the caller
        // appends network args (topology-dependent, see ensure_http).
        if let Some(port) = spec.container_port {
            argv.push("-p".into());
            argv.push(format!("127.0.0.1::{port}"));
        }
    }
    // The image (or stock runner) and its entry point. The runner images
    // carry their own ENTRYPOINT (uv's is `uv`), so the entry command rides
    // an explicit --entrypoint override rather than a prepended argv word.
    match spec.kind.as_str() {
        "npm" => {
            argv.push("node:22-slim".into());
            argv.push("npx".into());
            argv.push("-y".into());
            argv.push(match spec.version.as_deref() {
                Some(v) => format!("{}@{}", spec.identifier, v),
                None => spec.identifier.clone(),
            });
        }
        "pypi" => {
            // The runner image ships only `uv` (no uvx symlink), so every
            // form rides the uv entrypoint: `uv tool run pkg` is uvx, and a
            // python-hinted package runs as `uv run pkg`.
            argv.push("--entrypoint".into());
            argv.push("/usr/local/bin/uv".into());
            argv.push("ghcr.io/astral-sh/uv:python3.12-bookworm-slim".into());
            let target = match spec.version.as_deref() {
                Some(v) => format!("{}=={}", spec.identifier, v),
                None => spec.identifier.clone(),
            };
            match spec.runtime_hint.as_deref() {
                Some("python") | Some("uv") => {
                    argv.push("run".into());
                    argv.push(target);
                }
                _ => {
                    argv.push("tool".into());
                    argv.push("run".into());
                    argv.push(target);
                }
            }
        }
        _ => argv.push(spec.pinned_image()),
    }
    // Positionals come after the entry point, in declared order.
    for (i, arg) in spec.run_args.iter().enumerate() {
        if arg.get("type").and_then(Value::as_str) == Some("named") {
            continue;
        }
        if let Some(v) = arg_value(arg, sealed, i) {
            argv.push(v);
        }
    }
    Ok(argv)
}

/// A declared argument's effective value: the admin's fill, else the
/// declared default, else the declared literal.
fn arg_value(arg: &Value, sealed: &SealedDoc, i: usize) -> Option<String> {
    sealed.args.get(&i).cloned().or_else(|| {
        arg.get("default")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| arg.get("value").and_then(Value::as_str).map(str::to_string))
    })
}

// ── the live-child registry + the stdio pump ────────────────────────────────

struct PkgChild {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: Arc<tokio::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>,
    next_id: std::sync::atomic::AtomicU64,
    initialize: tokio::sync::RwLock<Option<Value>>,
}

fn children() -> &'static Mutex<HashMap<String, Arc<PkgChild>>> {
    static KIDS: LazyLock<Mutex<HashMap<String, Arc<PkgChild>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &KIDS
}

fn spawn_ms() -> &'static Mutex<HashMap<String, u64>> {
    static MS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    &MS
}

fn rpc_error(code: i64, message: &str, id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

async fn spawn_stdio_child(
    server_name: &str,
    spec: &PkgSpec,
    sealed: &SealedDoc,
    timeout_secs: i64,
) -> Result<Arc<PkgChild>, String> {
    let container = container_name(server_name);
    // A stray from a previous api life would steal the name.
    let _ = talaria_fleet_docker::docker(&["rm", "-f", &container], Duration::from_secs(30)).await;
    let dns = pinned_dns().await;
    let argv = pkg_argv(spec, sealed, &container, &dns, false)?;
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut child = tokio::process::Command::new("docker")
        .args(&argv_refs)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("docker run failed: {e}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "docker run gave no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "docker run gave no stdout".to_string())?;
    if let Some(stderr) = child.stderr.take() {
        let log_prefix = format!("[talaria-mcp/{server_name}]");
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    tracing::error!("{log_prefix} {}", line.trim());
                }
            }
        });
    }
    let pending: Arc<tokio::sync::Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Value>>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let kid = Arc::new(PkgChild {
        stdin: tokio::sync::Mutex::new(stdin),
        pending: pending.clone(),
        next_id: std::sync::atomic::AtomicU64::new(1),
        initialize: tokio::sync::RwLock::new(None),
    });
    {
        let reader_pending = pending.clone();
        let server_name = server_name.to_string();
        let kid_weak = Arc::downgrade(&kid);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                if line.len() > 16 * 1024 * 1024 {
                    tracing::error!("[talaria-mcp/{server_name}] oversized line dropped");
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(id) = v.get("id").and_then(Value::as_i64) else {
                    // A server→client notification (progress, logs) —
                    // nothing here consumes them.
                    tracing::debug!(
                        "[talaria-mcp/{server_name}] notification: {}",
                        v.get("method").and_then(|m| m.as_str()).unwrap_or("?")
                    );
                    continue;
                };
                if let Some(tx) = reader_pending.lock().await.remove(&(id as u64)) {
                    let _ = tx.send(v);
                }
            }
            // EOF/exit: fail everything in flight, evict so the next call
            // respawns, drop the cached handshake with the child.
            let mut p = reader_pending.lock().await;
            for (_, tx) in p.drain() {
                let _ = tx.send(rpc_error(-32000, "package server exited", Value::Null));
            }
            drop(p);
            children()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&server_name);
            if let Some(k) = kid_weak.upgrade() {
                *k.initialize.write().await = None;
            }
            tracing::error!("[talaria-mcp/{server_name}] exited — will respawn on next use");
        });
    }
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    // The one handshake per lifetime (id 0 — caller ids never collide with
    // it because next_id starts at 1).
    let init = raw_request(
        &kid,
        timeout_secs,
        json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "talaria", "version": "1.0" },
            },
        }),
    )
    .await?;
    if init.get("error").is_some() {
        return Err(format!(
            "package server refused the handshake: {}",
            init.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ));
    }
    // initialized is a notification — no reply to await.
    {
        use tokio::io::AsyncWriteExt;
        let mut w = kid.stdin.lock().await;
        let _ = w
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await;
    }
    *kid.initialize.write().await = Some(init);
    Ok(kid)
}

async fn raw_request(
    kid: &Arc<PkgChild>,
    timeout_secs: i64,
    mut rpc: Value,
) -> Result<Value, String> {
    let id = kid
        .next_id
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let caller_id = rpc.get("id").cloned();
    if let Some(obj) = rpc.as_object_mut() {
        obj.insert("id".into(), Value::from(id as i64));
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut p = kid.pending.lock().await;
        if p.len() >= MAX_PENDING {
            return Ok(rpc_error(-32604, "package server busy", Value::Null));
        }
        p.insert(id, tx);
    }
    use tokio::io::AsyncWriteExt;
    {
        let mut w = kid.stdin.lock().await;
        w.write_all(format!("{}\n", rpc).as_bytes())
            .await
            .map_err(|e| format!("package server write failed: {e}"))?;
        w.flush().await.map_err(|e| format!("flush failed: {e}"))?;
    }
    let timeout = Duration::from_secs(timeout_secs.clamp(30, 600) as u64);
    let answer = tokio::time::timeout(timeout, rx)
        .await
        .map_err(|_| "package server timed out".to_string())?
        .map_err(|_| "package server exited".to_string())?;
    let mut answer = answer;
    if let Some(obj) = answer.as_object_mut() {
        obj.insert("id".into(), caller_id.unwrap_or(Value::Null));
    }
    Ok(answer)
}

async fn fresh_spec(pg: &PgPool, server_id: &str) -> Result<Option<PkgSpec>, String> {
    let row: Option<(Option<Value>,)> =
        sqlx::query_as("select package from mcp_servers where id::text = $1")
            .bind(server_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    Ok(row.and_then(|(p,)| p).as_ref().and_then(PkgSpec::of))
}

async fn ensure_child(
    pg: &PgPool,
    sb: &SecretBox,
    server: &McpServer,
    spec: &PkgSpec,
) -> Result<Arc<PkgChild>, String> {
    if let Some(kid) = children()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&server.name)
    {
        return Ok(kid.clone());
    }
    {
        let mut ms = spawn_ms().lock().unwrap_or_else(|p| p.into_inner());
        let last = *ms.get(&server.name).unwrap_or(&0);
        if (now_ms() as u64).saturating_sub(last) < RESPAWN_DEBOUNCE_MS {
            return Err("package server is restarting; retry shortly".into());
        }
        ms.insert(server.name.clone(), now_ms() as u64);
    }
    if spec.pull_state != "ready" {
        let mut waited = 0u64;
        while waited < 30_000 {
            match fresh_spec(pg, &server.id).await?.map(|s| s.pull_state) {
                Some(state) => match state.as_str() {
                    "ready" => break,
                    "error" => {
                        return Err(
                            "this package's image failed to pull — check the server card".into(),
                        );
                    }
                    _ => {}
                },
                None => return Err("the package row vanished mid-call".into()),
            }
            tokio::time::sleep(Duration::from_millis(1_000)).await;
            waited += 1_000;
        }
        if waited >= 30_000 {
            return Err("this package's image is still pulling; try again shortly".into());
        }
    }
    // The pull may have pinned a digest while we waited.
    let spec = match fresh_spec(pg, &server.id).await {
        Ok(Some(fresh)) => fresh,
        _ => {
            let _ = spec; // the row changed shape under us — say so
            return Err("the package row changed mid-call".into());
        }
    };
    let sealed = match &server.env_enc {
        Some(enc) => SealedDoc::open(sb, enc).unwrap_or_default(),
        None => SealedDoc::default(),
    };
    let kid = spawn_stdio_child(
        &server.name,
        &spec,
        &sealed,
        server.timeout_secs.unwrap_or(120),
    )
    .await?;
    children()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(server.name.clone(), kid.clone());
    Ok(kid)
}

/// One gateway dispatch into a stdio package. Initializes answer from the
/// cached handshake (a second one is a protocol error in every SDK);
/// initialized echoes succeed vacuously.
pub async fn pkg_call(
    pg: &PgPool,
    sb: &SecretBox,
    server: &McpServer,
    spec: &PkgSpec,
    rpc: &Value,
) -> Result<(u16, Value), String> {
    let kid = ensure_child(pg, sb, server, spec).await?;
    match rpc.get("method").and_then(Value::as_str) {
        Some("initialize") => {
            let mut answer =
                kid.initialize.read().await.clone().unwrap_or_else(|| {
                    rpc_error(-32003, "package server handshake lost", Value::Null)
                });
            if let (Some(obj), Some(id)) = (answer.as_object_mut(), rpc.get("id")) {
                obj.insert("id".into(), id.clone());
            }
            Ok((200, answer))
        }
        Some("notifications/initialized") => Ok((200, Value::Null)),
        _ => {
            let answer = raw_request(&kid, server.timeout_secs.unwrap_or(120), rpc.clone()).await?;
            Ok((200, answer))
        }
    }
}

// ── oci-http: detached container + resolved URL ─────────────────────────────

/// Ensure the detached oci-http container runs and return the URL this api
/// relays to (container DNS in-image, loopback publish on the host).
pub async fn ensure_http(
    sb: &SecretBox,
    server: &McpServer,
    spec: &PkgSpec,
) -> Result<String, String> {
    let container = container_name(&server.name);
    let port = spec.container_port.ok_or("this package declares no port")?;
    let path = spec.transport_path.clone().unwrap_or_else(|| "/mcp".into());
    let running = talaria_fleet_docker::docker(
        &["inspect", "-f", "{{.State.Running}}", &container],
        Duration::from_secs(10),
    )
    .await
    .map(|(out, _)| out.trim() == "true")
    .unwrap_or(false);
    if !running {
        let _ =
            talaria_fleet_docker::docker(&["rm", "-f", &container], Duration::from_secs(30)).await;
        let sealed = match &server.env_enc {
            Some(enc) => SealedDoc::open(sb, enc).unwrap_or_default(),
            None => SealedDoc::default(),
        };
        let dns = pinned_dns().await;
        let mut argv = pkg_argv(spec, &sealed, &container, &dns, true)?;
        if let Some(net) = api_network().await {
            argv.push("--network".into());
            argv.push(net);
        }
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        talaria_fleet_docker::docker(&argv_refs, Duration::from_secs(120)).await?;
    }
    if in_image() {
        Ok(format!("http://{container}:{port}{path}"))
    } else {
        let (out, _) = talaria_fleet_docker::docker(
            &["port", &container, &format!("{port}/tcp")],
            Duration::from_secs(10),
        )
        .await?;
        let host_port = out
            .lines()
            .filter_map(|l| l.rsplit(':').next())
            .find_map(|p| p.parse::<u16>().ok())
            .ok_or("the package container published no port")?;
        Ok(format!("http://127.0.0.1:{host_port}{path}"))
    }
}

// ── lifecycle ───────────────────────────────────────────────────────────────

/// Stop whatever this server runs (disable, delete).
pub async fn stop_pkg(server_name: &str) {
    children()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(server_name);
    let container = container_name(server_name);
    let _ = talaria_fleet_docker::docker(&["rm", "-f", &container], Duration::from_secs(30)).await;
}

/// Pull the image (install time) and pin the RepoDigest into the row. Runs
/// in the POST's background task — the row's `pull.state` is the progress
/// the UI reads.
pub async fn pull_and_pin(pg: &PgPool, server_id: &str, image: &str) {
    let (state, digest, error) =
        match talaria_fleet_docker::docker(&["pull", image], Duration::from_secs(600)).await {
            Ok(_) => {
                let digest = talaria_fleet_docker::docker(
                    &["image", "inspect", "-f", "{{json .RepoDigests}}", image],
                    Duration::from_secs(30),
                )
                .await
                .ok()
                .and_then(|(out, _)| serde_json::from_str::<Value>(out.trim()).ok())
                .and_then(|v| {
                    v.as_array()
                        .and_then(|a| a.first())
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
                ("ready", digest, None)
            }
            Err(e) => ("error", None, Some(e)),
        };
    let row = sqlx::query_as::<_, (Option<Value>,)>(
        "select package from mcp_servers where id::text = $1",
    )
    .bind(server_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    // Merge into the stored document (raw-Value rule: read, modify, write —
    // pg owns key order).
    if let Some((Some(mut package),)) = row {
        if let Some(obj) = package.as_object_mut() {
            obj.insert(
                "pull".into(),
                json!({ "state": state, "error": error, "digest": digest }),
            );
            if let Some(d) = &digest {
                obj.insert("imageDigest".into(), Value::String(d.clone()));
            }
        }
        let _ = sqlx::query(
            "update mcp_servers set package = $2, updated_at = now() where id::text = $1",
        )
        .bind(server_id)
        .bind(&package)
        .execute(pg)
        .await;
    }
}

/// Desired-state pass: remove strays a previous api life left (stdio
/// leftovers, containers of deleted rows) and disabled oci-http containers.
/// Boot task + scheduled; healing a STOPPED enabled http container needs
/// the secretbox, so it self-heals on first use instead.
pub async fn reconcile(pg: &PgPool) {
    let rows: Vec<(String, bool, Option<Value>)> = match sqlx::query_as(
        "select name, enabled, package from mcp_servers where package is not null",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[mcp/pkg] reconcile read failed: {e}");
            return;
        }
    };
    // Containers still wanted: live stdio children, or enabled http rows.
    let mut wanted: std::collections::HashSet<String> = children()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .keys()
        .map(|n| container_name(n))
        .collect();
    for (name, enabled, package) in &rows {
        let Some(spec) = package.as_ref().and_then(PkgSpec::of) else {
            continue;
        };
        if spec.transport == "http" && *enabled && spec.pull_state == "ready" {
            wanted.insert(container_name(name));
        }
    }
    let prefix = format!("talaria-mcp-{}-", instance());
    let listed = talaria_fleet_docker::docker(
        &[
            "ps",
            "-a",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Names}}",
        ],
        Duration::from_secs(15),
    )
    .await
    .map(|(out, _)| {
        out.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for name in listed {
        if !wanted.contains(&name) {
            let _ =
                talaria_fleet_docker::docker(&["rm", "-f", &name], Duration::from_secs(30)).await;
            tracing::info!("[mcp/pkg] reconcile removed {name}");
        }
    }
}

/// The status the server card shows.
pub async fn pkg_status(spec: &PkgSpec, server_name: &str) -> String {
    match spec.pull_state.as_str() {
        "pulling" => return "pulling".into(),
        "error" => return "error".into(),
        _ => {}
    }
    if spec.transport != "http" {
        // stdio children spawn on first use — "idle" is a fine resting state.
        return if children()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(server_name)
        {
            "ready".into()
        } else {
            "idle".into()
        };
    }
    let container = container_name(server_name);
    let running = talaria_fleet_docker::docker(
        &["inspect", "-f", "{{.State.Running}}", &container],
        Duration::from_secs(10),
    )
    .await
    .map(|(out, _)| out.trim() == "true")
    .unwrap_or(false);
    if running {
        "ready".into()
    } else {
        "stopped".into()
    }
}

/// The scheduled desired-state pass (5 min; per-instance — the containers
/// being reconciled belong to this deployment's own daemon).
pub fn register_pkg_reconcile_job(pg: PgPool) {
    talaria_scheduler::register_job(talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::McpPkgReconcile,
        every_ms: 5 * 60_000,
        first_run_delay_ms: Some(30_000),
        max_run_ms: Some(60_000),
        per_instance: true,
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move {
                reconcile(&pg).await;
                Ok(None)
            })
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json(kind: &str, transport: &str) -> Value {
        json!({
            "kind": kind,
            "identifier": match kind { "oci" => "ghcr.io/example/srv:v1", _ => "example-srv" },
            "version": "1.2.3",
            "transport": transport,
            "containerPort": if transport == "http" { json!(8080) } else { json!(null) },
            "transportPath": if transport == "http" { json!("/mcp") } else { json!(null) },
            "image": match kind { "oci" => "ghcr.io/example/srv", _ => "node-placeholder" },
            "runArgs": [],
            "pull": { "state": "ready" },
        })
    }

    fn sealed(env: &[(&str, &str)]) -> SealedDoc {
        let mut doc = SealedDoc::default();
        for (k, v) in env {
            doc.env.insert(k.to_string(), v.to_string());
        }
        doc
    }

    const DNS: &[String] = &[];

    #[test]
    fn npm_argv_runs_npx_in_a_hardened_child_with_cache_volume() {
        let spec = PkgSpec::of(&spec_json("npm", "stdio")).unwrap();
        let argv = pkg_argv(
            &spec,
            &sealed(&[("API_KEY", "sk-1")]),
            "talaria-mcp-talaria-x",
            DNS,
            false,
        )
        .unwrap();
        assert_eq!(
            &argv[..4],
            &["run", "--rm", "--name", "talaria-mcp-talaria-x"][..]
        );
        assert!(argv.contains(&"-i".to_string()));
        assert!(!argv.contains(&"-d".to_string()));
        assert!(
            argv.windows(2)
                .any(|w| w == ["--security-opt", "no-new-privileges:true"])
        );
        assert!(argv.windows(2).any(|w| w == ["--cap-drop", "ALL"]));
        assert!(argv.windows(2).any(|w| w == ["--memory", "1g"]));
        assert!(argv.windows(2).any(|w| w == ["-e", "API_KEY=sk-1"]));
        let cache = format!("talaria-mcp-npm-cache-{}:/root/.npm", instance());
        assert!(argv.windows(2).any(|w| w[0] == "-v" && w[1] == cache));
        let at = argv.iter().position(|a| a == "node:22-slim").unwrap();
        assert_eq!(argv[at + 1], "npx");
        assert_eq!(argv[at + 3], "example-srv@1.2.3");
    }

    #[test]
    fn pypi_argv_pins_version_with_double_equals_and_caches_uv() {
        let spec = PkgSpec::of(&spec_json("pypi", "stdio")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, false).unwrap();
        // The uv image ships only `uv`, so uvx runs as `uv tool run`.
        let ep = argv.iter().position(|a| a == "--entrypoint").unwrap();
        assert_eq!(argv[ep + 1], "/usr/local/bin/uv");
        assert_eq!(
            argv[ep + 2],
            "ghcr.io/astral-sh/uv:python3.12-bookworm-slim"
        );
        assert_eq!(argv[ep + 3], "tool");
        assert_eq!(argv[ep + 4], "run");
        assert_eq!(argv[ep + 5], "example-srv==1.2.3");
        let cache = format!("talaria-mcp-uv-cache-{}:/root/.cache/uv", instance());
        assert!(argv.windows(2).any(|w| w[0] == "-v" && w[1] == cache));
    }

    #[test]
    fn oci_http_runs_detached_and_publishes_the_declared_port() {
        let spec = PkgSpec::of(&spec_json("oci", "http")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, true).unwrap();
        assert!(argv.contains(&"-d".to_string()));
        assert!(!argv.contains(&"-i".to_string()));
        assert!(argv.windows(2).any(|w| w == ["-p", "127.0.0.1::8080"]));
        assert!(argv.contains(&"ghcr.io/example/srv".to_string()));
    }

    #[test]
    fn oci_stdio_runs_the_pinned_image_interactively() {
        let spec = PkgSpec::of(&spec_json("oci", "stdio")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, false).unwrap();
        assert!(argv.contains(&"-i".to_string()));
        assert_eq!(argv.last().unwrap(), "ghcr.io/example/srv");
    }

    #[test]
    fn dangerous_flags_are_refused_and_safe_ones_pass() {
        let mut doc = json!({
            "kind": "oci", "identifier": "ghcr.io/x/y:v1", "transport": "stdio",
            "image": "ghcr.io/x/y", "runArgs": [
                { "type": "named", "name": "--privileged" },
                { "type": "named", "name": "-v", "default": "/cfg:/cfg:ro" },
                { "type": "positional", "value": "--stdio" },
            ],
            "pull": { "state": "ready" },
        });
        let spec = PkgSpec::of(&doc).unwrap();
        let err = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, false).unwrap_err();
        assert!(err.contains("--privileged"), "{err}");
        doc["runArgs"].as_array_mut().unwrap().remove(0);
        let spec = PkgSpec::of(&doc).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, false).unwrap();
        assert!(argv.windows(2).any(|w| w == ["-v", "/cfg:/cfg:ro"]));
        // The positional lands after the image, which is argv's last element
        // when no other positionals follow.
        assert_eq!(argv.last().unwrap(), "--stdio");
    }

    #[test]
    fn filled_run_args_override_their_defaults_in_declared_order() {
        let doc = json!({
            "kind": "oci", "identifier": "ghcr.io/x/y:v1", "transport": "stdio",
            "image": "ghcr.io/x/y", "runArgs": [
                { "type": "named", "name": "-p", "default": "9999:80" },
                { "type": "positional", "value": "serve" },
                { "type": "positional", "placeholder": "the mount" },
            ],
            "pull": { "state": "ready" },
        });
        let spec = PkgSpec::of(&doc).unwrap();
        let mut sealed_doc = SealedDoc::default();
        sealed_doc.args.insert(2, "/data:/data".to_string());
        let argv = pkg_argv(&spec, &sealed_doc, "c", DNS, false).unwrap();
        assert!(argv.windows(2).any(|w| w == ["-p", "9999:80"]));
        let image_at = argv.iter().position(|a| a == "ghcr.io/x/y").unwrap();
        assert_eq!(argv[image_at + 1], "serve");
        assert_eq!(argv[image_at + 2], "/data:/data");
    }

    #[test]
    fn sealed_doc_round_trips_env_and_filled_args() {
        let sb = test_box();
        let mut env = serde_json::Map::new();
        env.insert("TOKEN".into(), json!("t1"));
        let mut args = HashMap::new();
        args.insert(2usize, "8080:8080".to_string());
        let enc = SealedDoc::seal(&sb, env, args).unwrap();
        let doc = SealedDoc::open(&sb, &enc).unwrap();
        assert_eq!(doc.env.get("TOKEN").map(String::as_str), Some("t1"));
        assert_eq!(doc.args.get(&2).map(String::as_str), Some("8080:8080"));
    }

    fn test_box() -> SecretBox {
        let kek = talaria_secretbox::derive_kek("pkg-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
    }
}
