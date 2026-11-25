// The ClickUp v2 shapes the board sync reads, parsed from `serde_json::Value`
// rather than derived: ClickUp spells one id as a number in one response and a
// string in the next (a user id is 183 on a comment and "183" on a task's
// creator), dates are epoch-ms STRINGS, and `reply_count` has been both. A
// derive would fail the whole page over one field; these readers take either
// spelling and drop what they cannot read.

use serde::Serialize;
use serde_json::Value;

/// An id, whether ClickUp sent it as a string or a number.
pub(crate) fn id_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Epoch milliseconds, from `"1567780450202"` or `1567780450202`.
pub(crate) fn ms_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// A count, from a number or a numeric string; 0 when absent.
pub(crate) fn count_of(v: Option<&Value>) -> i64 {
    ms_of(v).unwrap_or(0).max(0)
}

pub(crate) fn text_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn array<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter())
        .into_iter()
        .flatten()
}

/// The ClickUp identity a token acts as (GET /user).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
}

impl Account {
    pub fn parse(body: &Value) -> Option<Account> {
        let u = body.get("user").unwrap_or(body);
        Some(Account {
            id: id_of(u.get("id"))?,
            username: text_of(u.get("username")).unwrap_or_default(),
            email: text_of(u.get("email")),
        })
    }
}

/// A workspace (ClickUp calls it a team), a space or a folder: the levels a
/// person walks to pick a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    pub id: String,
    pub name: String,
}

impl Named {
    fn parse(v: &Value) -> Option<Named> {
        Some(Named {
            id: id_of(v.get("id"))?,
            name: text_of(v.get("name")).unwrap_or_default(),
        })
    }

    /// Every entry of `body[key]` that has an id.
    pub fn parse_all(body: &Value, key: &str) -> Vec<Named> {
        array(body, key).filter_map(Named::parse).collect()
    }
}

/// A list as a picker shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRef {
    pub id: String,
    pub name: String,
    pub task_count: Option<i64>,
}

impl ListRef {
    pub fn parse_all(body: &Value) -> Vec<ListRef> {
        array(body, "lists")
            .filter_map(|l| {
                Some(ListRef {
                    id: id_of(l.get("id"))?,
                    name: text_of(l.get("name")).unwrap_or_default(),
                    task_count: ms_of(l.get("task_count")),
                })
            })
            .collect()
    }
}

/// One of a list's effective statuses (GET /list/{id}).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListStatus {
    pub status: String,
    pub color: Option<String>,
    /// ClickUp's `type`: open, custom, done, closed. Shown in the preview; the
    /// mapping is keyed by name, never by this.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub orderindex: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListInfo {
    pub id: String,
    pub name: String,
    pub space: Option<Named>,
    pub folder: Option<Named>,
    pub statuses: Vec<ListStatus>,
    pub task_count: Option<i64>,
}

impl ListInfo {
    pub fn parse(body: &Value) -> Option<ListInfo> {
        let folder = body.get("folder").and_then(|f| {
            // A folderless list sits in a hidden folder ClickUp still reports.
            if f.get("hidden").and_then(Value::as_bool) == Some(true) {
                None
            } else {
                Named::parse(f)
            }
        });
        let mut statuses: Vec<ListStatus> = array(body, "statuses")
            .filter_map(|s| {
                Some(ListStatus {
                    status: text_of(s.get("status"))?,
                    color: text_of(s.get("color")),
                    kind: text_of(s.get("type")),
                    orderindex: ms_of(s.get("orderindex")).unwrap_or(0),
                })
            })
            .collect();
        statuses.sort_by_key(|s| s.orderindex);
        Some(ListInfo {
            id: id_of(body.get("id"))?,
            name: text_of(body.get("name")).unwrap_or_default(),
            space: body.get("space").and_then(Named::parse),
            folder,
            statuses,
            task_count: ms_of(body.get("task_count")),
        })
    }

    /// "Space / Folder / List", the line a person recognises.
    pub fn path(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if let Some(s) = &self.space
            && !s.name.is_empty()
        {
            parts.push(&s.name);
        }
        if let Some(f) = &self.folder
            && !f.name.is_empty()
        {
            parts.push(&f.name);
        }
        parts.push(&self.name);
        parts.join(" / ")
    }
}

/// A ClickUp member as a task or a comment names them. ClickUp withholds the
/// email for some members (guests, privacy settings), so it is optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: Option<String>,
    pub username: Option<String>,
    pub email: Option<String>,
}

impl Person {
    fn parse(v: &Value) -> Person {
        Person {
            id: id_of(v.get("id")),
            username: text_of(v.get("username")),
            email: text_of(v.get("email")).map(|e| e.trim().to_lowercase()),
        }
    }

    /// What to call them: the username, else the email's local part, else
    /// "a ClickUp member".
    pub fn display(&self) -> String {
        if let Some(u) = self.username.as_deref().filter(|u| !u.trim().is_empty()) {
            return u.trim().to_string();
        }
        if let Some(e) = &self.email {
            return e.split('@').next().unwrap_or("").to_string();
        }
        "a ClickUp member".into()
    }
}

/// A file on a task (GET /task/{id} only — the list endpoint leaves them out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub title: String,
    pub size: Option<i64>,
    pub mime: Option<String>,
    pub url: Option<String>,
}

impl Attachment {
    fn parse(v: &Value) -> Option<Attachment> {
        if v.get("deleted").and_then(Value::as_bool) == Some(true)
            || v.get("is_folder").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        let id = id_of(v.get("id"))?;
        let ext = text_of(v.get("extension"));
        let title = text_of(v.get("title")).unwrap_or_else(|| {
            format!(
                "attachment{}",
                ext.map(|e| format!(".{e}")).unwrap_or_default()
            )
        });
        Some(Attachment {
            id,
            title,
            size: ms_of(v.get("size")),
            mime: text_of(v.get("mimetype")),
            url: text_of(v.get("url")).or_else(|| text_of(v.get("url_w_host"))),
        })
    }
}

/// A task, from the list page or from GET /task/{id}.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub custom_id: Option<String>,
    pub name: String,
    /// markdown_description, else text_content, else description.
    pub description: Option<String>,
    pub status: String,
    pub status_type: Option<String>,
    /// ClickUp's priority id: 1 urgent, 2 high, 3 normal, 4 low; None = none.
    pub priority: Option<u8>,
    pub assignees: Vec<Person>,
    pub tags: Vec<String>,
    pub due_ms: Option<i64>,
    pub start_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub updated_ms: Option<i64>,
    pub closed_ms: Option<i64>,
    pub parent: Option<String>,
    pub top_level_parent: Option<String>,
    /// The task's home list, and the other lists it sits in (ClickUp's
    /// "tasks in multiple lists").
    pub list_id: Option<String>,
    pub locations: Vec<String>,
    pub archived: bool,
    pub url: Option<String>,
    pub creator: Option<Person>,
    /// Present only when the response carried them (GET /task/{id}).
    pub attachments: Option<Vec<Attachment>>,
}

impl Task {
    pub fn parse(v: &Value) -> Option<Task> {
        let status = v.get("status");
        let priority = v.get("priority").and_then(|p| {
            ms_of(p.get("id"))
                .filter(|n| (1..=4).contains(n))
                .map(|n| n as u8)
                .or_else(
                    || match text_of(p.get("priority"))?.to_lowercase().as_str() {
                        "urgent" => Some(1),
                        "high" => Some(2),
                        "normal" => Some(3),
                        "low" => Some(4),
                        _ => None,
                    },
                )
        });
        Some(Task {
            id: id_of(v.get("id"))?,
            custom_id: text_of(v.get("custom_id")),
            name: text_of(v.get("name")).unwrap_or_else(|| "(untitled)".into()),
            description: text_of(v.get("markdown_description"))
                .or_else(|| text_of(v.get("text_content")))
                .or_else(|| text_of(v.get("description"))),
            status: status
                .and_then(|s| text_of(s.get("status")))
                .unwrap_or_default(),
            status_type: status.and_then(|s| text_of(s.get("type"))),
            priority,
            assignees: array(v, "assignees").map(Person::parse).collect(),
            tags: array(v, "tags")
                .filter_map(|t| text_of(t.get("name")))
                .collect(),
            due_ms: ms_of(v.get("due_date")),
            start_ms: ms_of(v.get("start_date")),
            created_ms: ms_of(v.get("date_created")),
            updated_ms: ms_of(v.get("date_updated")),
            closed_ms: ms_of(v.get("date_closed")),
            parent: id_of(v.get("parent")),
            top_level_parent: id_of(v.get("top_level_parent")),
            list_id: v.get("list").and_then(|l| id_of(l.get("id"))),
            locations: array(v, "locations")
                .filter_map(|l| id_of(l.get("id")))
                .collect(),
            archived: v.get("archived").and_then(Value::as_bool).unwrap_or(false),
            url: text_of(v.get("url")),
            creator: v
                .get("creator")
                .filter(|c| c.is_object())
                .map(Person::parse),
            attachments: v
                .get("attachments")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Attachment::parse).collect()),
        })
    }

    /// Is the task in `list_id` — its home list, or one of its other lists?
    pub fn in_list(&self, list_id: &str) -> bool {
        self.list_id.as_deref() == Some(list_id) || self.locations.iter().any(|l| l == list_id)
    }

    /// Closed in ClickUp: the status type says so, or the task carries a
    /// closed date.
    pub fn closed(&self) -> bool {
        matches!(self.status_type.as_deref(), Some("closed") | Some("done"))
            || self.closed_ms.is_some()
    }
}

/// One page of GET /list/{id}/task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasksPage {
    pub tasks: Vec<Task>,
    /// ClickUp's own flag. When the response omits it, a page shorter than
    /// 100 is the last one.
    pub last_page: bool,
}

impl TasksPage {
    pub fn parse(body: &Value) -> TasksPage {
        let tasks: Vec<Task> = array(body, "tasks").filter_map(Task::parse).collect();
        let last_page = body
            .get("last_page")
            .and_then(Value::as_bool)
            .unwrap_or(tasks.len() < crate::PAGE_SIZE);
        TasksPage { tasks, last_page }
    }
}

/// A comment or a threaded reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub text: String,
    pub user: Option<Person>,
    pub date_ms: Option<i64>,
    pub reply_count: i64,
}

impl Comment {
    pub fn parse_all(body: &Value) -> Vec<Comment> {
        array(body, "comments")
            .filter_map(|c| {
                let text = text_of(c.get("comment_text")).unwrap_or_else(|| {
                    // The structured form, flattened, when comment_text is
                    // missing.
                    c.get("comment")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|p| p.get("text").and_then(Value::as_str))
                                .collect::<String>()
                        })
                        .unwrap_or_default()
                });
                Some(Comment {
                    id: id_of(c.get("id"))?,
                    text,
                    user: c.get("user").filter(|u| u.is_object()).map(Person::parse),
                    date_ms: ms_of(c.get("date")),
                    reply_count: count_of(c.get("reply_count")),
                })
            })
            .collect()
    }
}
