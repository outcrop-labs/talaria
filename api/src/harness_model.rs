// THE model-resolution chain. Port of harness/model.ts. Every harness
// resolves its model through here — this port's first consumer is the muse
// chain (/api/models' `effective`), and the runs plane (batch 4) resolves
// every harness through the same function.
//
// The chain exists because the same eight lines — the platform-agent pin,
// then the Utility model role, then TALARIA_COPILOT_MODEL, then 'pl-main',
// then the first routable bare model — were hand-copied VERBATIM into seven
// files (AUDIT-HARNESS-2026-08-06, finding 1.10). Changing the policy meant
// finding all seven, and nobody ever did.
//
// RESOLUTION CONTRACT, inherited from model_roles.rs and platform_agents.rs
// and non-negotiable: a model only wins while it still ROUTES on the gateway.
// A deleted model can never silently break a subsystem — the harness falls to
// the next step instead.

use crate::model_access::{
    GatewayModel, gateway_models, member_model_allowlist, model_allowed_for,
};
use crate::model_roles::resolve_role_model;
use crate::platform_agents::platform_agent_model;
use crate::users::{get_preferred_model, get_user_role};
use sqlx::PgPool;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::OnceCell;

/// THE RESOLUTION EDGES, as an argument rather than a global. The TS twin of
/// this chain takes its whole dependency set that way (`resolveHarnessModelWith`),
/// and the fitness binding pass is the reason it has to: `bind_slots` needs to
/// know which ROLES a harness's chain consults, which is a question about the
/// CHAIN, not about today's assignments — running the real chain against the
/// database would answer a different question every time an admin moved a pin.
/// So the chain runs over edges that record and refuse, and the set of roles it
/// asked for is the answer. Nothing else in this port may restate the step
/// order; that is finding 1.10's whole point.
pub trait ResolveEdges: Send + Sync {
    fn pin_model<'a>(
        &'a self,
        pin: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>>;
    fn role_model<'a>(
        &'a self,
        role: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>>;
    fn routes<'a>(
        &'a self,
        model: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, sqlx::Error>> + Send + 'a>>;
    fn gateway_models<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<GatewayModel>, sqlx::Error>> + Send + 'a>>;
    fn preferred_model<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>>;
    fn user_role<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, sqlx::Error>> + Send + 'a>>;
    fn member_allowlist(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>>;
    fn env_model(&self) -> Option<String>;
}

/// The production edges: every question answered by the database and the
/// environment, exactly as the pg-bound chain always answered them.
struct PgEdges<'a> {
    pg: &'a PgPool,
}

impl ResolveEdges for PgEdges<'_> {
    fn pin_model<'a>(
        &'a self,
        pin: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        Box::pin(platform_agent_model(self.pg, pin))
    }
    fn role_model<'a>(
        &'a self,
        role: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        Box::pin(resolve_role_model(self.pg, role))
    }
    fn routes<'a>(
        &'a self,
        model: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, sqlx::Error>> + Send + 'a>> {
        Box::pin(async move {
            Ok(crate::gateway::registry::resolve_route(self.pg, model)
                .await?
                .is_some())
        })
    }
    fn gateway_models<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<GatewayModel>, sqlx::Error>> + Send + 'a>> {
        Box::pin(gateway_models(self.pg))
    }
    fn preferred_model<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        Box::pin(get_preferred_model(self.pg, user_id))
    }
    fn user_role<'a>(
        &'a self,
        user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, sqlx::Error>> + Send + 'a>> {
        Box::pin(get_user_role(self.pg, user_id))
    }
    fn member_allowlist(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>> {
        Box::pin(member_model_allowlist(self.pg))
    }
    fn env_model(&self) -> Option<String> {
        copilot_env_model()
    }
}

pub type ModelChainStep = &'static str;

pub struct ModelSpec<'a> {
    /// Admin assignment slot on Models → Platform (platform_agents.rs).
    pub pin: Option<&'a str>,
    /// The model role this harness belongs to (model_roles.rs).
    pub role: Option<&'a str>,
    /// Order to try. Default: pin, role, utility, env, first-routable.
    ///
    /// AN EMPTY CHAIN IS A DECLARATION, NOT AN OVERSIGHT — for every harness
    /// whose model comes from the SUBJECT of the call (the owner's own
    /// assistant, the agent on the ticket). Those callers hand the model in
    /// directly and the chain's only job is to answer "nothing, loudly" if a
    /// caller ever forgets. A silent identity substitution is the one failure
    /// mode worth failing loudly to avoid.
    pub chain: Option<&'a [ModelChainStep]>,
    /// For user-scoped harnesses (muse, distiller): the owner, enabling the
    /// 'preferred' step and the member model allowlist.
    pub user_id: Option<&'a str>,
}

pub struct ResolvedHarnessModel {
    pub model: String,
    /// The winning STEP is part of the answer, not a detail: the runner
    /// records it on the harness_runs row, and the model-fitness UI reads
    /// those rows to show an operator which fallback actually carried a
    /// harness in production. A subsystem limping along on 'first-routable'
    /// for a month is a real finding.
    pub step: ModelChainStep,
}

const DEFAULT_CHAIN: [ModelChainStep; 5] = ["pin", "role", "utility", "env", "first-routable"];

// The reference deployment names its main endpoint model 'pl-main'. Preferring
// it in the last-resort step reproduces today's behavior on that install
// (pl-main beat the alphabetical scan) without making any install DEPEND on
// the name: where it doesn't exist, the step still returns a real model
// instead of nothing. This list is the ONLY place in this port allowed to
// spell it.
const LAST_RESORT_PREFERENCE: [&str; 1] = ["pl-main"];

/// TALARIA_COPILOT_MODEL, read late not at boot — same as the TS dep, which
/// exists so a test never has to touch the environment.
fn copilot_env_model() -> Option<String> {
    std::env::var("TALARIA_COPILOT_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
}

/// One resolution's memoization state. A chain that falls through five steps
/// must not fetch the catalog five times — the TS `once()` closures, spelled
/// as cells instead of closures.
struct Chain<'a, E: ResolveEdges + ?Sized> {
    edges: &'a E,
    spec: &'a ModelSpec<'a>,
    catalog: OnceCell<Vec<GatewayModel>>,
    /// None until first needed; computed once, then shared by every gated
    /// step (and by first-routable's allowed()).
    gate: OnceCell<Gate>,
}

/// The member model allowlist is ORG POLICY: an admin gating the expensive
/// brains decides which models a non-admin may be handed, and no refactor
/// gets to route around it. It applies to the steps that hand a USER's own
/// choice or the user-visible catalog to a harness — 'preferred', 'role',
/// 'utility', 'first-routable'. It deliberately does NOT apply to 'pin' or
/// 'env': an admin-assigned platform agent model and a server env default
/// are org policy themselves, set by the people the allowlist exists to
/// serve. Org-scoped harnesses (no userId) have no owner to gate, so the
/// gate is open.
enum Gate {
    Open,
    Gated {
        role: String,
        allow: Vec<String>,
        catalog: Vec<GatewayModel>,
    },
}

impl<'a, E: ResolveEdges + ?Sized> Chain<'a, E> {
    async fn catalog(&self) -> Result<&[GatewayModel], sqlx::Error> {
        if let Some(c) = self.catalog.get() {
            return Ok(c);
        }
        let loaded = self.edges.gateway_models().await?;
        // A lost set race (impossible in one sequential resolution) keeps the
        // winner's identical copy — `get` is then always Some.
        let _ = self.catalog.set(loaded);
        Ok(self.catalog.get().map_or(&[][..], |v| v.as_slice()))
    }

    async fn gate(&self) -> Result<&Gate, sqlx::Error> {
        if let Some(g) = self.gate.get() {
            return Ok(g);
        }
        let computed = match self.spec.user_id {
            None => Gate::Open,
            Some(user_id) => {
                // TS reads role, allowlist and catalog concurrently; the
                // awaits are sequential here and the cell dedups repeats.
                let role = self.edges.user_role(user_id).await?;
                let allow = self.edges.member_allowlist().await;
                let catalog = self.catalog().await?.to_vec();
                Gate::Gated {
                    role,
                    allow,
                    catalog,
                }
            }
        };
        let _ = self.gate.set(computed);
        Ok(self.gate.get().expect("gate just set"))
    }

    async fn gated(&self, model: Option<String>) -> Result<Option<String>, sqlx::Error> {
        let Some(model) = model else {
            return Ok(None);
        };
        Ok(match self.gate().await? {
            Gate::Open => Some(model),
            Gate::Gated {
                role,
                allow,
                catalog,
            } => {
                if model_allowed_for(role, &model, allow, catalog) {
                    Some(model)
                } else {
                    None
                }
            }
        })
    }

    /// Does this model id land on an endpoint right now? RESOLVES — and so
    /// advances the round-robin cursor — which is the TS `routes` dep's exact
    /// behavior and intentional parity: this is the same call live traffic
    /// makes. (Conversely 'pin' and 'role'/'utility' are validated INSIDE
    /// platform_agent_model/resolve_role_model, their documented contract, and
    /// are not re-checked here for the same cursor reason.)
    async fn routes(&self, model: &str) -> Result<bool, sqlx::Error> {
        self.edges.routes(model).await
    }

    async fn attempt(&self, step: &str) -> Result<Option<String>, sqlx::Error> {
        match step {
            "pin" => match self.spec.pin {
                Some(pin) => self.edges.pin_model(pin).await,
                None => Ok(None),
            },
            "role" => match self.spec.role {
                Some(role) => {
                    let m = self.edges.role_model(role).await?;
                    self.gated(m).await
                }
                None => Ok(None),
            },
            "utility" => {
                let m = self.edges.role_model("utility").await?;
                self.gated(m).await
            }
            "env" => {
                // A raw string from config: nothing has vetted it, so it goes
                // through routes() here.
                match self.edges.env_model() {
                    Some(m) if self.routes(&m).await? => Ok(Some(m)),
                    _ => Ok(None),
                }
            }
            "preferred" => {
                // A raw string from a user row — gated (it is the user's own
                // choice) and then routed.
                match self.spec.user_id {
                    Some(user_id) => {
                        let pref = self
                            .gated(self.edges.preferred_model(user_id).await?)
                            .await?;
                        match pref {
                            Some(p) if self.routes(&p).await? => Ok(Some(p)),
                            _ => Ok(None),
                        }
                    }
                    None => Ok(None),
                }
            }
            "first-routable" => {
                // Bare ids only: an endpoint-qualified pin ("ep/model") would
                // strand the harness on one backend, which is the opposite of
                // a last resort. gateway_models sorts by id, so "first" is
                // stable across runs.
                let gate = self.gate().await?;
                let catalog = self.catalog().await?;
                let allowed = |id: &str| match gate {
                    Gate::Open => true,
                    Gate::Gated {
                        role,
                        allow,
                        catalog,
                    } => model_allowed_for(role, id, allow, catalog),
                };
                let bare: Vec<&GatewayModel> = catalog
                    .iter()
                    .filter(|m| !m.qualified && allowed(&m.id))
                    .collect();
                Ok(bare
                    .iter()
                    .find(|m| LAST_RESORT_PREFERENCE.contains(&m.id.as_str()))
                    .map(|m| m.id.clone())
                    .or_else(|| bare.first().map(|m| m.id.clone())))
            }
            _ => Ok(None),
        }
    }
}

/// Null when the gateway serves nothing this spec can reach. Every step is
/// validated with routing before it wins — the existing contract that a
/// deleted model can never silently break a subsystem. A database failure
/// propagates (TS throws to the same 500).
pub async fn resolve_harness_model<'a>(
    pg: &'a PgPool,
    spec: &'a ModelSpec<'a>,
) -> Result<Option<ResolvedHarnessModel>, sqlx::Error> {
    resolve_harness_model_with(spec, &PgEdges { pg }).await
}

/// The chain over INJECTED edges — the seam the fitness binding pass runs the
/// real step order through without touching a database.
pub async fn resolve_harness_model_with<'a>(
    spec: &'a ModelSpec<'a>,
    edges: &dyn ResolveEdges,
) -> Result<Option<ResolvedHarnessModel>, sqlx::Error> {
    let chain = Chain {
        edges,
        spec,
        catalog: OnceCell::new(),
        gate: OnceCell::new(),
    };
    for step in spec.chain.unwrap_or(&DEFAULT_CHAIN) {
        if let Some(model) = chain.attempt(step).await? {
            return Ok(Some(ResolvedHarnessModel { model, step }));
        }
    }
    Ok(None)
}

/// The Muse's model policy, declared once instead of written out (audit 1.10
/// — this was one of the seven hand-copied spellings of the fallback chain).
/// The ORDER is today's, preserved exactly, and it is not the default chain:
///
///    pin        an admin-assigned Muse model (Models → Platform) is ORG
///               POLICY and wins over a personal preference.
///    preferred  then the user's own choice — the whole reason the Muse is
///               user-scoped and the reason 'preferred' comes before the role.
///    utility    then the org's Utility role model, spelled as the 'utility'
///               step (same resolution as role:'utility', one label — the
///               chain_step recorded on the run).
///    env        then TALARIA_COPILOT_MODEL.
///    first-…    then whatever the gateway serves (which prefers 'pl-main'
///               where that name exists, so the reference deployment resolves
///               as it always did, and a self-host that never used the name
///               still gets a real model instead of nothing).
pub const MUSE_CHAIN: [ModelChainStep; 5] =
    ["pin", "preferred", "utility", "env", "first-routable"];

/// The model a user's muse resolves to right now. Two other subsystems ask
/// the question without running a harness: /api/models shows it in the
/// picker, and comms-decay falls back to it for the distiller and the
/// concluder.
pub async fn muse_model_for(pg: &PgPool, user_id: &str) -> Result<Option<String>, sqlx::Error> {
    let spec = ModelSpec {
        pin: Some("muse"),
        role: None,
        chain: Some(&MUSE_CHAIN),
        user_id: Some(user_id),
    };
    Ok(resolve_harness_model(pg, &spec).await?.map(|r| r.model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chains_are_the_policy() {
        assert_eq!(
            DEFAULT_CHAIN,
            ["pin", "role", "utility", "env", "first-routable"]
        );
        // Muse's order is NOT the default: the user's preference outranks the
        // Utility role.
        assert_eq!(
            MUSE_CHAIN,
            ["pin", "preferred", "utility", "env", "first-routable"]
        );
        assert_eq!(LAST_RESORT_PREFERENCE, ["pl-main"]);
    }
}
