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
// ALL, pids/memory/cpus ceilings, pinned DNS, an unprivileged uid
// (65534, so nothing inside is root anywhere), a read-only root with one
// tmpfs at /tmp (HOME — npx and uv keep their caches there, so a respawn
// re-downloads; a bind mount for a cache would be the hole this closes),
// no docker socket. NETWORK: stdio packages sit on the default bridge
// (outbound only — nothing dials in); oci-http packages get their own
// `talaria-mcp-<instance>` bridge network, which this api joins on demand
// when it runs in-image, so the relay reaches them by container DNS and
// they still have no path to the fleet network, postgres, redis or any
// other `*_internal` service. Runtime arguments arrive from the registry as
// LITERAL `docker run` flags, so an allowlist is the gate, never verbatim
// appends — and the allowlist checks VALUES, not just names: a volume may
// only be a `talaria-mcp-*` named volume (never a host path, never the
// socket), a publish may only bind loopback, and `--mount`/`--add-host`
// are refused outright (`check_run_flag`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use sqlx::PgPool;

use crate::registry::McpServer;
use talaria_agent_auth::now_ms;
use talaria_mcp_jsonrpc::MCP_PROTOCOL_VERSION;
use talaria_secretbox::SecretBox;

/// Named `docker run` flags a registry package may ask for. Anything else —
/// `--privileged`, `--pid=host`, `--network=host`, `--mount`, `--add-host`
/// are all expressible in a package's runtimeArguments — is refused at
/// install (the POST parser checks the declaration; pkg_argv re-checks at
/// spawn — defense in depth). The name is half the gate: `check_run_flag`
/// is the whole of it, value included.
pub const ALLOWED_FLAGS: &[&str] = &["-v", "--volume", "-e", "--env", "-p", "--publish"];

/// The sentence every refusal of a flag NAME carries.
fn flag_refused(flag: &str) -> String {
    format!(
        "this package asks for the docker flag \"{flag}\", which is not allowed (named volumes, env and loopback ports only)"
    )
}

/// A named volume a package may mount: the instance-scoped prefix, then
/// `[a-z0-9-]`. No `/` (a host path), no `.` (`./here`), no `:` — so the
/// source can only ever be a docker-managed volume, never the host
/// filesystem or the docker socket.
fn named_volume_ok(src: &str) -> bool {
    src.strip_prefix("talaria-mcp-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// The gate for one declared run flag and the value it would carry. `None`
/// = the declaration carries no value yet (an unfilled placeholder) — the
/// name alone is checked; the fill is checked again when it exists. Pure;
/// the POST parser and `pkg_argv` share it.
pub fn check_run_flag(flag: &str, value: Option<&str>) -> Result<(), String> {
    if !ALLOWED_FLAGS.contains(&flag) {
        return Err(flag_refused(flag));
    }
    let Some(value) = value else {
        return Ok(());
    };
    match flag {
        "-v" | "--volume" => {
            // `source:destination[:options]` — the source is a named volume,
            // the destination an absolute container path, the options the
            // documented read/write and SELinux words only.
            let parts: Vec<&str> = value.split(':').collect();
            let ok = matches!(parts.len(), 2 | 3)
                && named_volume_ok(parts[0])
                && parts[1].starts_with('/')
                && parts.get(2).is_none_or(|opts| {
                    !opts.is_empty()
                        && opts
                            .split(',')
                            .all(|o| matches!(o, "ro" | "rw" | "z" | "Z" | "nocopy"))
                });
            if !ok {
                return Err(format!(
                    "this package asks to mount \"{value}\", which is not allowed (only a talaria-mcp-* named volume may be mounted — never a host path)"
                ));
            }
        }
        "-p" | "--publish" => {
            // Loopback only: `127.0.0.1:host:container[/proto]` or
            // `127.0.0.1::container[/proto]` — digits, colons, a port range's
            // dash, and the protocol word.
            let bound_to_loopback = value.strip_prefix("127.0.0.1:").is_some_and(|rest| {
                let ports = rest
                    .strip_suffix("/tcp")
                    .or_else(|| rest.strip_suffix("/udp"))
                    .unwrap_or(rest);
                !ports.is_empty()
                    && ports
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b':' || b == b'-')
            });
            if !bound_to_loopback {
                return Err(format!(
                    "this package asks to publish \"{value}\", which is not allowed (ports bind 127.0.0.1 only)"
                ));
            }
        }
        _ => {} // -e/--env: a KEY=VALUE pair is the container's own business
    }
    Ok(())
}

/// The stock image an npm or pypi package runs in; an oci package brings its
/// own. One table, read by the spec parser and by the install's pull.
pub fn runtime_image(kind: &str) -> &'static str {
    match kind {
        "pypi" => "ghcr.io/astral-sh/uv:python3.12-bookworm-slim",
        _ => "node:22-slim",
    }
}

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

/// Does THIS api reach oci-http packages by container DNS on the package
/// network, rather than on the loopback port each one publishes?
///
/// Wherever the api itself runs in a container that shares the host's docker
/// daemon but not the host's network namespace, a published port binds the
/// HOST's loopback, and `127.0.0.1:<port>` dialled from in here is the
/// container's own: nothing answers. Two places do that:
///
///   - the product image (`TALARIA_INSTALL=image`, or the prod-server
///     runtime);
///   - a devbox: its api runs in the box container, whose compose sets
///     `TALARIA_DEVBOX` (docker/devbox.compose.yml). The same class as an app
///     database dialled from a box (F21, ui/src/server/app-db.ts).
///
/// There the api joins the package network, by its own `HOSTNAME` (docker's
/// default hostname is the container's id), and dials the package by name. A
/// host-run api sets none of these and keeps the loopback port.
fn dials_by_name() -> bool {
    let var = |k: &str| std::env::var(k).ok();
    dials_by_name_in(
        var("TALARIA_INSTALL").as_deref(),
        var("TALARIA_RUNTIME").as_deref(),
        var("TALARIA_DEVBOX").as_deref(),
    )
}

/// `dials_by_name`, over the three variables it reads. Pure for the tests.
fn dials_by_name_in(install: Option<&str>, runtime: Option<&str>, devbox: Option<&str>) -> bool {
    install == Some("image")
        || runtime == Some("prod-server")
        || devbox.is_some_and(|b| !b.is_empty())
}

/// The bridge network oci-http package containers live on — theirs alone,
/// per instance. Never the api's own `*_internal` network: that one carries
/// postgres, redis, qdrant and the rest, several without authentication,
/// and a package is third-party code.
pub fn pkg_network() -> String {
    format!("talaria-mcp-{}", instance())
}

/// Whose a package container or network is, as docker labels. Both are made
/// with plain `docker run` / `docker network create`, outside any compose
/// project, so no compose label says whose they are. A worktree removed, or
/// a box deleted without `box rm`, left them running and holding a subnet,
/// and the cleanup sweep, which places what it removes by compose project,
/// never saw them (follow-up UB3). `scripts/cleanup-sweep.mjs` reads these:
///
///   - `talaria.mcp.instance`: `instance()`, the name in the object's own;
///   - `talaria.mcp.owner`: `worktree`, `devbox`, or `instance` for the
///     primary and an image install, which the sweep never touches;
///   - `talaria.mcp.checkout`, a worktree's only: the checkout this api runs
///     from. The sweep removes a worktree's packages once that directory is
///     gone, and a box's once its `box.env` is.
pub const LABEL_INSTANCE: &str = "talaria.mcp.instance";
pub const LABEL_OWNER: &str = "talaria.mcp.owner";
pub const LABEL_CHECKOUT: &str = "talaria.mcp.checkout";

/// This api's owner labels, `key=value` each (`LABEL_INSTANCE` above).
pub fn owner_labels() -> Vec<String> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    owner_labels_in(
        &instance(),
        var("TALARIA_WORKTREE").as_deref(),
        var("TALARIA_DEVBOX").as_deref(),
        std::env::current_dir().ok().as_deref(),
    )
}

/// `owner_labels`, over what it reads. Pure for the tests. The owner follows
/// `instance()`'s order: a worktree's name first, then a box's. A worktree's
/// checkout is the api's cwd's parent only when that cwd is the checkout's
/// `api/` (where `talaria dev` runs it, and the rule `apps_dir` and
/// `fleet_dir` already read); from anywhere else no checkout is named, and
/// the sweep leaves the package alone.
fn owner_labels_in(
    instance: &str,
    worktree: Option<&str>,
    devbox: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> Vec<String> {
    let mut out = vec![format!("{LABEL_INSTANCE}={instance}")];
    if worktree.is_some() {
        out.push(format!("{LABEL_OWNER}=worktree"));
        let checkout = cwd
            .filter(|c| c.file_name().is_some_and(|n| n == "api"))
            .and_then(|c| c.parent());
        if let Some(dir) = checkout {
            out.push(format!("{LABEL_CHECKOUT}={}", dir.display()));
        }
    } else if devbox.is_some() {
        out.push(format!("{LABEL_OWNER}=devbox"));
    } else {
        out.push(format!("{LABEL_OWNER}=instance"));
    }
    out
}

/// `--label k=v` for each owner label, as docker's argv wants them.
fn label_args(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .flat_map(|l| ["--label".to_string(), l.clone()])
        .collect()
}

/// The package networks THIS api process has joined (`ensure_pkg_network`).
///
/// A package container outlives the container of the api that started it.
/// When a devbox's container is recreated (a `box start` after a template
/// change) or the image's api container is replaced, the package keeps
/// running and the new api container is on none of its networks, so a URL
/// by name does not resolve from it. The new container is a new process
/// here, with nothing in this set: its first relay to a running package
/// joins, and every later one costs no docker call.
fn joined_networks() -> &'static Mutex<HashSet<String>> {
    static JOINED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
    &JOINED
}

/// Does a relay to an already-running package have to join its network
/// first? Only an api that dials by name, and only once per process. Pure
/// for the tests.
fn join_before_relay(running: bool, by_name: bool, joined_here: bool) -> bool {
    running && by_name && !joined_here
}

/// Create the package network if it is missing, and — when this api runs in
/// a container (`dials_by_name`: the image, a devbox) — join it, so container
/// DNS resolves the package from here. The join is idempotent (`inspect`
/// first; a lost race on `connect` is re-checked the same way), and a joined
/// network is remembered for this process (`joined_networks`). On the host
/// nothing joins: the relay dials the loopback-published port.
async fn ensure_pkg_network() -> Result<String, String> {
    let name = pkg_network();
    let inspect = || async {
        talaria_fleet_docker::docker(&["network", "inspect", &name], Duration::from_secs(10)).await
    };
    if inspect().await.is_err() {
        // A create that loses a race fails; the re-inspect says which. The
        // owner labels are how the cleanup sweep places it (`owner_labels`).
        let mut args = vec!["network".to_string(), "create".into()];
        args.extend(label_args(&owner_labels()));
        args.extend(["--driver".into(), "bridge".into(), name.clone()]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let created = talaria_fleet_docker::docker(&args, Duration::from_secs(10)).await;
        if created.is_err() {
            inspect()
                .await
                .map_err(|e| format!("package network {name} unavailable: {e}"))?;
        }
    }
    if dials_by_name() {
        let self_id = std::env::var("HOSTNAME")
            .map_err(|_| "HOSTNAME is unset, so this api cannot name its own container")?;
        let joined = || async {
            talaria_fleet_docker::docker(
                &[
                    "inspect",
                    "-f",
                    "{{range $k,$v := .NetworkSettings.Networks}}{{$k}}\n{{end}}",
                    &self_id,
                ],
                Duration::from_secs(10),
            )
            .await
            .is_ok_and(|(out, _)| out.lines().map(str::trim).any(|n| n == name))
        };
        if !joined().await {
            let connected = talaria_fleet_docker::docker(
                &["network", "connect", &name, &self_id],
                Duration::from_secs(30),
            )
            .await;
            if connected.is_err() && !joined().await {
                return Err(format!(
                    "this api could not join the package network {name}: {}",
                    connected.err().unwrap_or_default()
                ));
            }
        }
        joined_networks()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.clone());
    }
    Ok(name)
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
            image: match obj.get("image").and_then(Value::as_str) {
                Some(i) if !i.is_empty() => i.to_string(),
                // npm and pypi packages run in the stock runtime image; only
                // an oci package names its own, and without one it is not a
                // spec at all.
                _ => match obj.get("kind").and_then(Value::as_str) {
                    Some("oci") | None => return None,
                    Some(kind) => runtime_image(kind).to_string(),
                },
            },
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

/// The unprivileged uid:gid every package runs as — `nobody` on every stock
/// image; not root anywhere, so a volume or a kernel surface the container
/// reaches is reached as nobody.
const PKG_USER: &str = "65534:65534";

/// The one writable path: HOME, where npx (`~/.npm`, `~/.npm/_npx`) and uv
/// (`~/.cache/uv`, `~/.local/share/uv/tools`) keep what they download.
/// Memory-backed and gone with the container — a cache that survives a
/// respawn would need a volume owned by uid 65534, which `docker volume`
/// cannot promise; a slower respawn is the price of a read-only root. Sized
/// under the memory ceiling (tmpfs pages count against it); `exec` stays
/// allowed because uv's tool venvs and npm's native binaries run from here.
const PKG_TMPFS: &str = "/tmp:rw,exec,nosuid,nodev,size=768m";

fn hardening(argv: &mut Vec<String>, dns: &[String]) {
    argv.push("--security-opt".into());
    argv.push("no-new-privileges:true".into());
    argv.push("--cap-drop".into());
    argv.push("ALL".into());
    argv.push("--user".into());
    argv.push(PKG_USER.into());
    argv.push("--read-only".into());
    argv.push("--tmpfs".into());
    argv.push(PKG_TMPFS.into());
    // Before the sealed env, so a package that declares its own HOME wins
    // (docker keeps the last spelling of a repeated -e).
    argv.push("-e".into());
    argv.push("HOME=/tmp".into());
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
///
/// `http_network` is the package network an oci-http package runs detached
/// on (ensure_http); None is a stdio package, run interactively on the
/// default bridge. The network is a docker FLAG, so it goes with the flags,
/// before the image: everything after the image is the package's own argv.
/// ensure_http used to append it to the finished argv, which handed
/// `--network talaria-mcp-<instance>` to the package's entry point and left
/// the container on the default bridge. A package that rejects unknown
/// arguments exited at once (node: `bad option: --network`), and one that
/// ignored them was never on the network the image's api dials it on.
///
/// `labels` are the owner labels (`owner_labels`), `key=value` each: docker
/// flags too, right after the name.
pub fn pkg_argv(
    spec: &PkgSpec,
    sealed: &SealedDoc,
    container: &str,
    dns: &[String],
    http_network: Option<&str>,
    labels: &[String],
) -> Result<Vec<String>, String> {
    let http_detached = http_network.is_some();
    let mut argv = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        container.to_string(),
    ];
    argv.extend(label_args(labels));
    if http_detached {
        argv.push("-d".into());
    } else {
        argv.push("-i".into());
    }
    hardening(&mut argv, dns);
    // npx/uv download their package into HOME (the tmpfs) on every spawn —
    // see PKG_TMPFS for why there is no cache volume.
    for (k, v) in &sealed.env {
        argv.push("-e".into());
        argv.push(format!("{k}={v}"));
    }
    // The registry's declared run flags — allowlisted NAMED flags with
    // allowlisted VALUES only (the admin's fill included); positionals are
    // appended after the image below.
    for (i, arg) in spec.run_args.iter().enumerate() {
        if arg.get("type").and_then(Value::as_str) != Some("named") {
            continue;
        }
        let flag = arg.get("name").and_then(Value::as_str).unwrap_or_default();
        let value = arg_value(arg, sealed, i);
        check_run_flag(flag, value.as_deref())?;
        let Some(v) = value else {
            continue; // declared but unfilled and unfillable — skip
        };
        argv.push(flag.to_string());
        argv.push(v);
    }
    if let Some(network) = http_network {
        // oci-http: the transport names its in-container port, published on
        // loopback for a host-run api; and the package runs on its own
        // network, never the api's internal one (see the header), where an
        // api in a container reaches it by name (ensure_http).
        if let Some(port) = spec.container_port {
            argv.push("-p".into());
            argv.push(format!("127.0.0.1::{port}"));
        }
        argv.push("--network".into());
        argv.push(network.to_string());
    }
    // The image (or stock runner) and its entry point. The runner images
    // carry their own ENTRYPOINT (uv's is `uv`), so the entry command rides
    // an explicit --entrypoint override rather than a prepended argv word.
    match spec.kind.as_str() {
        "npm" => {
            argv.push(runtime_image("npm").into());
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
            argv.push(runtime_image("pypi").into());
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
    let argv = pkg_argv(spec, sealed, &container, &dns, None, &owner_labels())?;
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

/// Ensure the detached oci-http container runs — on the package network —
/// and return the URL this api relays to (container DNS from the image or a
/// devbox, the loopback publish on the host; `dials_by_name`).
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
        // Its own network, never the api's internal one (see the header).
        let network = ensure_pkg_network().await?;
        let argv = pkg_argv(
            spec,
            &sealed,
            &container,
            &dns,
            Some(&network),
            &owner_labels(),
        )?;
        let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        talaria_fleet_docker::docker(&argv_refs, Duration::from_secs(120)).await?;
    } else if join_before_relay(
        running,
        dials_by_name(),
        joined_networks()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&pkg_network()),
    ) {
        // The package outlived the api container that started it
        // (`joined_networks`): join before dialling it by name.
        ensure_pkg_network().await?;
    }
    if dials_by_name() {
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
    fn npm_argv_runs_npx_in_a_hardened_child() {
        let spec = PkgSpec::of(&spec_json("npm", "stdio")).unwrap();
        let argv = pkg_argv(
            &spec,
            &sealed(&[("API_KEY", "sk-1")]),
            "talaria-mcp-talaria-x",
            DNS,
            None,
            &[],
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
        // Not root, read-only root, HOME on the one tmpfs — and no volume at
        // all: the cache lives in HOME and dies with the container.
        assert!(argv.windows(2).any(|w| w == ["--user", "65534:65534"]));
        assert!(argv.contains(&"--read-only".to_string()));
        assert!(argv.windows(2).any(|w| w == ["--tmpfs", PKG_TMPFS]));
        assert!(argv.windows(2).any(|w| w == ["-e", "HOME=/tmp"]));
        assert!(
            !argv
                .iter()
                .any(|a| a == "-v" || a == "--volume" || a == "--mount")
        );
        let at = argv.iter().position(|a| a == "node:22-slim").unwrap();
        assert_eq!(argv[at + 1], "npx");
        assert_eq!(argv[at + 3], "example-srv@1.2.3");
    }

    #[test]
    fn pypi_argv_pins_version_with_double_equals() {
        let spec = PkgSpec::of(&spec_json("pypi", "stdio")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, None, &[]).unwrap();
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
        assert!(argv.windows(2).any(|w| w == ["--user", "65534:65534"]));
        assert!(!argv.iter().any(|a| a == "-v" || a == "--volume"));
    }

    #[test]
    fn oci_http_runs_detached_and_publishes_the_declared_port() {
        let spec = PkgSpec::of(&spec_json("oci", "http")).unwrap();
        let argv = pkg_argv(
            &spec,
            &SealedDoc::default(),
            "c",
            DNS,
            Some("talaria-mcp-test"),
            &[],
        )
        .unwrap();
        assert!(argv.contains(&"-d".to_string()));
        assert!(!argv.contains(&"-i".to_string()));
        assert!(argv.windows(2).any(|w| w == ["-p", "127.0.0.1::8080"]));
        assert!(argv.contains(&"ghcr.io/example/srv".to_string()));
        // The network is a docker flag, so it precedes the image; anything
        // after the image is the package's own argv.
        let image = argv
            .iter()
            .position(|a| a == "ghcr.io/example/srv")
            .unwrap();
        let network = argv
            .windows(2)
            .position(|w| w == ["--network", "talaria-mcp-test"])
            .unwrap();
        assert!(network < image, "--network after the image: {argv:?}");
        assert!(!argv[image..].iter().any(|a| a == "--network"));
    }

    #[test]
    fn a_stdio_package_names_no_network() {
        let spec = PkgSpec::of(&spec_json("oci", "stdio")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, None, &[]).unwrap();
        assert!(!argv.iter().any(|a| a == "--network"));
        assert!(argv.contains(&"-i".to_string()));
    }

    #[test]
    fn oci_stdio_runs_the_pinned_image_interactively() {
        let spec = PkgSpec::of(&spec_json("oci", "stdio")).unwrap();
        let argv = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, None, &[]).unwrap();
        assert!(argv.contains(&"-i".to_string()));
        assert_eq!(argv.last().unwrap(), "ghcr.io/example/srv");
    }

    #[test]
    fn dangerous_flags_are_refused_and_safe_ones_pass() {
        let mut doc = json!({
            "kind": "oci", "identifier": "ghcr.io/x/y:v1", "transport": "stdio",
            "image": "ghcr.io/x/y", "runArgs": [
                { "type": "named", "name": "--privileged" },
                { "type": "named", "name": "-v", "default": "talaria-mcp-cfg:/cfg:ro" },
                { "type": "positional", "value": "--stdio" },
            ],
            "pull": { "state": "ready" },
        });
        let spec = PkgSpec::of(&doc).unwrap();
        let err = pkg_argv(&spec, &SealedDoc::default(), "c", DNS, None, &[]).unwrap_err();
        assert!(err.contains("--privileged"), "{err}");
        doc["runArgs"].as_array_mut().unwrap().remove(0);
        let spec = PkgSpec::of(&doc).unwrap();
        let argv = pkg_argv(&spec, &sealed(&[]), "c", DNS, None, &[]).unwrap();
        assert!(
            argv.windows(2)
                .any(|w| w == ["-v", "talaria-mcp-cfg:/cfg:ro"])
        );
        // The positional lands after the image, which is argv's last element
        // when no other positionals follow.
        assert_eq!(argv.last().unwrap(), "--stdio");
    }

    /// The rule, value by value: a volume is a `talaria-mcp-*` named volume
    /// or nothing; a publish binds loopback or nothing; `--mount` and
    /// `--add-host` have no allowed value at all.
    #[test]
    fn run_flag_values_are_gated_not_just_names() {
        // Volumes: named, instance-prefixed, absolute destination, known options.
        assert!(check_run_flag("-v", Some("talaria-mcp-data:/data")).is_ok());
        assert!(check_run_flag("--volume", Some("talaria-mcp-x-1:/x:ro,z")).is_ok());
        for bad in [
            "/:/host",
            "/var/run/docker.sock:/var/run/docker.sock",
            "./here:/x",
            "~/.ssh:/x",
            "talaria-mcp-:/x",
            "talaria-mcp-X:/x",
            "talaria-mcp-a:relative",
            "talaria-mcp-a:/x:exec",
            "talaria-mcp-a:/x:",
            "data:/data",
            "talaria-mcp-a",
            "talaria-mcp-a:/x:ro:extra",
            // A CA bundle from the host, the bind that made dockerd create
            // /etc/pki/tls/certs/ca-bundle.crt as a directory on a host
            // without one (follow-up UB4): a host path like any other.
            "/etc/pki/tls/certs/ca-bundle.crt:/etc/ssl/certs/ca-certificates.crt:ro",
            "/etc/ssl/certs:/etc/ssl/certs:ro",
        ] {
            let err = check_run_flag("-v", Some(bad)).unwrap_err();
            assert!(err.contains("not allowed"), "{bad} → {err}");
            assert!(check_run_flag("--volume", Some(bad)).is_err(), "{bad}");
        }
        // Mounts and hosts: refused by name, whatever the value.
        assert!(check_run_flag("--mount", Some("type=volume,src=talaria-mcp-a,dst=/a")).is_err());
        assert!(check_run_flag("--mount", None).is_err());
        assert!(
            check_run_flag(
                "--mount",
                Some("type=bind,src=/etc/pki/tls/certs/ca-bundle.crt,dst=/etc/ssl/cert.pem")
            )
            .is_err()
        );
        assert!(check_run_flag("--add-host", Some("host.docker.internal:host-gateway")).is_err());
        assert!(check_run_flag("--network", Some("host")).is_err());
        // Ports: loopback only.
        assert!(check_run_flag("-p", Some("127.0.0.1:9999:80")).is_ok());
        assert!(check_run_flag("--publish", Some("127.0.0.1::8080/tcp")).is_ok());
        assert!(check_run_flag("-p", Some("127.0.0.1:9000-9001:9000-9001")).is_ok());
        for bad in [
            "9999:80",
            "0.0.0.0:2222:2222",
            "127.0.0.1:",
            "127.0.0.1:x:80",
            "127.0.0.1:80/sctp",
            "[::1]:80:80",
        ] {
            assert!(check_run_flag("-p", Some(bad)).is_err(), "{bad}");
        }
        // Env: any pair; an unfilled declaration passes on its name alone.
        assert!(check_run_flag("-e", Some("KEY=value")).is_ok());
        assert!(check_run_flag("-v", None).is_ok());
        // The fill is gated like the declaration: an admin's value for a
        // volume placeholder cannot reach the host either.
        let doc = json!({
            "kind": "oci", "identifier": "ghcr.io/x/y:v1", "transport": "stdio",
            "image": "ghcr.io/x/y", "runArgs": [
                { "type": "named", "name": "-v", "placeholder": "the data volume" },
            ],
            "pull": { "state": "ready" },
        });
        let spec = PkgSpec::of(&doc).unwrap();
        let mut filled = SealedDoc::default();
        filled.args.insert(0, "/etc:/etc".to_string());
        let err = pkg_argv(&spec, &filled, "c", DNS, None, &[]).unwrap_err();
        assert!(err.contains("/etc:/etc"), "{err}");
        filled.args.insert(0, "talaria-mcp-data:/data".to_string());
        let argv = pkg_argv(&spec, &filled, "c", DNS, None, &[]).unwrap();
        assert!(
            argv.windows(2)
                .any(|w| w == ["-v", "talaria-mcp-data:/data"])
        );
    }

    #[test]
    fn filled_run_args_override_their_defaults_in_declared_order() {
        let doc = json!({
            "kind": "oci", "identifier": "ghcr.io/x/y:v1", "transport": "stdio",
            "image": "ghcr.io/x/y", "runArgs": [
                { "type": "named", "name": "-p", "default": "127.0.0.1:9999:80" },
                { "type": "positional", "value": "serve" },
                { "type": "positional", "placeholder": "the mount" },
            ],
            "pull": { "state": "ready" },
        });
        let spec = PkgSpec::of(&doc).unwrap();
        let mut sealed_doc = SealedDoc::default();
        sealed_doc.args.insert(2, "/data:/data".to_string());
        let argv = pkg_argv(&spec, &sealed_doc, "c", DNS, None, &[]).unwrap();
        assert!(argv.windows(2).any(|w| w == ["-p", "127.0.0.1:9999:80"]));
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

    #[test]
    fn a_running_package_is_joined_once_per_process_and_only_by_name() {
        // An api that dials by name, on a package started by an earlier
        // container of itself: it joins before the first relay.
        assert!(join_before_relay(true, true, false));
        // Joined already in this process: no docker call.
        assert!(!join_before_relay(true, true, true));
        // A host-run api dials the loopback port and never joins.
        assert!(!join_before_relay(true, false, false));
        // A package that is not running is started, which joins on its own.
        assert!(!join_before_relay(false, true, false));
    }

    #[test]
    fn package_urls_go_by_container_name_from_the_image_and_a_devbox_only() {
        // The product image, either spelling.
        assert!(dials_by_name_in(Some("image"), None, None));
        assert!(dials_by_name_in(None, Some("prod-server"), None));
        // A devbox: its compose sets TALARIA_DEVBOX to the box's name (an
        // older template set '1'). Its published loopback port is the host's.
        assert!(dials_by_name_in(None, None, Some("fixui-n8")));
        assert!(dials_by_name_in(None, None, Some("1")));
        // The host: none set, or set empty — the loopback port answers there.
        assert!(!dials_by_name_in(None, None, None));
        assert!(!dials_by_name_in(None, None, Some("")));
        assert!(!dials_by_name_in(Some("checkout"), Some("dev"), None));
    }

    // UB3: a package container or network names its owner, so the cleanup
    // sweep can remove it once that owner is gone.
    #[test]
    fn a_package_names_its_owner_in_labels() {
        use std::path::Path;
        let wt = owner_labels_in(
            "r6-ui",
            Some("r6-ui"),
            None,
            Some(Path::new("/home/u/talaria-r6-ui/api")),
        );
        assert_eq!(
            wt,
            [
                "talaria.mcp.instance=r6-ui",
                "talaria.mcp.owner=worktree",
                "talaria.mcp.checkout=/home/u/talaria-r6-ui",
            ]
        );
        // A worktree's name wins over a box's, as `instance()` reads them.
        assert_eq!(
            owner_labels_in(
                "r6-ui",
                Some("r6-ui"),
                Some("b1"),
                Some(Path::new("/w/t/api"))
            )[1],
            "talaria.mcp.owner=worktree"
        );
        // An api run from anywhere but a checkout's api/ names no checkout,
        // and the sweep then has nothing to place it by.
        assert_eq!(
            owner_labels_in("x", Some("x"), None, Some(Path::new("/tmp/elsewhere"))),
            ["talaria.mcp.instance=x", "talaria.mcp.owner=worktree"]
        );
        assert_eq!(owner_labels_in("x", Some("x"), None, None).len(), 2);
        // A box: its name is the instance, and box.env is what places it.
        assert_eq!(
            owner_labels_in("b1", None, Some("b1"), Some(Path::new("/work/talaria/api"))),
            ["talaria.mcp.instance=b1", "talaria.mcp.owner=devbox"]
        );
        // The primary and an image install: never the sweep's.
        assert_eq!(
            owner_labels_in(
                "talaria",
                None,
                None,
                Some(Path::new("/home/u/talaria/api"))
            ),
            ["talaria.mcp.instance=talaria", "talaria.mcp.owner=instance"]
        );
    }

    #[test]
    fn the_owner_labels_are_docker_flags_before_the_image() {
        let labels = owner_labels_in("b1", None, Some("b1"), None);
        for (kind, transport, network) in [
            ("npm", "stdio", None),
            ("pypi", "stdio", None),
            ("oci", "stdio", None),
            ("oci", "http", Some("talaria-mcp-b1")),
        ] {
            let spec = PkgSpec::of(&spec_json(kind, transport)).unwrap();
            let argv = pkg_argv(
                &spec,
                &SealedDoc::default(),
                "talaria-mcp-b1-x",
                DNS,
                network,
                &labels,
            )
            .unwrap();
            assert_eq!(
                &argv[..4],
                &["run", "--rm", "--name", "talaria-mcp-b1-x"][..]
            );
            assert_eq!(
                &argv[4..8],
                &[
                    "--label",
                    "talaria.mcp.instance=b1",
                    "--label",
                    "talaria.mcp.owner=devbox"
                ][..],
                "{kind}/{transport}: {argv:?}"
            );
            let image = argv
                .iter()
                .position(|a| {
                    a.as_str() == runtime_image(kind) || a.starts_with("ghcr.io/example/srv")
                })
                .unwrap();
            assert!(
                !argv[image..].iter().any(|a| a == "--label"),
                "{kind}/{transport}: {argv:?}"
            );
        }
    }

    fn test_box() -> SecretBox {
        let kek = talaria_secretbox::derive_kek("pkg-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
    }
}
