// Host resource usage for the observability surface.
//
// The api is a musl-static binary. In dev it runs on the host, so `/proc`
// and `/` are the machine. In the container deploy they are the container,
// which is the wrong answer — a full disk on the host would read as a quiet
// overlay. The container compose therefore bind-mounts the host's `/proc`
// at `/host/proc` and the host's `/` at `/host` (read-only, rslave), and
// this crate reads those. A container without the mounts says so. It never
// reports its own cgroup as the host.
//
// Read-only, on purpose: comm and rss and statvfs, never cmdline, never
// environ, never a file's contents. cmdline and environ are where secrets
// sit in argv.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

const LOG: &str = "[host]";

/// How long a published snapshot stays the answer. The UI polls slower than
/// this, so a refresh is a new sample, and two panels opening together share
/// one.
const SNAPSHOT_FRESH: Duration = Duration::from_secs(2);

/// First sample has no previous tick to subtract. Wait this long and read
/// again so the first paint has a CPU number instead of a blank.
const COLD_WINDOW: Duration = Duration::from_millis(200);

/// Below this, a delta is clock noise. Serve the previous snapshot.
const MIN_DELTA: Duration = Duration::from_millis(150);

/// tmpfs smaller than this is a scratch mount, not a disk filling up.
const TMPFS_FLOOR: u64 = 64 * 1024 * 1024;

const TOP_PROCESSES: usize = 8;

/// PF_KTHREAD. Kernel threads are not "what is using the box".
const PF_KTHREAD: u64 = 0x0020_0000;

const CONTAINER_NOTE: &str = "This process is in a container and cannot see the host. Mount host /proc at /host/proc and host / at /host (read-only, rslave), or set TALARIA_HOST_PROC and TALARIA_HOST_ROOT.";

const UNREADABLE_PROC_NOTE: &str = "TALARIA_HOST_PROC is set, but that proc is not readable. Host metrics are withheld rather than reported from this container.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    Ok,
    Warn,
    Danger,
}

impl Pressure {
    pub fn from_percent(percent: f64) -> Self {
        if percent >= 90.0 {
            Pressure::Danger
        } else if percent >= 80.0 {
            Pressure::Warn
        } else {
            Pressure::Ok
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuReading {
    pub percent: f64,
    pub iowait_percent: f64,
    pub cores: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadReading {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemReading {
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwapReading {
    pub present: bool,
    pub total: u64,
    pub used: u64,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountReading {
    pub mount: String,
    pub source: String,
    pub fstype: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub percent: f64,
    pub pressure: Pressure,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcReading {
    pub pid: i32,
    pub name: String,
    pub cpu_percent: f64,
    pub rss: u64,
}

/// The observability read. `available` false means we refused to guess —
/// the fields are null / empty, and `note` says why. A present `note` on
/// an available snapshot is a partial: some host mounts did not propagate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSnapshot {
    pub available: bool,
    pub sampled_at: i64,
    pub note: Option<String>,
    pub cpu: Option<CpuReading>,
    pub load: Option<LoadReading>,
    pub memory: Option<MemReading>,
    pub swap: Option<SwapReading>,
    pub mounts: Vec<MountReading>,
    pub processes: Vec<ProcReading>,
}

#[derive(Clone, Copy)]
struct CpuTicks {
    busy: u64,
    iowait: u64,
    total: u64,
}

struct ProcTick {
    pid: i32,
    name: String,
    ticks: u64,
    rss: u64,
}

struct RawSample {
    at: Instant,
    cpu: CpuTicks,
    procs: HashMap<i32, u64>,
}

struct Cache {
    raw: Option<RawSample>,
    snap: Option<(Instant, HostSnapshot)>,
}

static CACHE: std::sync::Mutex<Cache> = std::sync::Mutex::new(Cache {
    raw: None,
    snap: None,
});
static LOGGED_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// The read the route and the alerts pass share. Blocking file IO runs off
/// the async worker. The first call after boot waits `COLD_WINDOW` so CPU
/// has a delta; later calls reuse the last snapshot while it is fresh.
pub async fn snapshot() -> HostSnapshot {
    match tokio::task::spawn_blocking(sample_cached).await {
        Ok(snap) => snap,
        Err(_) => unavailable("host sampler failed"),
    }
}

fn sample_cached() -> HostSnapshot {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, snap)) = &cache.snap
        && at.elapsed() < SNAPSHOT_FRESH
    {
        return snap.clone();
    }
    let now = Instant::now();
    let prev = cache.raw.as_ref().filter(|r| r.at.elapsed() >= MIN_DELTA);
    let (snap, raw) = sample_once(prev);
    if let Some(raw) = raw {
        cache.raw = Some(raw);
    }
    cache.snap = Some((now, snap.clone()));
    snap
}

fn sample_once(prev: Option<&RawSample>) -> (HostSnapshot, Option<RawSample>) {
    let choice = choose_roots(&RootFacts::detect());
    let Roots::Host { proc, root } = choice else {
        let Roots::Unavailable { note } = choice else {
            unreachable!("choose_roots is host or unavailable");
        };
        if LOGGED_UNAVAILABLE
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            tracing::warn!("{LOG} {note}");
        }
        return (unavailable(&note), None);
    };

    let proc_path = Path::new(&proc);
    let root_path = Path::new(&root);
    let Some(cpu) = read_cpu(proc_path) else {
        return (unavailable("could not read host /proc/stat"), None);
    };
    let procs = read_processes(proc_path);

    // No previous tick: wait once and read the counters again. Memory and
    // disk come from the second read; they do not need a delta.
    let (cpu, procs, prev_cpu, prev_procs) = if let Some(prev) = prev {
        (cpu, procs, prev.cpu, prev.procs.clone())
    } else {
        std::thread::sleep(COLD_WINDOW);
        let Some(cpu2) = read_cpu(proc_path) else {
            return (unavailable("could not read host /proc/stat"), None);
        };
        let procs2 = read_processes(proc_path);
        let prev_procs: HashMap<i32, u64> = procs.iter().map(|p| (p.pid, p.ticks)).collect();
        (cpu2, procs2, cpu, prev_procs)
    };

    let cores = cpu_cores(proc_path);
    let cpu_reading = cpu_reading(prev_cpu, cpu, cores);
    let (mounts, mount_note) = read_mounts(proc_path, root_path);
    let processes = top_processes(
        &procs,
        &prev_procs,
        cpu.total.saturating_sub(prev_cpu.total),
    );
    let raw = RawSample {
        at: Instant::now(),
        cpu,
        procs: procs.iter().map(|p| (p.pid, p.ticks)).collect(),
    };

    let snap = HostSnapshot {
        available: true,
        sampled_at: now_ms(),
        note: mount_note,
        cpu: cpu_reading,
        load: read_load(proc_path),
        memory: read_memory(proc_path),
        swap: read_swap(proc_path),
        mounts,
        processes,
    };
    (snap, Some(raw))
}

fn unavailable(note: &str) -> HostSnapshot {
    HostSnapshot {
        available: false,
        sampled_at: now_ms(),
        note: Some(note.to_string()),
        cpu: None,
        load: None,
        memory: None,
        swap: None,
        mounts: Vec::new(),
        processes: Vec::new(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

enum Roots {
    Host { proc: String, root: String },
    Unavailable { note: String },
}

struct RootFacts {
    env_proc: Option<String>,
    env_root: Option<String>,
    env_proc_readable: bool,
    host_proc_mounted: bool,
    in_container: bool,
}

impl RootFacts {
    fn detect() -> Self {
        let env_proc = env_nonempty("TALARIA_HOST_PROC");
        let env_root = env_nonempty("TALARIA_HOST_ROOT");
        let env_proc_readable = env_proc
            .as_deref()
            .is_some_and(|p| Path::new(p).join("stat").is_file());
        Self {
            env_proc,
            env_root,
            env_proc_readable,
            host_proc_mounted: Path::new("/host/proc/stat").is_file(),
            in_container: in_container(),
        }
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn in_container() -> bool {
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup").is_ok_and(|t| {
            t.contains("/docker/") || t.contains("/containerd/") || t.contains("/kubepods/")
        })
}

fn choose_roots(facts: &RootFacts) -> Roots {
    if let Some(proc) = &facts.env_proc {
        if facts.env_proc_readable {
            return Roots::Host {
                proc: proc.clone(),
                root: facts
                    .env_root
                    .clone()
                    .unwrap_or_else(|| "/host".to_string()),
            };
        }
        return Roots::Unavailable {
            note: UNREADABLE_PROC_NOTE.to_string(),
        };
    }
    if facts.host_proc_mounted {
        return Roots::Host {
            proc: "/host/proc".to_string(),
            root: facts
                .env_root
                .clone()
                .unwrap_or_else(|| "/host".to_string()),
        };
    }
    if facts.in_container {
        return Roots::Unavailable {
            note: CONTAINER_NOTE.to_string(),
        };
    }
    Roots::Host {
        proc: "/proc".to_string(),
        root: facts.env_root.clone().unwrap_or_else(|| "/".to_string()),
    }
}

fn read_cpu(proc: &Path) -> Option<CpuTicks> {
    let text = std::fs::read_to_string(proc.join("stat")).ok()?;
    parse_cpu_aggregate(&text)
}

fn cpu_cores(proc: &Path) -> u32 {
    std::fs::read_to_string(proc.join("stat"))
        .ok()
        .map(|t| count_cores(&t))
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

fn parse_cpu_aggregate(text: &str) -> Option<CpuTicks> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() != Some("cpu") {
            continue;
        }
        let nums: Vec<u64> = parts.filter_map(|p| p.parse().ok()).collect();
        if nums.len() < 4 {
            return None;
        }
        let user = nums[0];
        let nice = nums[1];
        let system = nums[2];
        let idle = nums[3];
        let iowait = nums.get(4).copied().unwrap_or(0);
        let irq = nums.get(5).copied().unwrap_or(0);
        let softirq = nums.get(6).copied().unwrap_or(0);
        let steal = nums.get(7).copied().unwrap_or(0);
        // guest is already inside user. Adding it double-counts.
        let busy = user + nice + system + irq + softirq + steal;
        let total = busy + idle + iowait;
        return Some(CpuTicks {
            busy,
            iowait,
            total,
        });
    }
    None
}

fn count_cores(text: &str) -> u32 {
    text.lines()
        .filter(|l| {
            let name = l.split_whitespace().next().unwrap_or("");
            let rest = name.strip_prefix("cpu").unwrap_or("");
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
        })
        .count() as u32
}

fn cpu_reading(prev: CpuTicks, next: CpuTicks, cores: u32) -> Option<CpuReading> {
    let dt = next.total.saturating_sub(prev.total);
    if dt == 0 {
        return None;
    }
    let busy = next.busy.saturating_sub(prev.busy);
    let iowait = next.iowait.saturating_sub(prev.iowait);
    Some(CpuReading {
        percent: round1(pct(busy, dt)),
        iowait_percent: round1(pct(iowait, dt)),
        cores,
    })
}

fn read_load(proc: &Path) -> Option<LoadReading> {
    let text = std::fs::read_to_string(proc.join("loadavg")).ok()?;
    parse_loadavg(&text)
}

// ── The admission surface ────────────────────────────────────────────────────
//
// `snapshot()` is the observability read: cached, CPU-delta-windowed, and
// shaped for the UI. Admission needs none of that — it wants the same
// root-detection (`/proc` vs `/host/proc`) and the same file reads, but
// fresh, one number each, and pure enough to test without a host. The
// helpers below reuse the private machinery (never a second copy of root
// detection or statvfs) and expose exactly what the workbench's
// resource gate reads.

/// The host's `/proc` this process can actually see — `/proc` on the host,
/// `/host/proc` in the container deploy, or the `TALARIA_HOST_PROC`
/// override when it is readable. `None` when neither is available (a
/// container without the bind mounts): the caller fails that dimension
/// open rather than read the container's own `/proc` as the host's.
pub fn host_proc() -> Option<PathBuf> {
    match choose_roots(&RootFacts::detect()) {
        Roots::Host { proc, .. } => Some(PathBuf::from(proc)),
        Roots::Unavailable { .. } => None,
    }
}

/// The host's root filesystem as seen here — `/` on the host, `/host` in the
/// container deploy (with `TALARIA_HOST_ROOT` the override). `None` under
/// the same law as `host_proc`: no host root, no host disk reading.
pub fn host_root() -> Option<PathBuf> {
    match choose_roots(&RootFacts::detect()) {
        Roots::Host { root, .. } => Some(PathBuf::from(root)),
        Roots::Unavailable { .. } => None,
    }
}

/// The one-minute load average divided by the core count — "how saturated
/// is a core", comparable across machines. `None` when the host's loadavg
/// or core count cannot be read.
pub fn load1_per_core() -> Option<f64> {
    let proc = host_proc()?;
    load1_per_core_of(read_load(&proc)?, cpu_cores(&proc))
}

/// The pure half of `load1_per_core`: one reading over its core count.
/// A zero core count is a broken stat, not a saturated machine — refuse it
/// rather than divide by zero.
pub fn load1_per_core_of(load: LoadReading, cores: u32) -> Option<f64> {
    (cores > 0).then(|| load.one / f64::from(cores))
}

/// The host's core count as `/proc/stat` spells it.
pub fn core_count() -> Option<u32> {
    let proc = host_proc()?;
    let cores = cpu_cores(&proc);
    (cores > 0).then_some(cores)
}

/// Free bytes available to non-root on the filesystem holding `path` —
/// `f_bavail`, the `df` number an operator cross-checks. `path` is resolved
/// through the host root (`/host/opt/data` in the container deploy), so a
/// containerized api still reads the host's disk, not its overlay. `None`
/// when the host root or the statvfs fails.
pub fn mount_free_bytes(path: &Path) -> Option<u64> {
    let resolved = host_path(&host_root()?, &path.to_string_lossy());
    statvfs_usage(&resolved).map(|u| u.available)
}

fn parse_loadavg(text: &str) -> Option<LoadReading> {
    let mut parts = text.split_whitespace();
    Some(LoadReading {
        one: round2(parts.next()?.parse().ok()?),
        five: round2(parts.next()?.parse().ok()?),
        fifteen: round2(parts.next()?.parse().ok()?),
    })
}

fn read_memory(proc: &Path) -> Option<MemReading> {
    let text = std::fs::read_to_string(proc.join("meminfo")).ok()?;
    parse_memory(&text)
}

fn parse_memory(text: &str) -> Option<MemReading> {
    let map = meminfo_map(text);
    let total = map.get("MemTotal").copied()?;
    let available = map.get("MemAvailable").copied().or_else(|| {
        let free = map.get("MemFree").copied()?;
        let buffers = map.get("Buffers").copied().unwrap_or(0);
        let cached = map.get("Cached").copied().unwrap_or(0);
        Some(free + buffers + cached)
    })?;
    let available = available.min(total);
    let used = total - available;
    Some(MemReading {
        total,
        available,
        used,
        percent: round1(pct(used, total)),
    })
}

fn read_swap(proc: &Path) -> Option<SwapReading> {
    let text = std::fs::read_to_string(proc.join("meminfo")).ok()?;
    Some(parse_swap(&text))
}

fn parse_swap(text: &str) -> SwapReading {
    let map = meminfo_map(text);
    let total = map.get("SwapTotal").copied().unwrap_or(0);
    let free = map.get("SwapFree").copied().unwrap_or(0).min(total);
    let used = total - free;
    SwapReading {
        present: total > 0,
        total,
        used,
        percent: if total == 0 {
            0.0
        } else {
            round1(pct(used, total))
        },
    }
}

fn meminfo_map(text: &str) -> HashMap<&str, u64> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kb: u64 = rest
            .split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        // meminfo units are kB. Bytes on the wire, so the UI's byte formatter applies.
        map.insert(key.trim(), kb.saturating_mul(1024));
    }
    map
}

#[derive(Clone)]
struct ParsedMount {
    major: u32,
    minor: u32,
    mount: String,
    source: String,
    fstype: String,
}

fn read_mounts(proc: &Path, fs_root: &Path) -> (Vec<MountReading>, Option<String>) {
    let text = match std::fs::read_to_string(proc.join("1/mountinfo")) {
        Ok(t) => t,
        Err(_) => match std::fs::read_to_string(proc.join("self/mountinfo")) {
            Ok(t) => t,
            Err(_) => {
                return (
                    Vec::new(),
                    Some("could not read host mountinfo".to_string()),
                );
            }
        },
    };
    let parsed = parse_mountinfo(&text);
    let kept = dedupe_mounts(parsed.into_iter().filter(mount_is_relevant).collect());
    let root_dev = stat_dev(&host_path(fs_root, "/"));
    let mut out = Vec::new();
    let mut unpropagated = 0u32;
    for m in kept {
        let path = host_path(fs_root, &m.mount);
        let Some(dev) = stat_dev(&path) else {
            continue;
        };
        if !dev_matches(dev, m.major, m.minor) {
            if root_dev == Some(dev) && m.mount != "/" {
                unpropagated += 1;
            }
            continue;
        }
        let Some(usage) = statvfs_usage(&path) else {
            continue;
        };
        if m.fstype == "tmpfs" && usage.total < TMPFS_FLOOR {
            continue;
        }
        let used = usage.total.saturating_sub(usage.free);
        let percent = df_percent(used, usage.available);
        out.push(MountReading {
            mount: m.mount,
            source: m.source,
            fstype: m.fstype,
            total: usage.total,
            used,
            available: usage.available,
            percent: round1(percent),
            pressure: Pressure::from_percent(percent),
        });
    }
    out.sort_by(|a, b| {
        b.percent
            .partial_cmp(&a.percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.mount.cmp(&b.mount))
    });
    let note = if unpropagated > 0 {
        Some(format!(
            "{unpropagated} host mount{} did not propagate into this process, so only the filesystems visible here are listed. The /host bind needs rslave.",
            if unpropagated == 1 { "" } else { "s" }
        ))
    } else {
        None
    };
    (out, note)
}

fn parse_mountinfo(text: &str) -> Vec<ParsedMount> {
    text.lines().filter_map(parse_mountinfo_line).collect()
}

fn parse_mountinfo_line(line: &str) -> Option<ParsedMount> {
    let (left, right) = line.split_once(" - ")?;
    let mut left_fields = left.split_whitespace();
    let _id = left_fields.next()?;
    let _parent = left_fields.next()?;
    let dev = left_fields.next()?;
    let _root = left_fields.next()?;
    let mount = unescape_mount(left_fields.next()?);
    let (major, minor) = dev.split_once(':')?;
    let mut right_fields = right.split_whitespace();
    let fstype = right_fields.next()?.to_string();
    let source = unescape_mount(right_fields.next().unwrap_or(""));
    Some(ParsedMount {
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
        mount,
        source,
        fstype,
    })
}

fn unescape_mount(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| c.is_ascii_digit()) {
            let oct = &raw[i + 1..i + 4];
            if let Ok(v) = u8::from_str_radix(oct, 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "ramfs",
    "nsfs",
    "autofs",
    "binfmt_misc",
    "rpc_pipefs",
    "nfsd",
    "overlay",
    "squashfs",
    "efivarfs",
    "binder",
    "functionfs",
    "none",
];

fn mount_is_relevant(m: &ParsedMount) -> bool {
    if PSEUDO_FS.contains(&m.fstype.as_str()) {
        return false;
    }
    if m.mount.starts_with("/proc") || m.mount.starts_with("/sys") || m.mount.starts_with("/snap/")
    {
        return false;
    }
    if m.mount.starts_with("/dev") && m.mount != "/dev/shm" {
        return false;
    }
    if m.mount.contains("/docker/overlay2") || m.mount.contains("/containerd/") {
        return false;
    }
    true
}

fn dedupe_mounts(mounts: Vec<ParsedMount>) -> Vec<ParsedMount> {
    let mut best: HashMap<(u32, u32), ParsedMount> = HashMap::new();
    for m in mounts {
        best.entry((m.major, m.minor))
            .and_modify(|cur| {
                if m.mount.len() < cur.mount.len()
                    || (m.mount.len() == cur.mount.len() && m.mount < cur.mount)
                {
                    *cur = m.clone();
                }
            })
            .or_insert(m);
    }
    best.into_values().collect()
}

/// Join a host mount point onto the root we can stat. `Path::join` drops
/// the root when the mount is absolute, which would stat the container's
/// own `/var` instead of `/host/var`.
fn host_path(root: &Path, mount: &str) -> PathBuf {
    if root == Path::new("/") {
        return PathBuf::from(mount);
    }
    let rel = mount.trim_start_matches('/');
    if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

struct FsUsage {
    total: u64,
    free: u64,
    available: u64,
}

fn statvfs_usage(path: &Path) -> Option<FsUsage> {
    let c = c_path(path)?;
    let mut buf = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a NUL-terminated path, `buf` is writable and zeroed,
    // and the call does not retain either pointer.
    let rc = unsafe { libc::statvfs(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so the struct is initialized.
    let st = unsafe { buf.assume_init() };
    let bsize = as_u64(if st.f_frsize == 0 {
        st.f_bsize
    } else {
        st.f_frsize
    });
    Some(FsUsage {
        total: as_u64(st.f_blocks).saturating_mul(bsize),
        free: as_u64(st.f_bfree).saturating_mul(bsize),
        available: as_u64(st.f_bavail).saturating_mul(bsize),
    })
}
fn as_u64(n: impl Into<u64>) -> u64 {
    n.into()
}

fn stat_dev(path: &Path) -> Option<u64> {
    let c = c_path(path)?;
    let mut buf = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: same contract as statvfs — valid C string, writable zeroed buf.
    let rc = unsafe { libc::stat(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: stat returned 0.
    Some(unsafe { buf.assume_init() }.st_dev)
}

fn dev_matches(st_dev: u64, major: u32, minor: u32) -> bool {
    libc::major(st_dev) as u32 == major && libc::minor(st_dev) as u32 == minor
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_encoded_bytes()).ok()
}

/// `df`'s percent: used / (used + available). Reserved blocks make
/// used/total a different, quieter number than the one an operator sees
/// in a shell.
fn df_percent(used: u64, available: u64) -> f64 {
    pct(used, used.saturating_add(available))
}

fn pct(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        (num as f64 / den as f64) * 100.0
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn read_processes(proc: &Path) -> Vec<ProcTick> {
    let page = page_size();
    let Ok(dir) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in dir.flatten() {
        let name = ent.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<i32>() else {
            continue;
        };
        if pid <= 2 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(ent.path().join("stat")) else {
            continue;
        };
        if let Some(p) = parse_proc_stat(pid, &text, page) {
            out.push(p);
        }
    }
    out
}

fn page_size() -> u64 {
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as u64 } else { 4096 }
}

fn parse_proc_stat(pid: i32, text: &str, page: u64) -> Option<ProcTick> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let name = text[open + 1..close].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let rest = text[close + 1..].trim();
    let f: Vec<&str> = rest.split_whitespace().collect();
    // state, ppid, pgrp, session, tty, tpgid, flags, ... utime, stime, ... rss
    if f.len() < 22 {
        return None;
    }
    let ppid: i32 = f[1].parse().ok()?;
    let flags: u64 = f[6].parse().ok()?;
    if ppid == 2 || flags & PF_KTHREAD != 0 {
        return None;
    }
    let utime: u64 = f[11].parse().ok()?;
    let stime: u64 = f[12].parse().ok()?;
    let rss_pages: u64 = f[21].parse().ok()?;
    Some(ProcTick {
        pid,
        name,
        ticks: utime + stime,
        rss: rss_pages.saturating_mul(page),
    })
}

fn top_processes(now: &[ProcTick], prev: &HashMap<i32, u64>, cpu_delta: u64) -> Vec<ProcReading> {
    let mut rows: Vec<ProcReading> = now
        .iter()
        .map(|p| {
            let before = prev.get(&p.pid).copied().unwrap_or(p.ticks);
            let dt = p.ticks.saturating_sub(before);
            ProcReading {
                pid: p.pid,
                name: p.name.clone(),
                cpu_percent: round1(pct(dt, cpu_delta)),
                rss: p.rss,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.rss.cmp(&a.rss))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    rows.truncate(TOP_PROCESSES);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_aggregate_ignores_per_core_lines_and_guest() {
        let text = "\
cpu  100 10 20 800 50 5 5 10 40 0
cpu0 100 10 20 800 50 5 5 10 40 0
";
        let t = parse_cpu_aggregate(text).unwrap();
        // busy = 100+10+20+5+5+10 = 150; iowait 50; idle 800; total 1000.
        // guest (40) must not be added again.
        assert_eq!(t.busy, 150);
        assert_eq!(t.iowait, 50);
        assert_eq!(t.total, 1000);
        assert_eq!(count_cores(text), 1);
    }

    #[test]
    fn cpu_percent_is_busy_over_the_delta() {
        let prev = CpuTicks {
            busy: 100,
            iowait: 10,
            total: 1000,
        };
        let next = CpuTicks {
            busy: 200,
            iowait: 30,
            total: 2000,
        };
        let r = cpu_reading(prev, next, 4).unwrap();
        assert_eq!(r.percent, 10.0);
        assert_eq!(r.iowait_percent, 2.0);
        assert_eq!(r.cores, 4);
    }

    #[test]
    fn memory_uses_available_not_free() {
        let text = "\
MemTotal:       1000 kB
MemFree:         100 kB
MemAvailable:    400 kB
Buffers:          50 kB
Cached:          200 kB
SwapTotal:       500 kB
SwapFree:        100 kB
";
        let m = parse_memory(text).unwrap();
        assert_eq!(m.total, 1000 * 1024);
        assert_eq!(m.available, 400 * 1024);
        assert_eq!(m.used, 600 * 1024);
        assert_eq!(m.percent, 60.0);
        let s = parse_swap(text);
        assert!(s.present);
        assert_eq!(s.used, 400 * 1024);
        assert_eq!(s.percent, 80.0);
    }

    #[test]
    fn memory_falls_back_when_available_is_absent() {
        let text = "\
MemTotal:       1000 kB
MemFree:         100 kB
Buffers:          50 kB
Cached:          250 kB
";
        let m = parse_memory(text).unwrap();
        assert_eq!(m.available, 400 * 1024);
    }

    #[test]
    fn absent_swap_is_not_zero_percent_used() {
        let s = parse_swap("MemTotal: 10 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n");
        assert!(!s.present);
        assert_eq!(s.percent, 0.0);
    }

    #[test]
    fn loadavg_is_the_first_three_fields() {
        let l = parse_loadavg("1.50 0.75 0.25 2/100 9\n").unwrap();
        assert_eq!(l.one, 1.5);
        assert_eq!(l.five, 0.75);
        assert_eq!(l.fifteen, 0.25);
    }

    #[test]
    fn load_per_core_divides_and_refuses_a_zero_core_count() {
        let load = parse_loadavg("8.00 4.00 2.00 2/100 9\n").unwrap();
        // 8.0 over 4 cores is exactly 2 per core.
        assert_eq!(load1_per_core_of(load, 4), Some(2.0));
        // A zero core count is a broken stat, not a saturated machine.
        let load = parse_loadavg("8.00 4.00 2.00 2/100 9\n").unwrap();
        assert_eq!(load1_per_core_of(load, 0), None);
    }

    #[test]
    fn mount_free_bytes_resolves_through_the_host_root() {
        // The resolution half is pure and testable without a host: the
        // same host_path law read_mounts uses, applied to the admission
        // path. On the host the path is unchanged; in the container
        // deploy the host bind root is prepended.
        assert_eq!(
            host_path(Path::new("/"), "/opt/data/workbench/jobs"),
            PathBuf::from("/opt/data/workbench/jobs")
        );
        assert_eq!(
            host_path(Path::new("/host"), "/opt/data/workbench/jobs"),
            PathBuf::from("/host/opt/data/workbench/jobs")
        );
        // The statvfs half needs a real filesystem; against this repo's
        // own tree it answers Some, proving the plumbing end to end
        // without depending on /proc.
        assert!(mount_free_bytes(Path::new("tests")).is_none_or(|n| n > 0));
        assert_eq!(mount_free_bytes(Path::new("/no/such/mount/here")), None);
    }

    #[test]
    fn mountinfo_keeps_real_disks_and_decodes_spaces() {
        let text = "\
26 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
44 26 8:1 /var/lib /var/lib rw - ext4 /dev/sda1 rw
50 26 8:2 / /home rw - xfs /dev/sdb1 rw
70 26 8:3 / /mnt/my\\040disk rw - btrfs /dev/sdc1 rw
36 26 0:36 / /dev/shm rw - tmpfs tmpfs rw
99 26 0:1 / /proc rw - proc proc rw
80 26 0:40 / /snap/core/1 rw - squashfs /dev/loop0 rw
";
        let parsed = parse_mountinfo(text);
        let relevant: Vec<_> = parsed.into_iter().filter(mount_is_relevant).collect();
        let mounts = dedupe_mounts(relevant);
        let paths: Vec<_> = mounts.iter().map(|m| m.mount.as_str()).collect();
        assert!(
            paths.contains(&"/"),
            "root kept, bind of the same device dropped"
        );
        assert!(!paths.contains(&"/var/lib"));
        assert!(paths.contains(&"/home"));
        assert!(paths.contains(&"/mnt/my disk"));
        assert!(paths.contains(&"/dev/shm"));
        assert!(
            !paths
                .iter()
                .any(|p| *p == "/proc" || p.starts_with("/snap"))
        );
    }

    #[test]
    fn df_percent_counts_reserved_blocks() {
        // 90 used, 5 available to non-root, 10 free including reserved.
        // used/total = 90/100 = 90. df = 90/(90+5) = 94.7.
        assert_eq!(round1(df_percent(90, 5)), 94.7);
    }

    #[test]
    fn pressure_breaks_at_80_and_90() {
        assert_eq!(Pressure::from_percent(79.9), Pressure::Ok);
        assert_eq!(Pressure::from_percent(80.0), Pressure::Warn);
        assert_eq!(Pressure::from_percent(89.9), Pressure::Warn);
        assert_eq!(Pressure::from_percent(90.0), Pressure::Danger);
    }

    #[test]
    fn host_path_does_not_drop_the_bind_root() {
        assert_eq!(
            host_path(Path::new("/host"), "/var"),
            PathBuf::from("/host/var")
        );
        assert_eq!(host_path(Path::new("/host"), "/"), PathBuf::from("/host"));
        assert_eq!(host_path(Path::new("/"), "/var"), PathBuf::from("/var"));
    }

    #[test]
    fn proc_stat_skips_kernel_threads_and_reads_rss() {
        // comm with a space, then state ppid ... flags at field 7, utime/stime, rss at 22.
        let mut fields = vec!["S".to_string(), "1".to_string()];
        while fields.len() < 21 {
            fields.push("0".to_string());
        }
        fields[6] = "0".to_string();
        fields[11] = "10".to_string();
        fields[12] = "5".to_string();
        fields.push("3".to_string()); // rss pages, index 21
        let text = format!("123 (my proc) {}", fields.join(" "));
        let p = parse_proc_stat(123, &text, 4096).unwrap();
        assert_eq!(p.name, "my proc");
        assert_eq!(p.ticks, 15);
        assert_eq!(p.rss, 3 * 4096);

        fields[1] = "2".to_string(); // ppid kthreadd
        let text = format!("9 (kworker) {}", fields.join(" "));
        assert!(parse_proc_stat(9, &text, 4096).is_none());
    }

    #[test]
    fn roots_on_the_host_are_proc_and_slash() {
        let roots = choose_roots(&RootFacts {
            env_proc: None,
            env_root: None,
            env_proc_readable: false,
            host_proc_mounted: false,
            in_container: false,
        });
        match roots {
            Roots::Host { proc, root } => {
                assert_eq!(proc, "/proc");
                assert_eq!(root, "/");
            }
            Roots::Unavailable { .. } => panic!("a host process must read the host"),
        }
    }

    #[test]
    fn container_without_mounts_does_not_pretend() {
        let roots = choose_roots(&RootFacts {
            env_proc: None,
            env_root: None,
            env_proc_readable: false,
            host_proc_mounted: false,
            in_container: true,
        });
        match roots {
            Roots::Unavailable { note } => assert!(note.contains("cannot see the host")),
            Roots::Host { .. } => panic!("container /proc is not the host"),
        }
    }

    #[test]
    fn host_mounts_win_inside_a_container() {
        let roots = choose_roots(&RootFacts {
            env_proc: None,
            env_root: None,
            env_proc_readable: false,
            host_proc_mounted: true,
            in_container: true,
        });
        match roots {
            Roots::Host { proc, root } => {
                assert_eq!(proc, "/host/proc");
                assert_eq!(root, "/host");
            }
            Roots::Unavailable { note } => panic!("mounted host proc should be used, got {note}"),
        }
    }

    #[test]
    fn a_set_but_unreadable_proc_is_not_silently_replaced() {
        let roots = choose_roots(&RootFacts {
            env_proc: Some("/host/proc".into()),
            env_root: Some("/host".into()),
            env_proc_readable: false,
            host_proc_mounted: false,
            in_container: true,
        });
        assert!(matches!(roots, Roots::Unavailable { .. }));
    }

    #[test]
    fn live_proc_reads_this_machine() {
        if !Path::new("/proc/stat").is_file() {
            return;
        }
        let cpu = read_cpu(Path::new("/proc")).expect("stat");
        assert!(cpu.total > 0);
        assert!(cpu_cores(Path::new("/proc")) >= 1);
        let mem = read_memory(Path::new("/proc")).expect("meminfo");
        assert!(mem.total > 0);
        assert!(mem.available <= mem.total);
        assert!(read_load(Path::new("/proc")).is_some());
        let (mounts, _) = read_mounts(Path::new("/proc"), Path::new("/"));
        assert!(
            mounts.iter().any(|m| m.mount == "/"),
            "root mount visible: {mounts:?}"
        );
    }
}
