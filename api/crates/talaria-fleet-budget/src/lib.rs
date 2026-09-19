// Agent resource budget — packing, not a per-desk job cap.
//
// The 2026-09-17 outcrop freeze was eleven concurrent coding jobs inside ONE
// 4 GiB container: vite, Chrome, tsserver, Playwright, then 26 OOM kills.
// The first fix was a hard 3-job / 3-session cap plus an 8g workbench
// mem_limit. That stopped the OOMs by stalling work — every extra ticket
// waited a minute, then another, while the box sat idle beside them.
//
// THIS MODULE is the replacement. Hard cgroup ceilings stay as last-ditch
// host protection (a leak must not eat Postgres). What an agent may START
// is packed against host MemAvailable, and the running container's
// reservation is resized to the work actually in flight. Conversation-only
// agents stay cheap; a workbench grows when a job starts and shrinks when
// it finishes.
//
// Numbers are the cloud-pricing floors too: a light seat is the conversation
// reservation; a coding seat is the workbench base plus one standard job.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sqlx::PgPool;

use talaria_fleet_docker::{self as docker, managed_container};

const LOG: &str = "[budget]";
const HOST_MEM_TTL: Duration = Duration::from_secs(5);

static HOST_MEM_CACHE: Mutex<Option<(Instant, HostMem)>> = Mutex::new(None);

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// Conversation-only agent (Hermes gateway + tools, no sandbox).
pub const LIGHT_RESERVE: u64 = 768 * MIB;
pub const LIGHT_CEILING: u64 = 2 * GIB;

/// Workbench idle: Hermes + idle sandbox + persistent caches.
pub const WB_BASE_RESERVE: u64 = 2 * GIB;
/// Last-ditch hard ceiling. Packing, not this number, is the queue.
pub const WB_CEILING: u64 = 32 * GIB;

/// Per live coding job inside the agent's one container.
/// Heavy is 4 GiB because Playwright/Chrome + tsserver + a dev server
/// routinely passes 3 GiB; this is the packing estimate, not a cgroup cap.
pub const JOB_LIGHT: u64 = GIB;
pub const JOB_STANDARD: u64 = 2 * GIB;
pub const JOB_HEAVY: u64 = 4 * GIB;

/// Runaway guard — not the queue. A loop must not mint 100 clones on a
/// 256 GiB box. Sixteen standard jobs is 32 GiB of job RAM on top of base.
pub const MAX_LIVE_JOBS_PER_AGENT: usize = 16;

/// Fallback only when /proc/meminfo is missing. Live reserve is 25% of the
/// VM, clamped to 2–16 GiB, so a 8 GiB box keeps 2 GiB for kernel/docker/
/// Postgres and a 64 GiB box keeps 16 GiB.
pub const HOST_RESERVE: u64 = 4 * GIB;
pub const PLATFORM_RESERVE_MIN: u64 = 2 * GIB;
pub const PLATFORM_RESERVE_MAX: u64 = 16 * GIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMem {
    pub total: u64,
    pub available: u64,
}

pub fn effort_reserve(effort: &str) -> u64 {
    match effort {
        "light" => JOB_LIGHT,
        "heavy" => JOB_HEAVY,
        _ => JOB_STANDARD,
    }
}

pub fn jobs_bytes(efforts: &[String]) -> u64 {
    let jobs: u64 = efforts
        .iter()
        .map(|e| effort_reserve(e))
        .fold(0, u64::saturating_add);
    WB_BASE_RESERVE.saturating_add(jobs)
}

/// Hard cgroup ceiling for a known VM. `host` none → configured default only.
pub fn workbench_limit_for(host: Option<&HostMem>) -> u64 {
    let configured = parse_bytes_env("AGENT_WB_MEM_LIMIT")
        .unwrap_or(WB_CEILING)
        .max(WB_BASE_RESERVE);
    match host {
        Some(h) => host_safe_ceiling(h.total, host_reserve_for(h.total), configured),
        None => configured,
    }
}

pub async fn workbench_limit() -> u64 {
    workbench_limit_for(host_mem().await.as_ref())
}

pub fn host_safe_ceiling(host_total: u64, reserve: u64, configured: u64) -> u64 {
    let room = host_total.saturating_sub(reserve);
    configured.min(room).max(WB_BASE_RESERVE.min(room))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Ok,
    Tight,
    Critical,
}

pub fn host_pressure(available: u64, reserve: u64) -> Pressure {
    if available < reserve {
        Pressure::Critical
    } else if available < reserve.saturating_add(JOB_STANDARD) {
        Pressure::Tight
    } else {
        Pressure::Ok
    }
}

impl Pressure {
    pub fn as_str(self) -> &'static str {
        match self {
            Pressure::Ok => "ok",
            Pressure::Tight => "tight",
            Pressure::Critical => "critical",
        }
    }
}

pub fn platform_reserve_for(vm_total: u64) -> u64 {
    (vm_total / 4).clamp(PLATFORM_RESERVE_MIN, PLATFORM_RESERVE_MAX)
}

pub fn host_reserve_for(vm_total: u64) -> u64 {
    parse_bytes_env("TALARIA_HOST_RESERVE").unwrap_or_else(|| platform_reserve_for(vm_total))
}

pub async fn host_reserve() -> u64 {
    if let Some(v) = parse_bytes_env("TALARIA_HOST_RESERVE") {
        return v;
    }
    host_mem()
        .await
        .map(|h| platform_reserve_for(h.total))
        .unwrap_or(HOST_RESERVE)
}

pub fn parse_meminfo(text: &str) -> Option<HostMem> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        let kib: u64 = parts.next()?.parse().ok()?;
        let bytes = kib.saturating_mul(1024);
        match key {
            "MemTotal:" => total = Some(bytes),
            "MemAvailable:" => available = Some(bytes),
            _ => {}
        }
    }
    Some(HostMem {
        total: total?,
        available: available?,
    })
}

/// Prefer `/proc/meminfo` when it IS the docker host (API on the VM).
/// When the API is cgrouped smaller than `docker info` MemTotal, pack
/// against the docker host: total from docker, available = total − running
/// container RSS (stricter than MemAvailable — cache does not count as free).
pub fn merge_host_mem(proc: HostMem, docker_total: Option<u64>, containers_used: u64) -> HostMem {
    let Some(dt) = docker_total else {
        return proc;
    };
    let slack = dt.max(proc.total) / 10;
    if proc.total.abs_diff(dt) <= slack {
        HostMem {
            total: dt.max(proc.total),
            available: proc.available,
        }
    } else {
        HostMem {
            total: dt,
            available: dt.saturating_sub(containers_used),
        }
    }
}

pub async fn host_mem() -> Option<HostMem> {
    if let Ok(g) = HOST_MEM_CACHE.lock()
        && let Some((at, mem)) = *g
        && at.elapsed() < HOST_MEM_TTL
    {
        return Some(mem);
    }
    let proc = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .as_deref()
        .and_then(parse_meminfo)?;
    let docker_total = docker_mem_total().await;
    let used = if docker_total.is_some_and(|dt| proc.total.abs_diff(dt) > dt.max(proc.total) / 10) {
        docker_containers_used().await
    } else {
        0
    };
    let mem = merge_host_mem(proc, docker_total, used);
    if let Ok(mut g) = HOST_MEM_CACHE.lock() {
        *g = Some((Instant::now(), mem));
    }
    Some(mem)
}

async fn docker_mem_total() -> Option<u64> {
    let (out, _) = docker::docker(
        &["info", "--format", "{{.MemTotal}}"],
        Duration::from_secs(3),
    )
    .await
    .ok()?;
    out.trim().parse().ok()
}

async fn docker_containers_used() -> u64 {
    let Ok((out, _)) = docker::docker(
        &["stats", "--no-stream", "--format", "{{.MemUsage}}"],
        Duration::from_secs(8),
    )
    .await
    else {
        return 0;
    };
    out.lines()
        .filter_map(|line| line.split('/').next())
        .filter_map(parse_docker_iec)
        .fold(0, u64::saturating_add)
}

fn parse_docker_iec(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(head) = s.strip_suffix("iB") {
        let unit = *head.as_bytes().last()?;
        let num = head.get(..head.len() - 1)?;
        let mult = match unit {
            b'G' => GIB,
            b'M' => MIB,
            b'K' => 1024,
            _ => return None,
        };
        (num, mult)
    } else {
        let head = s.strip_suffix('B')?;
        (head, 1)
    };
    let n: f64 = num.trim().parse().ok()?;
    Some((n * mult as f64) as u64)
}

/// True when the host can take `extra` more reserved bytes without dipping
/// under the platform keep-back.
pub fn can_admit(available: u64, extra: u64, reserve: u64) -> bool {
    available >= extra.saturating_add(reserve)
}

pub fn fmt_bytes(n: u64) -> String {
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else {
        format!("{} MiB", n / MIB)
    }
}

pub fn compose_size(n: u64) -> String {
    if n.is_multiple_of(GIB) {
        format!("{}g", n / GIB)
    } else if n.is_multiple_of(MIB) {
        format!("{}m", n / MIB)
    } else {
        n.to_string()
    }
}

/// Refuse a new workbench session/job when the docker host cannot take
/// another job's RAM. Missing meminfo fails open.
pub async fn admit_work(extra: u64) -> Result<(), String> {
    let Some(host) = host_mem().await else {
        return Ok(());
    };
    let reserve = host_reserve_for(host.total);
    if can_admit(host.available, extra, reserve) {
        Ok(())
    } else {
        Err(format!(
            "host has {} free; starting this work needs {} plus {} kept for the platform — waiting for a job to finish",
            fmt_bytes(host.available),
            fmt_bytes(extra),
            fmt_bytes(reserve)
        ))
    }
}

/// Resize the agent's running container to the jobs it actually has.
/// Best-effort: a docker failure never fails the job that triggered it.
pub async fn sync_workbench_container(pg: &PgPool, department: &str, efforts: &[String]) {
    let host = host_mem().await;
    let limit = workbench_limit_for(host.as_ref());
    let reservation = jobs_bytes(efforts).min(limit);
    let container = managed_container(pg, department).await;
    match docker::update_container_memory(&container, limit, reservation).await {
        Ok(()) => tracing::info!(
            "{LOG} {department}: reservation {} / ceiling {} ({} live jobs)",
            fmt_bytes(reservation),
            fmt_bytes(limit),
            efforts.len()
        ),
        Err(e) => tracing::warn!("{LOG} {department}: docker update failed: {e}"),
    }
}

fn parse_bytes_env(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    parse_size(&raw)
}

pub fn parse_size(raw: &str) -> Option<u64> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (num, mul) = if let Some(n) = s.strip_suffix("gi") {
        (n, GIB)
    } else if let Some(n) = s.strip_suffix('g') {
        (n, GIB)
    } else if let Some(n) = s.strip_suffix("mi") {
        (n, MIB)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, MIB)
    } else {
        (s.as_str(), 1)
    };
    let n: f64 = num.trim().parse().ok()?;
    Some((n * mul as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_reads_available() {
        let text = "MemTotal:        16384000 kB\nMemFree:          1000000 kB\nMemAvailable:     8192000 kB\n";
        let h = parse_meminfo(text).expect("parse");
        assert_eq!(h.total, 16384000 * 1024);
        assert_eq!(h.available, 8192000 * 1024);
    }

    #[test]
    fn packing_refuses_when_host_is_tight() {
        assert!(can_admit(8 * GIB, JOB_STANDARD, HOST_RESERVE));
        assert!(!can_admit(5 * GIB, JOB_STANDARD, HOST_RESERVE));
        assert!(!can_admit(4 * GIB, JOB_STANDARD, HOST_RESERVE));
    }
    #[test]
    fn reservation_grows_with_jobs() {
        let three_std = vec!["standard".into(), "standard".into(), "standard".into()];
        assert_eq!(jobs_bytes(&three_std), 2 * GIB + 6 * GIB);
        let one_heavy = vec!["heavy".into()];
        assert_eq!(jobs_bytes(&one_heavy), 2 * GIB + 4 * GIB);
        assert!(jobs_bytes(&[]) < jobs_bytes(&one_heavy));
    }

    #[test]
    fn api_cgroup_does_not_masquerade_as_the_vm() {
        let proc = HostMem {
            total: 2 * GIB,
            available: GIB,
        };
        // API on the host: docker MemTotal ≈ /proc.
        let on_host = merge_host_mem(proc, Some(16 * GIB + 100), 0);
        // 2g vs 16g is NOT close — this is the containerized API case.
        assert_eq!(on_host.total, 16 * GIB + 100);
        let used = 6 * GIB;
        let boxed = merge_host_mem(proc, Some(16 * GIB), used);
        assert_eq!(boxed.total, 16 * GIB);
        assert_eq!(boxed.available, 10 * GIB);
        let same = merge_host_mem(
            HostMem {
                total: 16 * GIB,
                available: 9 * GIB,
            },
            Some(16 * GIB),
            99 * GIB,
        );
        assert_eq!(same.available, 9 * GIB);
    }

    #[test]
    fn one_agent_cannot_eat_the_host() {
        assert_eq!(platform_reserve_for(8 * GIB), 2 * GIB);
        assert_eq!(platform_reserve_for(16 * GIB), 4 * GIB);
        assert_eq!(platform_reserve_for(64 * GIB), 16 * GIB);
        assert_eq!(platform_reserve_for(256 * GIB), 16 * GIB);
        assert_eq!(
            host_safe_ceiling(8 * GIB, platform_reserve_for(8 * GIB), 32 * GIB),
            6 * GIB
        );
        assert_eq!(host_safe_ceiling(16 * GIB, 4 * GIB, 32 * GIB), 12 * GIB);
        assert_eq!(host_safe_ceiling(64 * GIB, 4 * GIB, 32 * GIB), 32 * GIB);
        assert_eq!(host_pressure(8 * GIB, 4 * GIB), Pressure::Ok);
        assert_eq!(host_pressure(5 * GIB, 4 * GIB), Pressure::Tight);
        assert_eq!(host_pressure(3 * GIB, 4 * GIB), Pressure::Critical);
    }

    #[test]
    fn effort_floors() {
        assert_eq!(effort_reserve("light"), JOB_LIGHT);
        assert_eq!(effort_reserve("standard"), JOB_STANDARD);
        assert_eq!(effort_reserve("heavy"), JOB_HEAVY);
        assert_eq!(effort_reserve(""), JOB_STANDARD);
    }

    #[test]
    fn parse_size_spellings() {
        assert_eq!(parse_size("4g"), Some(4 * GIB));
        assert_eq!(parse_size("768m"), Some(768 * MIB));
        assert_eq!(parse_size("1Gi"), Some(GIB));
    }
}
