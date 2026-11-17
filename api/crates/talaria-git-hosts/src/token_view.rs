// What an agent is told its git credential carries on each repo it is
// granted: list_repos' `github` block and doctor's line per repo
// (talaria-workbench-mcp). The agent spends that credential itself, git push
// and the host's API alike (curl or `gh`, with the token `git credential
// fill` hands it for one command), so what it may do there is what this
// says.
//
// THE DOOR'S OWN ORDER, READ WITHOUT OPENING ANYTHING. The credential door
// (talaria-routes-integrations' git-credential) answers a granted repo from
// a workspace secret granted to the agent and spendable at the host first,
// then from the host's own login. This reads the same order: the secret by
// name only (talaria-workspace-secrets' `secret_answering_git_at`, never
// opened and never spent, so a one-shot is not burned by a diagnosis), and
// for a GitHub App the agent's set on the host (`permissions`: what an admin
// set, or the default) met with what the installation that mints the repo's
// token held at the last refresh (`holdings`). The record only, never
// GitHub: an installation the record never read is unknown, and said so,
// never "nothing". A host whose token cannot be narrowed (a PAT, Forgejo)
// says why, and nothing is claimed about what its token carries.

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_git_host_types::{HostKind, Level, Narrowing, PermissionSet};
use talaria_github::api_origin_for;

use crate::grants::RepoRef;
use crate::holdings::{InstallationRow, all_installation_rows, tracked_installations};
use crate::permissions::{self, default_set, installation_for_repo};
use crate::registry::{HostRow, host_rows};

/// Where the credential git gets for one granted repo comes from, and what
/// is known about what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A workspace secret granted to the agent answers git at this host
    /// before the host's own login does: it carries whatever its owner gave
    /// it.
    WorkspaceSecret { name: String },
    /// A GitHub App's token for the one repository: the agent's set met with
    /// what the installation holds.
    AppToken {
        /// The agent's set on the host, or the default.
        set: PermissionSet,
        is_default: bool,
        /// The installation that mints the repo's token, as far as the
        /// record can tell without GitHub.
        installation: Option<String>,
        /// What that installation held at the last refresh. None: not on
        /// record.
        held: Option<PermissionSet>,
    },
    /// The host's own token, which Talaria cannot narrow per agent: the
    /// sentence that says why.
    HostWide { why: &'static str },
}

/// One granted repo's credential, as the agent is told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantCredential {
    pub host_id: String,
    pub host_name: String,
    pub kind: HostKind,
    pub repo: String,
    /// The REST origin the host's API answers on, for a GitHub host.
    pub api: Option<String>,
    pub credential: Credential,
}

impl GrantCredential {
    /// What the token carries, when that is known: the set met with what
    /// the installation holds, plus `metadata: read`, which GitHub puts on
    /// every token. Nothing at all when the installation holds none of the
    /// set: an ask that comes to nothing is never minted. None for a secret,
    /// a host-wide token, or an installation the record has not read.
    pub fn carries(&self) -> Option<PermissionSet> {
        let Credential::AppToken {
            set,
            held: Some(held),
            ..
        } = &self.credential
        else {
            return None;
        };
        let mut carries = set.meet(held);
        if !carries.is_empty() && held.contains("metadata") {
            carries.raise("metadata", Level::Read);
        }
        Some(carries)
    }

    /// What the set names that the installation does not hold (the token
    /// goes without it), when that is known.
    pub fn shortfall(&self) -> Option<PermissionSet> {
        match &self.credential {
            Credential::AppToken {
                set,
                held: Some(held),
                ..
            } => Some(set.missing_from(held)),
            _ => None,
        }
    }

    /// The grant's `github` member in list_repos: null on a host that is not
    /// GitHub. `narrowing` is `perRepo` (a token for this repo alone, the
    /// agent's set), `hostWide` (the host's own token, `why`), or
    /// `workspaceSecret` (that secret's, by `secret`).
    pub fn github_wire(&self) -> Value {
        if self.kind != HostKind::Github {
            return Value::Null;
        }
        match &self.credential {
            Credential::WorkspaceSecret { name } => json!({
                "narrowing": "workspaceSecret",
                "secret": name,
                "carries": null,
                "shortfall": null,
                "api": self.api,
            }),
            Credential::HostWide { why } => json!({
                "narrowing": Narrowing::HostWide(*why).wire(),
                "why": why,
                "carries": null,
                "shortfall": null,
                "api": self.api,
            }),
            Credential::AppToken {
                set,
                is_default,
                installation,
                ..
            } => json!({
                "narrowing": Narrowing::PerRepo.wire(),
                "set": set.to_json(),
                "isDefault": is_default,
                "carries": self.carries().map(|c| c.to_json()),
                "shortfall": self.shortfall().map(|s| s.to_json()),
                "installation": installation,
                "api": self.api,
            }),
        }
    }

    /// doctor's line for this repo: where git's credential comes from and
    /// what it carries.
    pub fn doctor_line(&self) -> String {
        let head = format!("git token for {} on {}", self.repo, self.host_name);
        match &self.credential {
            Credential::WorkspaceSecret { name } => format!(
                "{head}: the workspace secret \"{name}\" answers git there before the host's own login, so the token is that secret and carries whatever its owner gave it"
            ),
            Credential::HostWide { why } => format!("{head}: {why}"),
            Credential::AppToken {
                set, is_default, ..
            } => {
                let whose = if *is_default {
                    "your default set"
                } else {
                    "the set an admin gave you"
                };
                match (self.carries(), self.shortfall()) {
                    (Some(carries), _) if carries.is_empty() => format!(
                        "{head}: carries nothing — the GitHub App's installation holds none of {whose} ({set}), so git gets no credential there"
                    ),
                    (Some(carries), Some(short)) if !short.is_empty() => format!(
                        "{head}: carries {carries} ({whose}); short of {short}, which the GitHub App's installation does not hold yet, so no token carries it until an owner accepts it on GitHub"
                    ),
                    (Some(carries), _) => format!("{head}: carries {carries} ({whose})"),
                    (None, _) => format!(
                        "{head}: asks for {set} ({whose}); what the GitHub App's installation holds is not on record yet, so it carries as much of that as the installation holds"
                    ),
                }
            }
        }
    }
}

/// The REST origin a GitHub row's API answers on: its own `api_url`, else
/// the one its web origin implies (api.github.com, or `<server>/api/v3`).
fn api_of(row: &HostRow) -> Option<String> {
    (row.kind == HostKind::Github).then(|| {
        row.api_url
            .as_deref()
            .map(|a| a.trim_end_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| api_origin_for(row.base_url.trim_end_matches('/')))
    })
}

/// One granted repo's credential, from what the record says: `set` is the
/// agent's set on the host (None: the default), `selected` and
/// `installations` the host's tracked installations and their rows, and
/// `secret` the workspace secret that answers git at the host first, when
/// one does.
pub fn credential_of(
    row: &HostRow,
    repo: &str,
    set: Option<&PermissionSet>,
    selected: &[String],
    installations: &[InstallationRow],
    secret: Option<String>,
) -> GrantCredential {
    let credential = match (secret, crate::narrowing_of(row)) {
        (Some(name), _) => Credential::WorkspaceSecret { name },
        (None, Narrowing::HostWide(why)) => Credential::HostWide { why },
        (None, Narrowing::PerRepo) => {
            let on = installation_for_repo(selected, installations, repo);
            Credential::AppToken {
                set: set.cloned().unwrap_or_else(default_set),
                is_default: set.is_none(),
                installation: on.map(|r| r.installation_id.clone()),
                held: on.and_then(|r| r.held.clone()),
            }
        }
    };
    GrantCredential {
        host_id: row.id.clone(),
        host_name: row.name.clone(),
        kind: row.kind,
        repo: repo.to_string(),
        api: api_of(row),
        credential,
    }
}

/// Every one of `grants`' credentials for `agent_id` (whose model name,
/// `agent_model`, is what the workspace store grants by), in the grants'
/// order. Reads the hosts, the agent's sets and the holdings record once,
/// and the workspace store once per host.
pub async fn grant_credentials(
    pg: &PgPool,
    agent_id: &str,
    agent_model: &str,
    grants: &[RepoRef],
) -> Result<Vec<GrantCredential>, String> {
    if grants.is_empty() {
        return Ok(Vec::new());
    }
    let rows = host_rows(pg).await?;
    let sets = permissions::agent_permissions(pg, agent_id).await?;
    let installations = all_installation_rows(pg).await?;
    let mut secrets: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::with_capacity(grants.len());
    for grant in grants {
        let Some(row) = rows.iter().find(|r| r.id == grant.host_id) else {
            continue;
        };
        let secret = match secrets.get(&row.id) {
            Some(s) => s.clone(),
            None => {
                // The door's own scope: a secret also spendable at another
                // registered host answers only through that host's check.
                let withheld = crate::withheld_names(&rows, Some(row.id.as_str()));
                let s = talaria_workspace_secrets::secret_answering_git_at(
                    pg,
                    agent_model,
                    &row.authority(),
                    &withheld,
                )
                .await?;
                secrets.insert(row.id.clone(), s.clone());
                s
            }
        };
        let mine: Vec<InstallationRow> = installations
            .iter()
            .filter(|r| r.host_id == row.id)
            .cloned()
            .collect();
        let set = sets
            .iter()
            .find(|s| s.host_id == row.id)
            .map(|s| &s.permissions);
        out.push(credential_of(
            row,
            &grant.repo,
            set,
            &tracked_installations(row),
            &mine,
            secret,
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(kind: HostKind, auth: Value) -> HostRow {
        HostRow {
            id: "gh".into(),
            kind,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            api_url: None,
            auth,
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn app() -> HostRow {
        host(
            HostKind::Github,
            json!({ "mode": "app", "app": { "appId": "7", "installationIds": ["91"], "privateKeyEnc": "sealed" } }),
        )
    }

    fn installation(held: Option<PermissionSet>) -> Vec<InstallationRow> {
        vec![InstallationRow {
            host_id: "gh".into(),
            installation_id: "91".into(),
            account: "acme".into(),
            held,
            ..InstallationRow::default()
        }]
    }

    fn set(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    #[test]
    fn a_github_app_token_carries_the_set_met_with_what_the_installation_holds() {
        let held = set(&[
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
        ]);
        let selected = ["91".to_string()];
        // The default, all of it held.
        let c = credential_of(
            &app(),
            "acme/widgets",
            None,
            &selected,
            &installation(Some(held.clone())),
            None,
        );
        assert_eq!(c.carries(), Some(default_set()));
        assert_eq!(c.shortfall(), Some(PermissionSet::new()));
        let wire = c.github_wire();
        assert_eq!(wire["narrowing"], "perRepo");
        assert_eq!(wire["isDefault"], true);
        assert_eq!(wire["installation"], "91");
        assert_eq!(wire["api"], "https://api.github.com");
        assert_eq!(
            wire["carries"],
            json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
        );
        assert_eq!(wire["shortfall"], json!({}));
        assert_eq!(
            c.doctor_line(),
            "git token for acme/widgets on GitHub: carries contents:write, metadata:read, pull_requests:write (your default set)"
        );

        // An admin's set, wider than the installation holds and without
        // metadata: GitHub puts metadata on the token anyway.
        let given = set(&[("contents", Level::Read), ("workflows", Level::Write)]);
        let c = credential_of(
            &app(),
            "acme/widgets",
            Some(&given),
            &selected,
            &installation(Some(held.clone())),
            None,
        );
        assert_eq!(
            c.carries(),
            Some(set(&[("contents", Level::Read), ("metadata", Level::Read)]))
        );
        assert_eq!(c.shortfall(), Some(set(&[("workflows", Level::Write)])));
        assert_eq!(c.github_wire()["isDefault"], false);
        assert_eq!(c.github_wire()["set"], given.to_json());
        assert_eq!(
            c.doctor_line(),
            "git token for acme/widgets on GitHub: carries contents:read, metadata:read (the set an admin gave you); short of workflows:write, which the GitHub App's installation does not hold yet, so no token carries it until an owner accepts it on GitHub"
        );

        // A set the installation holds none of mints nothing.
        let none = set(&[("secrets", Level::Write)]);
        let c = credential_of(
            &app(),
            "acme/widgets",
            Some(&none),
            &selected,
            &installation(Some(held)),
            None,
        );
        assert_eq!(c.carries(), Some(PermissionSet::new()));
        assert!(
            c.doctor_line().contains("carries nothing"),
            "{}",
            c.doctor_line()
        );
        assert!(
            c.doctor_line().contains("no credential"),
            "{}",
            c.doctor_line()
        );
    }

    #[test]
    fn an_installation_never_read_is_unknown_never_nothing() {
        let c = credential_of(
            &app(),
            "acme/widgets",
            None,
            &["91".to_string()],
            &installation(None),
            None,
        );
        assert_eq!(c.carries(), None);
        assert_eq!(c.shortfall(), None);
        let wire = c.github_wire();
        assert_eq!(wire["carries"], Value::Null);
        assert_eq!(wire["shortfall"], Value::Null);
        assert_eq!(wire["set"], default_set().to_json());
        assert!(
            c.doctor_line().contains(
                "asks for contents:write, metadata:read, pull_requests:write (your default set)"
            ),
            "{}",
            c.doctor_line()
        );
        assert!(
            c.doctor_line().contains("not on record yet"),
            "{}",
            c.doctor_line()
        );
        // No row for the installation at all: the same.
        let c = credential_of(&app(), "acme/widgets", None, &["91".to_string()], &[], None);
        assert_eq!(c.carries(), None);
        assert_eq!(c.github_wire()["installation"], Value::Null);
    }

    #[test]
    fn a_host_wide_token_and_a_workspace_secret_claim_nothing_about_what_they_carry() {
        // A PAT: the host's own token decides.
        let pat = host(
            HostKind::Github,
            json!({ "mode": "pat", "pat": { "tokenEnc": "sealed" } }),
        );
        let c = credential_of(&pat, "acme/widgets", None, &[], &[], None);
        assert_eq!(c.carries(), None);
        let wire = c.github_wire();
        assert_eq!(wire["narrowing"], "hostWide");
        assert_eq!(wire["why"], talaria_github::PAT_HOST_WIDE);
        assert_eq!(wire["carries"], Value::Null);
        assert_eq!(
            c.doctor_line(),
            format!(
                "git token for acme/widgets on GitHub: {}",
                talaria_github::PAT_HOST_WIDE
            )
        );

        // Forgejo: host-wide too, and no `github` member.
        let mut forge = host(HostKind::Forgejo, json!({ "secret": "forge-token" }));
        forge.name = "Forge".into();
        forge.base_url = "https://git.example.com".into();
        let c = credential_of(&forge, "acme/widgets", None, &[], &[], None);
        assert_eq!(c.github_wire(), Value::Null);
        assert_eq!(c.api, None);
        assert_eq!(
            c.doctor_line(),
            format!(
                "git token for acme/widgets on Forge: {}",
                talaria_forgejo::HOST_WIDE
            )
        );

        // A workspace secret answers first, even on an App host with a set.
        let given = set(&[("administration", Level::Write)]);
        let c = credential_of(
            &app(),
            "acme/widgets",
            Some(&given),
            &["91".to_string()],
            &installation(Some(given.clone())),
            Some("github-pat".into()),
        );
        assert_eq!(c.carries(), None);
        assert_eq!(c.shortfall(), None);
        let wire = c.github_wire();
        assert_eq!(wire["narrowing"], "workspaceSecret");
        assert_eq!(wire["secret"], "github-pat");
        assert!(
            c.doctor_line()
                .contains("the workspace secret \"github-pat\" answers git there"),
            "{}",
            c.doctor_line()
        );
    }

    #[test]
    fn an_enterprise_server_answers_its_api_under_api_v3_or_where_the_row_says() {
        let mut ghes = app();
        ghes.base_url = "https://git.corp.example/".into();
        assert_eq!(
            api_of(&ghes).as_deref(),
            Some("https://git.corp.example/api/v3")
        );
        ghes.api_url = Some("https://api.git.corp.example/".into());
        assert_eq!(
            api_of(&ghes).as_deref(),
            Some("https://api.git.corp.example")
        );
    }
}
