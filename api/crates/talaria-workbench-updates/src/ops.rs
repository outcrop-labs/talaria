// THE LIVE INSTANCE behind [`Ops`]: the state row in Postgres, upstream over
// HTTPS, the running Developer Agents' containers, the provisioning's own
// install-and-preflight (talaria-workbench-mcp's `verify_harness`), and the
// admins' notifications.

use futures_util::future::BoxFuture;
use sqlx::PgPool;

use talaria_notify::{
    ActorKind, Coalesce, NotificationInput, NotifyActor, NotifyDeps, NotifyEntity, NotifyMeta,
    add_notification_with, entity,
};
use talaria_workbench_harnesses::Versions;
use talaria_workbench_mcp::devenv::VerifyFailure;

use crate::Tool;
use crate::engine::{Ops, Patch, Smoke, Workbench};
use crate::state::State;
use crate::upstream::{self, Release};

pub struct Live {
    pub pg: PgPool,
    pub notify: NotifyDeps,
}

/// Every Developer Agent's department runner, oldest first: one workbench
/// volume per department (the same ranking the agent image channel uses).
pub async fn developer_departments(pg: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "select department, display_name from ( \
           select d.*, row_number() over (partition by d.department order by \
             exists (select 1 from agent_versions v \
               where v.agent_id = d.id and v.version = d.current_version) desc, \
             d.created_at, d.id) as rank \
           from agent_defs d where d.managed and d.enabled \
         ) ranked where rank = 1 and developer order by created_at, slug",
    )
    .fetch_all(pg)
    .await
}

impl Ops for Live {
    fn now(&self) -> i64 {
        talaria_agent_auth::now_ms()
    }

    fn jitter(&self, lo: i64, hi: i64) -> i64 {
        let span = (hi - lo).max(1) as u128;
        lo + (uuid::Uuid::new_v4().as_u128() % span) as i64
    }

    fn load(&self) -> BoxFuture<'_, Result<State, String>> {
        Box::pin(crate::load(&self.pg))
    }

    fn patch<'a>(&'a self, f: Patch<'a>) -> BoxFuture<'a, Result<State, String>> {
        Box::pin(crate::patch(&self.pg, f))
    }

    fn fetch<'a>(
        &'a self,
        tool: Tool,
        version: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<Release>, String>> {
        Box::pin(upstream::fetch(tool, version))
    }

    fn workbenches(&self) -> BoxFuture<'_, Vec<Workbench>> {
        Box::pin(async move {
            let rows = match developer_departments(&self.pg).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        "[workbench-harness] the Developer Agents could not be read: {e}"
                    );
                    return Vec::new();
                }
            };
            let mut out = Vec::new();
            for (department, agent) in rows {
                let container =
                    talaria_fleet_docker::managed_container(&self.pg, &department).await;
                if talaria_fleet_docker::container_running(&container).await {
                    out.push(Workbench { department, agent });
                }
            }
            out
        })
    }

    fn verify<'a>(
        &'a self,
        w: &'a Workbench,
        v: &'a Versions,
        smoke: &'a Smoke,
    ) -> BoxFuture<'a, Result<Option<String>, VerifyFailure>> {
        Box::pin(async move {
            talaria_workbench_mcp::devenv::verify_harness(
                &self.pg,
                &w.department,
                v,
                &smoke.keep,
                smoke.integrity.as_deref(),
            )
            .await
            .map(|(_, lock)| lock)
        })
    }

    fn store_lock<'a>(
        &'a self,
        v: &'a Versions,
        lock: &'a str,
        integrity: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let value = serde_json::json!({
                "lock": lock,
                "integrity": integrity,
                "at": talaria_agent_auth::now_ms(),
            });
            sqlx::query(
                "insert into app_settings (key, value) values ($1, $2) \
                 on conflict (key) do update set value = excluded.value, updated_at = now()",
            )
            .bind(talaria_workbench_harnesses::lock_key(v))
            .bind(value)
            .execute(&self.pg)
            .await
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "the verified lockfile for Oh My Pi {} on bun {} could not be kept: {e}",
                    v.omp, v.bun
                )
            })
        })
    }

    fn prune_locks<'a>(&'a self, keep: &'a [Versions]) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let keys: Vec<String> = keep
                .iter()
                .map(talaria_workbench_harnesses::lock_key)
                .collect();
            if let Err(e) =
                sqlx::query("delete from app_settings where key like $1 and not (key = any($2))")
                    .bind(format!("{}%", talaria_workbench_harnesses::LOCK_KEY_PREFIX))
                    .bind(&keys)
                    .execute(&self.pg)
                    .await
            {
                tracing::warn!("[workbench-harness] old lockfiles could not be pruned: {e}");
            }
        })
    }

    fn notify<'a>(&'a self, topic: &'a str, title: &'a str, body: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let admins: Vec<(String,)> =
                match sqlx::query_as("select id::text from users where role = 'admin'")
                    .fetch_all(&self.pg)
                    .await
                {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(
                            "[workbench-harness] the admins could not be read to notify: {e}"
                        );
                        return;
                    }
                };
            let meta = NotifyMeta {
                group_key: Some(talaria_notify::group_key::workbench_harness(topic)),
                coalesce: Coalesce::Realert,
                entity: Some(NotifyEntity::new(entity::WORKBENCH_HARNESS, topic)),
                actor: Some(NotifyActor {
                    kind: ActorKind::System,
                    id: "workbench-harness-channel".into(),
                    label: "Workbench harness updates".into(),
                }),
                ..Default::default()
            };
            for (admin,) in admins {
                if let Err(e) = add_notification_with(
                    &self.notify,
                    &admin,
                    &NotificationInput {
                        kind: crate::KIND,
                        title,
                        body: Some(body),
                        href: Some(crate::HREF),
                    },
                    &meta,
                )
                .await
                {
                    tracing::warn!("[workbench-harness] the notification to {admin} failed: {e}");
                }
            }
        })
    }
}
