// Who can read conversation `c`, as one table of membership paths rendered
// two ways: `viewer_sql` asks whether one person is a reader (the read gates,
// the listing, the unread badge, the working read), and `readers_sql` lists
// every reader (the firehose audience). Both walk the same table, so a path
// added here reaches both directions at once.
//
// THE VIEWER RENDERING KEEPS ITS LEGS UNCORRELATED. Each leg is
// `<key> in (select <link> from <via> where <reader> = $viewer)`, which
// Postgres hashes once per query. A correlated leg, or `$viewer in (<the
// reader set>)`, enumerated a shared team per row: 5 s for 2,000 plans shared
// with a 9,999-person team, against 5 ms. And it keeps each group's kind test
// outside its subquery, so the plan's estimated cost stays under the JIT
// threshold the old spelling stayed under.

/// One way to reach a reader from a group's key.
struct Leg {
    /// The column that names the reader.
    reader: &'static str,
    /// The tables that hold it and the column joining them to the key. None
    /// when the reader is a column of the anchor itself.
    via: Option<(&'static str, &'static str)>,
}

/// Paths that apply to one set of kinds and start from one key: `c.id`, or
/// a row the anchor finds for `c`.
struct Group {
    kind: &'static str,
    anchor: Option<(&'static str, &'static str)>,
    key: &'static str,
    legs: &'static [Leg],
}

/// Beyond the owner, who always reads. Chats have no group, so they admit only
/// their owner.
const READER_PATHS: &[Group] = &[
    Group {
        kind: "c.kind in ('plan', 'work')",
        anchor: None,
        key: "c.id",
        legs: &[
            Leg {
                reader: "cm.user_id",
                via: Some(("conversation_members cm", "cm.conversation_id")),
            },
            Leg {
                reader: "tm.user_id",
                via: Some((
                    "conversation_teams ct join team_members tm on tm.team_id = ct.team_id",
                    "ct.conversation_id",
                )),
            },
        ],
    },
    Group {
        kind: "c.kind = 'research'",
        anchor: Some(("research_runs r", "r.conversation_id = c.id")),
        key: "r.id",
        legs: &[
            Leg {
                reader: "r.owner_user_id",
                via: None,
            },
            Leg {
                reader: "rm.user_id",
                via: Some(("research_members rm", "rm.run_id")),
            },
            Leg {
                reader: "tm.user_id",
                via: Some((
                    "research_teams rt join team_members tm on tm.team_id = rt.team_id",
                    "rt.run_id",
                )),
            },
        ],
    },
];

/// `viewer` (a bind like `"$1::uuid"`) can read conversation `c`.
pub(crate) fn viewer_sql(viewer: &str) -> String {
    let mut parts = vec![format!("c.user_id = {viewer}")];
    for group in READER_PATHS {
        let legs = group
            .legs
            .iter()
            .map(|leg| match leg.via {
                None => format!("{} = {viewer}", leg.reader),
                Some((via, link)) => format!(
                    "{} in (select {link} from {via} where {} = {viewer})",
                    group.key, leg.reader
                ),
            })
            .collect::<Vec<_>>()
            .join(" or ");
        parts.push(match group.anchor {
            None => format!("({} and ({legs}))", group.kind),
            Some((tables, cond)) => format!(
                "({} and exists(select 1 from {tables} where {cond} and ({legs})))",
                group.kind
            ),
        });
    }
    format!("({})", parts.join(" or "))
}

/// Everyone who can read conversation `c`, as a subquery body correlated on
/// `c` with one column. May repeat a reader; callers that list take
/// `distinct`.
pub(crate) fn readers_sql() -> String {
    let mut selects = vec!["select c.user_id".to_string()];
    for group in READER_PATHS {
        for leg in group.legs {
            let mut tables = Vec::new();
            let mut conds = Vec::new();
            if let Some((anchor, cond)) = group.anchor {
                tables.push(anchor);
                conds.push(cond.to_string());
            }
            if let Some((via, link)) = leg.via {
                tables.push(via);
                conds.push(format!("{link} = {}", group.key));
            }
            conds.push(group.kind.to_string());
            conds.push(format!("{} is not null", leg.reader));
            selects.push(format!(
                "select {} from {} where {}",
                leg.reader,
                tables.join(", "),
                conds.join(" and ")
            ));
        }
    }
    selects.join(" union all ")
}
