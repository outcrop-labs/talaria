// Per-agent container resource sampling — the observability sampler.
//
// Nothing sampled container CPU/memory anywhere before this: the fleet's
// health surface was state-only (running/warming/unhealthy/down), and the
// 2026-09-17 outcrop incident made the gap concrete — an agent container sat
// at its 4g ceiling OOM-killing builds for hours while every dashboard read
// green. This job samples each managed agent container once a minute through
// the same docker CLI channel every other fleet verb uses, keeps a week of
// samples, and prunes expired run-transcript artifacts per the admin's
// retention setting (`observability.transcriptRetentionDays` — days, null or
// 0 = keep forever; some deployments want the permanent record).

use std::sync::Arc;

use sqlx::PgPool;

use talaria_scheduler::{JobName, JobSpec};

const LOG: &str = "[resources]";

/// Samples older than this are pruned every pass — resource history is a
/// week of one-minute points (~10k rows per agent), not a ledger.
const SAMPLE_RETENTION_DAYS: i64 = 7;

/// One `docker stats --no-stream` pass over the managed agent containers,
/// then the prune. Fire-and-forget by nature (a scheduler job calls it);
/// every failure is logged here and never thrown.
pub async fn sample_agent_resources(pg: &PgPool) {
    // Which agents run HERE, and under which container name — resolved
    // through managed_container (slot-aware, never the hardcoded spelling).
    let agents: Vec<(String, String)> = match sqlx::query_as::<_, (String, String)>(
        "select model, department from agent_defs where managed and enabled",
    )
    .fetch_all(pg)
    .await
    {
        Ok(rows) => {
            let mut v = Vec::new();
            for (model, department) in rows {
                v.push((
                    model,
                    talaria_fleet_docker::managed_container(pg, &department).await,
                ));
            }
            v
        }
        Err(e) => {
            tracing::error!("{LOG} roster read failed: {e}");
            return;
        }
    };
    if agents.is_empty() {
        return;
    }
    let mut args: Vec<&str> = vec!["stats", "--no-stream", "--format", "{{json .}}"];
    for (_, container) in &agents {
        args.push(container.as_str());
    }
    let (out, _) =
        match talaria_fleet_docker::docker(&args, std::time::Duration::from_secs(20)).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("{LOG} docker stats errored: {e}");
                return;
            }
        };
    let by_container: std::collections::HashMap<String, String> = agents
        .into_iter()
        .map(|(model, container)| (container, model))
        .collect();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // docker stats JSON: {"CPUPercentage":"1.23%","MemUsage":"2.4GiB / 8GiB",
        // "PIDs":"326", "Name": "..."} — strings by design of the CLI format.
        let Some(name) = v.get("Name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(model) = by_container.get(name) else {
            continue;
        };
        let cpu = v
            .get("CPUPercentage")
            .and_then(|c| c.as_str())
            .and_then(|c| c.trim_end_matches('%').parse::<f64>().ok())
            .unwrap_or(0.0);
        // MemUsage's used half, human-spelled ("2.4GiB", "946MiB", "512B").
        let mem = v
            .get("MemUsage")
            .and_then(|m| m.as_str())
            .and_then(|m| m.split('/').next())
            .and_then(parse_docker_size)
            .unwrap_or(0);
        let pids = v
            .get("PIDs")
            .and_then(|p| p.as_str())
            .and_then(|p| p.parse::<i64>().ok())
            .unwrap_or(0);
        let _ = sqlx::query(
            "insert into agent_resource_samples (agent_model, container, cpu_percent, mem_bytes, pids) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(model)
        .bind(name)
        .bind(cpu)
        .bind(mem)
        .bind(pids)
        .execute(pg)
        .await;
        warn_if_leaking(pg, model, mem).await;
    }
    if let Some(host) = talaria_fleet_budget::host_mem().await {
        let reserve = talaria_fleet_budget::host_reserve().await;
        match talaria_fleet_budget::host_pressure(host.available, reserve) {
            talaria_fleet_budget::Pressure::Critical => tracing::error!(
                "{LOG} host RAM critical: {} free, {} reserved for the platform — refusing new work",
                talaria_fleet_budget::fmt_bytes(host.available),
                talaria_fleet_budget::fmt_bytes(reserve)
            ),
            talaria_fleet_budget::Pressure::Tight => tracing::warn!(
                "{LOG} host RAM tight: {} free",
                talaria_fleet_budget::fmt_bytes(host.available)
            ),
            talaria_fleet_budget::Pressure::Ok => {}
        }
    }
    prune(pg).await;
}

/// Climbing RSS over ~8 minutes, never giving memory back, past 2 GiB.
/// A leak dies at the agent's cgroup ceiling — not by killing the host.
pub fn mem_is_leaking(oldest_first: &[i64]) -> bool {
    if oldest_first.len() < 8 {
        return false;
    }
    let first = oldest_first[0];
    let last = *oldest_first.last().unwrap();
    if last < 2 * (1 << 30) {
        return false;
    }
    let mut drops = 0;
    for w in oldest_first.windows(2) {
        if w[1] + (64 << 20) < w[0] {
            drops += 1;
        }
    }
    if drops > 1 {
        return false;
    }
    let slope = (last - first) / (oldest_first.len() as i64 - 1);
    slope > 256 * (1 << 20)
}

async fn warn_if_leaking(pg: &PgPool, model: &str, _latest: i64) {
    let rows: Vec<(i64,)> = match sqlx::query_as(
        "select mem_bytes from agent_resource_samples \
         where agent_model = $1 order by taken_at desc limit 15",
    )
    .bind(model)
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut oldest_first: Vec<i64> = rows.into_iter().map(|r| r.0).collect();
    oldest_first.reverse();
    if mem_is_leaking(&oldest_first) {
        tracing::warn!(
            "{LOG} {model}: memory climbing (likely leak) — cgroup will OOM this agent, not the host"
        );
    }
}

/// docker stats' human size spellings ("2.4GiB", "946MiB", "512B") to bytes.
fn parse_docker_size(s: &str) -> Option<i64> {
    let s = s.trim();
    let (num, mult) = if let Some(head) = s.strip_suffix("iB") {
        let unit = head.as_bytes().last()?;
        let num = head.get(..head.len() - 1)?;
        let mult = match unit {
            b'G' => 1 << 30,
            b'M' => 1 << 20,
            b'K' => 1 << 10,
            _ => return None,
        };
        (num, mult)
    } else if let Some(head) = s.strip_suffix('B') {
        (head, 1)
    } else {
        (s, 1)
    };
    let n: f64 = num.trim().parse().ok()?;
    Some((n * mult as f64) as i64)
}

async fn transcript_retention_days(pg: &PgPool) -> Option<i64> {
    let days: Option<i64> = sqlx::query_scalar(
        "select value ->> 'transcriptRetentionDays' from app_settings where key = 'observability'",
    )
    .fetch_optional(pg)
    .await
    .ok()
    .flatten();
    match days {
        None => Some(7),
        Some(0) => None,
        Some(n) => Some(n),
    }
}

async fn prune(pg: &PgPool) {
    let _ = sqlx::query(
        "delete from agent_resource_samples where taken_at < now() - ($1::int * interval '1 day')",
    )
    .bind(SAMPLE_RETENTION_DAYS as i32)
    .execute(pg)
    .await;
    if let Some(days) = transcript_retention_days(pg).await {
        let _ = sqlx::query(
            "delete from artifacts where kind = 'run-transcript' and updated_at < now() - ($1::int * interval '1 day')",
        )
        .bind(days as i32)
        .execute(pg)
        .await;
    }
}

pub fn resource_job_spec(pg: PgPool) -> JobSpec {
    JobSpec {
        name: JobName::AgentResourceSample,
        every_ms: 60_000,
        first_run_delay_ms: Some(120_000),
        max_run_ms: Some(30_000),
        // PER-INSTANCE, unlike work-redispatch: the samples come from THIS
        // host's docker — an instance that does not run beside the fleet it
        // samples finds no containers and no-ops.
        per_instance: true,
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move {
                sample_agent_resources(&pg).await;
                Ok(None)
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{mem_is_leaking, parse_docker_size};

    #[test]
    fn docker_sizes_parse_to_bytes() {
        assert_eq!(
            parse_docker_size("2.4GiB"),
            Some((2.4 * (1 << 30) as f64) as i64)
        );
        assert_eq!(parse_docker_size("946MiB"), Some(946 << 20));
        assert_eq!(parse_docker_size("512B"), Some(512));
        assert_eq!(parse_docker_size("16KiB"), Some(16 << 10));
        assert_eq!(parse_docker_size("nonsense"), None);
    }

    #[test]
    fn leak_is_steady_climb_past_two_gig() {
        let gig = 1 << 30;
        let climb: Vec<i64> = (0..10).map(|i| (2 * gig) + i * (300 << 20)).collect();
        assert!(mem_is_leaking(&climb));
        let flat = vec![3 * gig; 10];
        assert!(!mem_is_leaking(&flat));
        let tiny: Vec<i64> = (0..10).map(|i| (50 << 20) + i * (20 << 20)).collect();
        assert!(!mem_is_leaking(&tiny));
    }
}
