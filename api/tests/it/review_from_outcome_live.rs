// review_from_outcome_live (see main.rs): finished agent work reaches the
// review queues from the agent's REPORT, not from a column, against a real
// database. A work session never moves its ticket (talaria-tasks
// placement.rs), so the ticket stays where a person put it, and each surface
// that used to read the review column (or Blocked) reads the report too:
//
//   approvals   talaria_approvals::pending_approvals — a ticket_review entry
//   brief       talaria_daily_brief_focus::task_items — a REVIEW card, or a
//               BLOCKED one carrying the error as its evidence
//   inbox       talaria_inbox_focus::sources::task_items — the same cards
//   home        talaria_home::home_queues — the review / blocked buckets, the
//               blocked item carrying its reason (and the digest reads these)
//
// Each one clears when a PERSON acts — approves, sends it back, or moves the
// ticket — and never when the agent writes again.
//
// House rule: #[ignore]d, never CI's default job.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… TALARIA_SECRET_KEY=… \
//     cargo test -p talaria-api --test it review_from_outcome_live:: -- --ignored

use sqlx::PgPool;
use talaria_api::tasks::{self, TaskActor, TaskPatch};

use crate::support::live_app_state;

const PREFIX: &str = "review-outcome-live";

struct Fixture {
    pg: PgPool,
    user: String,
    email: String,
    board: String,
    agent: String,
    deps: tasks::TaskDeps,
}

impl Fixture {
    async fn new(tag: &str) -> Fixture {
        let state = live_app_state().await;
        let pg = state.pg.clone();
        let n = uuid::Uuid::new_v4().simple().to_string();
        let email = format!("{PREFIX}-{tag}-{n}@review-test.invalid");
        let (user,): (String,) = sqlx::query_as(
            "insert into users (sub, email, name, role) values ($1, $1, 'Review Live', 'member') \
             returning id::text",
        )
        .bind(&email)
        .fetch_one(&pg)
        .await
        .unwrap();
        let (board,): (String,) = sqlx::query_as(
            "insert into boards (name, owner_id) values ('review-live', $1::uuid) returning id::text",
        )
        .bind(&user)
        .fetch_one(&pg)
        .await
        .unwrap();
        sqlx::query(
            "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
        )
        .bind(&board)
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
        let agent = format!("{PREFIX}-{tag}-{n}");
        sqlx::query(
            "insert into agent_defs (slug, department, model, display_name) \
             values ($1, 'personal', $1, 'Review Live Agent')",
        )
        .bind(&agent)
        .execute(&pg)
        .await
        .unwrap();
        sqlx::query("insert into board_agents (board_id, agent_model) values ($1::uuid, $2)")
            .bind(&board)
            .bind(&agent)
            .execute(&pg)
            .await
            .unwrap();
        let deps = tasks::TaskDeps::from_route(pg.clone(), None);
        Fixture {
            pg,
            user,
            email,
            board,
            agent,
            deps,
        }
    }

    fn person(&self) -> TaskActor {
        TaskActor::person(self.user.clone(), self.email.clone())
    }

    /// A ticket a person created and placed in the agent's pickup column.
    async fn placed(&self, title: &str) -> String {
        let task = tasks::create_task(
            &self.deps,
            &tasks::NewTask {
                board_id: &self.board,
                title,
                description: None,
                priority: None,
                effort: None,
                assignees: std::slice::from_ref(&self.agent),
                due_date: None,
                start_date: None,
                color: None,
                estimated_hours: None,
                parent_id: None,
                tags: &[],
                created_by: &self.email,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("create: {}", e.message()));
        self.move_to(&task.id, "assigned").await;
        task.id
    }

    async fn move_to(&self, id: &str, status: &str) {
        tasks::update_task(
            &self.deps,
            id,
            TaskPatch {
                status: Some(status.into()),
                ..Default::default()
            },
            &self.person(),
        )
        .await
        .unwrap_or_else(|e| panic!("a person's move to {status}: {}", e.message()));
    }

    /// The agent's report, through the write report_outcome makes.
    async fn report(&self, id: &str, outcome: &str, error: Option<&str>) {
        tasks::update_task(
            &self.deps,
            id,
            TaskPatch {
                outcome: Some(Some(outcome.into())),
                error_message: error.map(|e| Some(e.to_string())),
                ..Default::default()
            },
            &TaskActor::agent(self.agent.clone()),
        )
        .await
        .unwrap_or_else(|e| panic!("the agent's report: {}", e.message()));
    }

    async fn status(&self, id: &str) -> String {
        sqlx::query_scalar("select status from tasks where id = $1::uuid")
            .bind(id)
            .fetch_one(&self.pg)
            .await
            .unwrap()
    }

    /// Where the ticket shows up, surface by surface: the approvals inbox's
    /// ticket_review, the brief's and the inbox's card label, and Home's
    /// bucket (with the blocked item's reason).
    async fn surfaces(&self, id: &str) -> Surfaces {
        let none: talaria_runs_run::DefinitionForFn = std::sync::Arc::new(|_| None);
        let approvals = talaria_approvals::pending_approvals(&self.pg, &none).await;
        let approval = approvals
            .approvals
            .iter()
            .any(|a| a.key == format!("ticket_review:{id}"));
        let brief = talaria_daily_brief_focus::task_items(&self.pg, &self.user)
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.source_id == id)
            .map(|i| {
                let failure = i
                    .evidence
                    .iter()
                    .find(|e| e.label == "Failure")
                    .map(|e| e.text.clone());
                (i.status_label, i.action_ids, failure)
            });
        let inbox = talaria_inbox_focus::sources::task_items(&self.pg, &self.user, Some(id))
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.source_id == id)
            .map(|i| i.status_label);
        let queues = talaria_home::home_queues(&self.pg, &self.user)
            .await
            .unwrap();
        let home = if let Some(w) = queues.review.items.iter().find(|w| w.id == id) {
            Some(("review".to_string(), w.reason.clone()))
        } else {
            queues
                .blocked
                .items
                .iter()
                .find(|w| w.id == id)
                .map(|w| ("blocked".to_string(), w.reason.clone()))
        };
        Surfaces {
            approval,
            brief,
            inbox,
            home,
        }
    }

    async fn drop(self) {
        sqlx::query("delete from agent_defs where model = $1")
            .bind(&self.agent)
            .execute(&self.pg)
            .await
            .unwrap();
        sqlx::query("delete from users where id = $1::uuid")
            .bind(&self.user)
            .execute(&self.pg)
            .await
            .unwrap();
    }
}

#[derive(Debug)]
struct Surfaces {
    approval: bool,
    brief: Option<(String, Vec<String>, Option<String>)>,
    inbox: Option<String>,
    home: Option<(String, Option<String>)>,
}

impl Surfaces {
    fn assert_review(&self, what: &str) {
        assert!(
            self.approval,
            "{what}: no ticket_review approval — {self:?}"
        );
        let brief = self
            .brief
            .as_ref()
            .unwrap_or_else(|| panic!("{what}: not in the brief — {self:?}"));
        assert_eq!(brief.0, "REVIEW", "{what}: brief label");
        assert_eq!(
            brief.1,
            ["approve_task", "request_changes"],
            "{what}: the card's buttons"
        );
        assert_eq!(self.inbox.as_deref(), Some("REVIEW"), "{what}: inbox card");
        assert_eq!(
            self.home.as_ref().map(|h| h.0.as_str()),
            Some("review"),
            "{what}: home"
        );
    }

    fn assert_blocked(&self, what: &str, reason: &str) {
        assert!(
            !self.approval,
            "{what}: an error is not a sign-off — {self:?}"
        );
        let brief = self
            .brief
            .as_ref()
            .unwrap_or_else(|| panic!("{what}: not in the brief — {self:?}"));
        assert_eq!(brief.0, "BLOCKED", "{what}: brief label");
        assert_eq!(
            brief.2.as_deref(),
            Some(reason),
            "{what}: the brief carries the reason"
        );
        assert_eq!(self.inbox.as_deref(), Some("BLOCKED"), "{what}: inbox card");
        assert_eq!(
            self.home.clone(),
            Some(("blocked".to_string(), Some(reason.to_string()))),
            "{what}: home's blocked bucket, with its reason"
        );
    }

    /// Off every review / blocked surface. The brief and the inbox list
    /// tickets for other reasons (triage), so those may still hold it — just
    /// not as review or blocked work.
    fn assert_cleared(&self, what: &str) {
        assert!(!self.approval, "{what}: still a ticket_review approval");
        if let Some(b) = &self.brief {
            assert!(
                b.0 != "REVIEW" && b.0 != "BLOCKED",
                "{what}: brief still says {}",
                b.0
            );
        }
        if let Some(l) = &self.inbox {
            assert!(
                l != "REVIEW" && l != "BLOCKED",
                "{what}: inbox still says {l}"
            );
        }
        if let Some((q, _)) = &self.home {
            panic!("{what}: home still shows it as {q}");
        }
    }
}

#[tokio::test]
#[ignore]
async fn an_outcome_reaches_every_review_queue_and_a_person_clears_it() {
    let f = Fixture::new("outcome").await;

    // Placed and not yet reported: no review queue holds it.
    let approve = f.placed("Approve me").await;
    f.surfaces(&approve)
        .await
        .assert_cleared("before the report");

    // The agent reports. The ticket stays in Assigned, and every surface
    // that used to read the review column holds it as review work.
    f.report(&approve, "fixed it; PR #1", None).await;
    assert_eq!(
        f.status(&approve).await,
        "assigned",
        "the report moved the ticket"
    );
    f.surfaces(&approve).await.assert_review("reported");

    // The agent writing again (a field, or the same report) does not clear it.
    tasks::update_task(
        &f.deps,
        &approve,
        TaskPatch {
            tags: Some(vec!["triaged".into()]),
            ..Default::default()
        },
        &TaskActor::agent(f.agent.clone()),
    )
    .await
    .unwrap_or_else(|e| panic!("agent triage: {}", e.message()));
    f.report(&approve, "fixed it; PR #1", None).await;
    f.surfaces(&approve)
        .await
        .assert_review("after the agent wrote again");

    // A person APPROVES it on the review card: off every queue, still in its
    // column, and signed off.
    tasks::complete_quality_review(&f.deps, &approve, &f.person(), "approved", "assigned")
        .await
        .unwrap_or_else(|e| panic!("approve: {}", e.message()))
        .expect("the card's approval applies to a reported outcome");
    assert_eq!(
        f.status(&approve).await,
        "assigned",
        "an approval moved the ticket"
    );
    f.surfaces(&approve).await.assert_cleared("approved");
    // A NEW report after that sign-off is new review work again.
    f.report(&approve, "follow-up fixed too", None).await;
    f.surfaces(&approve)
        .await
        .assert_review("a new report after a sign-off");

    // A person SENDS IT BACK in place (the ticket's Request changes, already
    // in its working column): off the queues, and the placement renewed so
    // the agent may pick it up again.
    let back = f.placed("Send me back").await;
    f.report(&back, "half done", None).await;
    f.surfaces(&back)
        .await
        .assert_review("reported (send-back case)");
    let before: Option<String> =
        sqlx::query_scalar("select placed_at::text from tasks where id = $1::uuid")
            .bind(&back)
            .fetch_one(&f.pg)
            .await
            .unwrap();
    tasks::add_review(&f.deps, &back, &f.email, "rejected", Some("needs tests"))
        .await
        .unwrap();
    tasks::send_back(&f.deps, &back, "assigned", &f.person())
        .await
        .unwrap_or_else(|e| panic!("send back: {}", e.message()));
    assert_eq!(f.status(&back).await, "assigned");
    let after: Option<String> =
        sqlx::query_scalar("select placed_at::text from tasks where id = $1::uuid")
            .bind(&back)
            .fetch_one(&f.pg)
            .await
            .unwrap();
    assert_ne!(
        before, after,
        "sending it back in place renews the person's placement"
    );
    f.surfaces(&back).await.assert_cleared("sent back");

    // THE COLUMN STILL COUNTS, beside the report: a ticket a person put in
    // the review column, with no report at all, is review work as before.
    let column = f.placed("Parked in review").await;
    f.move_to(&column, "quality_review").await;
    f.surfaces(&column)
        .await
        .assert_review("in the review column, unreported");

    // A person MOVES it: off the queues.
    let moved = f.placed("Move me").await;
    f.report(&moved, "done", None).await;
    f.surfaces(&moved)
        .await
        .assert_review("reported (move case)");
    f.move_to(&moved, "in_progress").await;
    f.surfaces(&moved).await.assert_cleared("moved by a person");

    f.drop().await;
}

#[tokio::test]
#[ignore]
async fn an_error_report_is_blocked_work_with_its_reason_until_a_person_acts() {
    let f = Fixture::new("error").await;
    let id = f.placed("Blocked by the vendor").await;
    f.report(
        &id,
        "could not finish",
        Some("waiting on the vendor's API key"),
    )
    .await;
    assert_eq!(
        f.status(&id).await,
        "assigned",
        "the report moved the ticket"
    );
    f.surfaces(&id)
        .await
        .assert_blocked("reported an error", "waiting on the vendor's API key");
    // The agent cannot clear it by writing again.
    tasks::update_task(
        &f.deps,
        &id,
        TaskPatch {
            tags: Some(vec!["vendor".into()]),
            ..Default::default()
        },
        &TaskActor::agent(f.agent.clone()),
    )
    .await
    .unwrap_or_else(|e| panic!("agent triage: {}", e.message()));
    f.surfaces(&id).await.assert_blocked(
        "after the agent wrote again",
        "waiting on the vendor's API key",
    );
    // A person moves it: no longer blocked work.
    f.move_to(&id, "inbox").await;
    f.surfaces(&id).await.assert_cleared("moved by a person");
    // THE COLUMN STILL COUNTS: a ticket a person put in Blocked, with no
    // report, is blocked work as before (no reason line — nobody gave one).
    let parked = f.placed("Parked in Blocked").await;
    f.move_to(&parked, "blocked").await;
    let s = f.surfaces(&parked).await;
    assert_eq!(
        s.brief.as_ref().map(|b| b.0.as_str()),
        Some("BLOCKED"),
        "{s:?}"
    );
    assert_eq!(s.inbox.as_deref(), Some("BLOCKED"), "{s:?}");
    assert_eq!(s.home, Some(("blocked".to_string(), None)), "{s:?}");
    f.drop().await;
}
