// Work pulse — what a live work session is doing RIGHT NOW, read off its watch
// tail.
//
// The run row says which turn a session is on, and the phase the driver wrote
// when it sent it: "turn 2 of 12 — waiting on doug-engineering". That line is
// the same for a session whose agent has been in `cargo test` for twenty
// minutes and for one whose turn the agent has not taken yet, and one turn can
// last most of an hour (outcrop, 2026-11-04: three sessions on one developer
// agent, each on "turn 1 of 12" for 40+ minutes, and nothing on screen said
// which was building, which was stuck, or which had failed its last six tool
// calls).
//
// The watch tail (`run-watch:<id>:tail`) already holds every frame a turn
// produced, in arrival order: the stream's tool and reasoning frames, the
// events plugin's tool results with their verdicts, the workbench tools' own
// frames. Each writer now stamps its frame with `at` (epoch ms, [`stamp`]),
// and the turn records when it began ([`turn_key`]), so the tail answers
// "which step, since when, what did it last print, what failed" without a
// second store to keep in step. The tail is the record; this is a read of it.

use serde_json::{Value, json};

/// How long a tail and its turn marker outlive their last write. Every
/// writer refreshes both ([`touch`]), so they die together, two hours after
/// the turn's last frame.
pub const TAIL_TTL_S: i64 = 7_200;

/// How much of the tail a read parses: its newest end. A turn's tail can run
/// to several hundred KB; the current step, the last output and the recent
/// failures are all at the end, so a bounded read sees what a glance needs.
/// Counts (`steps`, `failures`) are over this window, which the wire says.
pub const READ_BYTES: isize = 131_072;

/// The tail a work session's frames append to.
pub fn tail_key(run_id: &str) -> String {
    format!("run-watch:{run_id}:tail")
}

/// When the session's current turn began, `<turn>:<epoch ms>`. A key of its
/// own rather than a frame at the head of the tail: a long turn's head is past
/// [`READ_BYTES`], and the turn's age is the one number a glance always needs.
pub fn turn_key(run_id: &str) -> String {
    format!("run-watch:{run_id}:turn")
}

/// The wall clock in epoch ms — the unit every `at` is written in.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Stamp one watch line with its arrival time. Every writer passes its line
/// through here before APPEND/PUBLISH, so every frame on the tail carries
/// `at`. Only a line that parses as one JSON object gets it, and only when
/// that object has no `at` key of its own — a key test, not a text search,
/// so a command that merely MENTIONS `"at":` (an `echo '{"at":1}'` in a
/// preview) is still stamped. Anything else is returned as it was.
pub fn stamp(line: &str, at_ms: i64) -> String {
    match serde_json::from_str::<Value>(line.trim_end()) {
        Ok(Value::Object(mut frame)) if !frame.contains_key("at") => {
            frame.insert("at".into(), json!(at_ms));
            Value::Object(frame).to_string()
        }
        _ => line.to_string(),
    }
}

/// One running step: the tool and the argument the agent gave it (a terminal
/// command, an execute_code body, an omp prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub tool: String,
    pub detail: String,
    pub at: Option<i64>,
    /// The stream's id for this call (`tool` frames), once it has reported it.
    stream_id: String,
    /// The events plugin has reported this call (`toolfull` frames). One call
    /// is reported by both, in either order: the plugin's frame often lands
    /// first, because it is posted the moment the tool starts.
    plugin: bool,
}

/// A finished step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub tool: String,
    pub detail: String,
    pub ok: bool,
    pub at: Option<i64>,
    /// How long it ran, when both ends were stamped.
    pub ms: Option<i64>,
}

/// A failed step and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub tool: String,
    pub text: String,
    pub at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pulse {
    /// The newest frame's arrival — "is anything happening".
    pub last_at: Option<i64>,
    /// The innermost step still running (a terminal call inside an
    /// execute_code cell is the terminal call).
    pub step: Option<Step>,
    /// The outermost step still running, when it is not `step`.
    pub outer: Option<Step>,
    pub last: Option<Done>,
    /// The last thing a finished step printed: the tail of its output.
    pub output: Option<String>,
    /// The agent's own last sentence (its reasoning/commentary frames).
    pub said: Option<String>,
    pub steps: u32,
    pub failures: u32,
    pub failure: Option<Failure>,
    /// The window was cut: counts are over the newest [`READ_BYTES`] only.
    pub partial: bool,
}

fn str_of<'a>(f: &'a Value, k: &str) -> &'a str {
    f.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The last `max` chars of `s` on a char boundary, trimmed.
fn tail_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let skip = n - max;
    format!("…{}", s.chars().skip(skip).collect::<String>().trim_start())
}

fn head_chars(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>().trim_end())
}

/// The readable part of a plugin result: a terminal result's `output`, an
/// execute_code result's `output`, an error's `error`, else the text itself.
/// The last two non-empty lines, so "what did it last print" is a line, not a
/// log.
fn output_of(r: &Value) -> Option<String> {
    let text = match r {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(o)) => o
                .get("error")
                .and_then(Value::as_str)
                .filter(|e| !e.trim().is_empty())
                .or_else(|| o.get("output").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| s.clone()),
            _ => s.clone(),
        },
        Value::Null => return None,
        other => other.to_string(),
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let from = lines.len().saturating_sub(2);
    Some(tail_chars(&lines[from..].join(" ⏎ "), 240))
}

/// The argument worth showing from a plugin frame's `p` (the call's args as
/// JSON text): the command, the code, the prompt — else the args themselves.
fn detail_of(p: &str) -> String {
    if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(p) {
        for k in ["command", "code", "prompt", "task", "query", "path", "name"] {
            if let Some(v) = o.get(k).and_then(Value::as_str) {
                return head_chars(v, 300);
            }
        }
    }
    head_chars(p, 300)
}

/// Read a tail. `partial` says the text starts mid-tail (a bounded read):
/// its first line is dropped as possibly cut.
pub fn read_tail(tail: &str, partial: bool) -> Pulse {
    let mut p = Pulse {
        partial,
        ..Pulse::default()
    };
    let mut stack: Vec<Step> = Vec::new();
    let mut stream_fails = 0u32;
    let mut plugin_fails = 0u32;
    let mut lines = tail.lines();
    if partial {
        lines.next();
    }
    for line in lines {
        let Ok(f) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let at = f.get("at").and_then(Value::as_i64);
        if at.is_some() {
            p.last_at = at;
        }
        let tool = str_of(&f, "v").to_string();
        let status = str_of(&f, "s");
        let id = str_of(&f, "id").to_string();
        match str_of(&f, "t") {
            "tool" => match status {
                "running" => {
                    let detail = head_chars(str_of(&f, "p"), 300);
                    // The plugin already opened this call: the oldest of its
                    // calls of this tool the stream has not matched yet, so
                    // parallel calls of one tool pair up in order.
                    match stack
                        .iter_mut()
                        .find(|s| s.tool == tool && s.plugin && s.stream_id.is_empty())
                    {
                        Some(open) => {
                            open.stream_id = id;
                            if detail.chars().count() > open.detail.chars().count() {
                                open.detail = detail;
                            }
                        }
                        None => stack.push(Step {
                            tool,
                            detail,
                            at,
                            stream_id: id,
                            plugin: false,
                        }),
                    }
                }
                "completed" | "error" | "failed" => {
                    let ok = status == "completed";
                    let found = stack.iter().rposition(|s| {
                        if id.is_empty() {
                            s.tool == tool
                        } else {
                            s.stream_id == id
                        }
                    });
                    // Only the call that finished leaves the stack: with
                    // parallel tool calls, another still running stays the
                    // step.
                    let started = found.map(|i| stack.remove(i));
                    let detail = started
                        .as_ref()
                        .map(|s| s.detail.clone())
                        .unwrap_or_default();
                    p.last = Some(Done {
                        ms: match (started.as_ref().and_then(|s| s.at), at) {
                            (Some(a), Some(b)) if b >= a => Some(b - a),
                            _ => None,
                        },
                        tool: tool.clone(),
                        detail,
                        ok,
                        at,
                    });
                    p.steps += 1;
                    if !ok {
                        stream_fails += 1;
                        // The plugin reported this same failure a moment
                        // ago, with its text: keep that spelling.
                        let keep = p.failure.as_ref().is_some_and(|x| {
                            x.tool == tool
                                && !x.text.is_empty()
                                && matches!((x.at, at), (Some(a), Some(b)) if (b - a).abs() < 5_000)
                        });
                        if !keep {
                            p.failure = Some(Failure {
                                tool,
                                text: String::new(),
                                at,
                            });
                        }
                    }
                }
                _ => {}
            },
            "toolfull" => {
                let detail = detail_of(str_of(&f, "p"));
                match status {
                    "running" => match stack.iter_mut().find(|s| s.tool == tool && !s.plugin) {
                        // The stream already opened this call: the plugin's
                        // args are the fuller spelling of its argument.
                        Some(open) => {
                            open.plugin = true;
                            if detail.chars().count() > open.detail.chars().count() {
                                open.detail = detail;
                            }
                        }
                        None => stack.push(Step {
                            tool,
                            detail,
                            at,
                            stream_id: String::new(),
                            plugin: true,
                        }),
                    },
                    "completed" | "error" => {
                        // A call the stream never reported (a plugin-only
                        // tail) ends here; one it did ends on its own frame.
                        if let Some(i) = stack
                            .iter()
                            .rposition(|s| s.plugin && s.tool == tool && s.stream_id.is_empty())
                        {
                            let s = stack.remove(i);
                            p.steps += 1;
                            p.last = Some(Done {
                                ms: match (s.at, at) {
                                    (Some(a), Some(b)) if b >= a => Some(b - a),
                                    _ => None,
                                },
                                tool: tool.clone(),
                                detail: s.detail,
                                ok: status == "completed",
                                at,
                            });
                        }
                        let out = f.get("r").and_then(output_of);
                        if status == "error" {
                            plugin_fails += 1;
                            p.failure = Some(Failure {
                                tool,
                                text: out.clone().unwrap_or_default(),
                                at,
                            });
                        }
                        if out.is_some() {
                            p.output = out;
                        }
                    }
                    _ => {}
                }
            }
            // The workbench tools' own frame: Talaria ran it, so it is a
            // finished step with its result.
            "wtool" => {
                let out = f.get("r").and_then(output_of);
                // Frames written before `ok` existed read as successes.
                let ok = f.get("ok").and_then(Value::as_bool).unwrap_or(true);
                p.last = Some(Done {
                    tool: tool.clone(),
                    detail: detail_of(str_of(&f, "p")),
                    ok,
                    at,
                    ms: f.get("ms").and_then(Value::as_i64),
                });
                if !ok {
                    stream_fails += 1;
                    p.failure = Some(Failure {
                        tool,
                        text: out.clone().unwrap_or_default(),
                        at,
                    });
                }
                if out.is_some() {
                    p.output = out;
                }
            }
            "r" => {
                let v = str_of(&f, "v").trim();
                if !v.is_empty() {
                    p.said = Some(tail_chars(v, 240));
                }
            }
            "err" => {
                stream_fails += 1;
                p.failure = Some(Failure {
                    tool: "turn".into(),
                    text: tail_chars(str_of(&f, "v"), 240),
                    at,
                });
            }
            _ => {}
        }
    }
    p.failures = stream_fails.max(plugin_fails);
    p.step = stack.last().cloned();
    p.outer = (stack.len() > 1).then(|| stack[0].clone());
    p
}

/// The pulse as the API hands it out. Times are epoch ms; the reader works
/// out "for how long" against its own clock, so a cached answer still ticks.
pub fn wire(p: &Pulse, turn: Option<(i64, i64)>) -> Value {
    let step = |s: &Step| json!({ "tool": s.tool, "detail": s.detail, "since": s.at });
    json!({
        "turn": turn.map(|t| t.0),
        "turnAt": turn.map(|t| t.1),
        "lastAt": p.last_at,
        "step": p.step.as_ref().map(step),
        "outer": p.outer.as_ref().map(step),
        "last": p.last.as_ref().map(|d| json!({
            "tool": d.tool, "detail": d.detail, "ok": d.ok, "at": d.at, "ms": d.ms,
        })),
        "output": p.output,
        "said": p.said,
        "steps": p.steps,
        "failures": p.failures,
        "failure": p.failure.as_ref().map(|f| json!({ "tool": f.tool, "text": f.text, "at": f.at })),
        "partial": p.partial,
    })
}

/// `<turn>:<ms>` → (turn, ms).
pub fn parse_turn(v: &str) -> Option<(i64, i64)> {
    let (t, at) = v.split_once(':')?;
    Some((t.trim().parse().ok()?, at.trim().parse().ok()?))
}

/// The turn begins: its tail starts empty and its age starts now. Called by
/// the turn itself, before the request opens, so a reader never pairs a new
/// turn's number with the last turn's frames.
pub async fn begin_turn(
    conn: &mut redis::aio::ConnectionManager,
    run_id: &str,
    turn: i64,
    at_ms: i64,
) -> redis::RedisResult<()> {
    redis::pipe()
        .cmd("SET")
        .arg(tail_key(run_id))
        .arg("")
        .arg("EX")
        .arg(TAIL_TTL_S)
        .ignore()
        .cmd("SET")
        .arg(turn_key(run_id))
        .arg(format!("{turn}:{at_ms}"))
        .arg("EX")
        .arg(TAIL_TTL_S)
        .ignore()
        .query_async(conn)
        .await
}

/// Keep a live turn's tail AND its turn marker alive. Every writer calls
/// this beside its APPEND: the tail's expiry used to be refreshed by some
/// writers only, and the marker never, so a turn longer than the TTL lost
/// its clock while its frames still landed.
pub async fn touch(
    conn: &mut redis::aio::ConnectionManager,
    run_id: &str,
) -> redis::RedisResult<()> {
    redis::pipe()
        .cmd("EXPIRE")
        .arg(tail_key(run_id))
        .arg(TAIL_TTL_S)
        .ignore()
        .cmd("EXPIRE")
        .arg(turn_key(run_id))
        .arg(TAIL_TTL_S)
        .ignore()
        .query_async(conn)
        .await
}

/// One run's last read: the tail's length and turn marker when it was read,
/// and what it said.
struct Cached {
    len: i64,
    turn: Option<String>,
    pulse: Pulse,
    read_at: std::time::Instant,
    used_at: std::time::Instant,
}

static CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Cached>>> =
    std::sync::LazyLock::new(Default::default);

/// A pulse read this recently is served again even if frames have landed
/// since: every viewer of a board polls every 5 s, and an active turn's tail
/// grows several times a second, so without this each viewer re-reads and
/// re-parses every live tail on every poll.
const FRESH: std::time::Duration = std::time::Duration::from_secs(2);
/// An entry nobody has asked for in this long is dropped.
const KEEP: std::time::Duration = std::time::Duration::from_secs(600);
/// Past this many entries the cache starts over (48 live sessions is a busy
/// instance; this is a bound, not a size anyone tunes).
const CACHE_CAP: usize = 4_096;

/// Whether a cached read still answers for a tail that is now `len` bytes
/// under turn marker `turn`: the same turn, and either the same length (an
/// APPEND-only tail that has not grown says the same thing) or a read young
/// enough to serve again. A new turn always reads: it resets the tail, and
/// its marker differs. Pure.
pub fn reuse(
    cached_len: i64,
    cached_turn: Option<&str>,
    age: std::time::Duration,
    len: i64,
    turn: Option<&str>,
) -> bool {
    cached_turn == turn && (cached_len == len || age < FRESH)
}

/// Read the pulse of each run. Two round trips at most: the tails' lengths
/// and turn markers, then the tails that changed since they were last read
/// (each bounded to its newest [`READ_BYTES`]). A run with no tail reads as an
/// empty pulse; a Redis failure is the caller's to show as "unknown".
pub async fn read_many(
    conn: &mut redis::aio::ConnectionManager,
    run_ids: &[String],
) -> redis::RedisResult<Vec<(Pulse, Option<(i64, i64)>)>> {
    if run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut pipe = redis::pipe();
    for id in run_ids {
        pipe.cmd("STRLEN").arg(tail_key(id));
        pipe.cmd("GET").arg(turn_key(id));
    }
    let raw: Vec<redis::Value> = pipe.query_async(conn).await?;
    let mut heads: Vec<(i64, Option<String>)> = Vec::with_capacity(run_ids.len());
    let mut it = raw.into_iter();
    while let (Some(len), Some(turn)) = (it.next(), it.next()) {
        heads.push((
            redis::from_redis_value(len).unwrap_or(0),
            redis::from_redis_value(turn).unwrap_or(None),
        ));
    }
    let now = std::time::Instant::now();
    let mut pulses: Vec<Option<Pulse>> = {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        run_ids
            .iter()
            .zip(&heads)
            .map(|(id, (len, turn))| {
                let hit = cache.get_mut(id).filter(|c| {
                    reuse(
                        c.len,
                        c.turn.as_deref(),
                        now - c.read_at,
                        *len,
                        turn.as_deref(),
                    )
                })?;
                hit.used_at = now;
                Some(hit.pulse.clone())
            })
            .collect()
    };
    let missing: Vec<usize> = (0..run_ids.len())
        .filter(|&i| pulses[i].is_none())
        .collect();
    if !missing.is_empty() {
        let mut pipe = redis::pipe();
        for &i in &missing {
            pipe.cmd("GETRANGE")
                .arg(tail_key(&run_ids[i]))
                .arg(-READ_BYTES)
                .arg(-1);
        }
        let texts: Vec<String> = pipe.query_async(conn).await?;
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > CACHE_CAP {
            cache.clear();
        }
        cache.retain(|_, c| now.duration_since(c.used_at) < KEEP);
        for (&i, text) in missing.iter().zip(texts) {
            let (len, turn) = &heads[i];
            let pulse = read_tail(&text, *len > READ_BYTES as i64);
            cache.insert(
                run_ids[i].clone(),
                Cached {
                    len: *len,
                    turn: turn.clone(),
                    pulse: pulse.clone(),
                    read_at: now,
                    used_at: now,
                },
            );
            pulses[i] = Some(pulse);
        }
    }
    Ok(pulses
        .into_iter()
        .zip(heads)
        .map(|(p, (_, turn))| (p.unwrap_or_default(), turn.as_deref().and_then(parse_turn)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_adds_at_to_an_object_and_leaves_anything_else_alone() {
        assert_eq!(
            stamp(r#"{"t":"d","v":"x"}"#, 7),
            r#"{"t":"d","v":"x","at":7}"#
        );
        assert_eq!(stamp("{}", 7), r#"{"at":7}"#);
        assert_eq!(stamp(r#"{"t":"d","at":1}"#, 7), r#"{"t":"d","at":1}"#);
        assert_eq!(stamp("not json", 7), "not json");
        // A trailing newline is the writer's; the stamp keeps the object whole.
        assert_eq!(stamp("{\"t\":\"r\"}\n", 9), r#"{"t":"r","at":9}"#);
        // A command that merely mentions `"at":` is still a frame without one.
        let echo = json!({"t": "tool", "v": "terminal", "p": "echo '{\"at\":1}'"}).to_string();
        let stamped: Value = serde_json::from_str(&stamp(&echo, 5)).unwrap();
        assert_eq!(stamped["at"], 5);
        assert_eq!(stamped["p"], "echo '{\"at\":1}'");
    }

    #[test]
    fn a_cached_read_answers_only_for_the_same_turn() {
        use std::time::Duration;
        let old = Duration::from_secs(30);
        // Same turn, nothing appended: the read stands however old it is.
        assert!(reuse(500, Some("2:9"), old, 500, Some("2:9")));
        // Frames landed: an old read is read again, a fresh one is served.
        assert!(!reuse(500, Some("2:9"), old, 900, Some("2:9")));
        assert!(reuse(
            500,
            Some("2:9"),
            Duration::from_millis(500),
            900,
            Some("2:9")
        ));
        // A new turn always reads, even at the same length and within FRESH.
        assert!(!reuse(500, Some("2:9"), Duration::ZERO, 500, Some("3:12")));
        assert!(!reuse(0, None, Duration::ZERO, 0, Some("1:1")));
    }

    #[test]
    fn parallel_calls_keep_the_one_still_running() {
        let t = tail(&[
            json!({"t":"tool","v":"terminal","s":"running","id":"m:terminal#1","p":"cargo build","at":10}),
            json!({"t":"tool","v":"terminal","s":"running","id":"m:terminal#2","p":"cargo test","at":11}),
            // The first one finishes; the second is still running.
            json!({"t":"tool","v":"terminal","s":"completed","id":"m:terminal#1","at":50}),
        ]);
        let p = read_tail(&t, false);
        assert_eq!(p.step.unwrap().detail, "cargo test");
        assert_eq!(p.last.unwrap().detail, "cargo build");
        // Plugin frames for two parallel calls of one tool pair with the
        // stream's in order.
        let t = tail(&[
            json!({"t":"toolfull","v":"terminal","s":"running","id":"c1","p":"{\"command\":\"cargo build --release\"}","at":10}),
            json!({"t":"toolfull","v":"terminal","s":"running","id":"c2","p":"{\"command\":\"cargo test --workspace\"}","at":10}),
            json!({"t":"tool","v":"terminal","s":"running","id":"m:t#1","p":"cargo build","at":11}),
            json!({"t":"tool","v":"terminal","s":"running","id":"m:t#2","p":"cargo test","at":11}),
            json!({"t":"tool","v":"terminal","s":"completed","id":"m:t#1","at":40}),
        ]);
        let p = read_tail(&t, false);
        assert_eq!(p.step.unwrap().detail, "cargo test --workspace");
        assert_eq!(p.last.unwrap().detail, "cargo build --release");
    }

    fn tail(frames: &[Value]) -> String {
        frames.iter().map(|f| format!("{f}\n")).collect()
    }

    /// The outcrop shape: an execute_code cell that runs a terminal command,
    /// then a foreground build the tool ceiling killed.
    #[test]
    fn the_innermost_running_step_is_the_step_and_a_killed_build_is_the_failure() {
        let t = tail(&[
            json!({"t":"r","v":"Checking the warm target.","at":1_000}),
            json!({"t":"tool","v":"execute_code","s":"running","id":"m:execute_code#1","p":"from hermes_tools import terminal","at":1_100}),
            json!({"t":"tool","v":"terminal","s":"running","id":"m:terminal#1","p":"ps aux | grep cargo","at":1_200}),
            json!({"t":"toolfull","v":"terminal","s":"running","id":"call_1","p":"{\"command\": \"ps aux | grep cargo | grep -v grep\"}","at":1_210}),
        ]);
        let p = read_tail(&t, false);
        let step = p.step.unwrap();
        assert_eq!(step.tool, "terminal");
        assert_eq!(
            step.detail, "ps aux | grep cargo | grep -v grep",
            "the plugin's args are the fuller spelling"
        );
        assert_eq!(step.at, Some(1_200));
        assert_eq!(p.outer.unwrap().tool, "execute_code");
        assert_eq!(p.said.as_deref(), Some("Checking the warm target."));
        assert_eq!(p.last_at, Some(1_210));

        let t2 = format!(
            "{t}{}",
            tail(&[
                json!({"t":"toolfull","v":"terminal","s":"completed","id":"call_1","r":"{\"output\": \"2083174 bun x omp\\n2089786 cargo test -p talaria-notify\", \"exit_code\": 0}","at":1_300}),
                json!({"t":"tool","v":"terminal","s":"completed","id":"m:terminal#1","at":1_301}),
                json!({"t":"tool","v":"execute_code","s":"completed","id":"m:execute_code#1","at":1_400}),
                json!({"t":"tool","v":"terminal","s":"running","id":"m:terminal#2","p":"cargo build -p talaria-api","at":2_000}),
                json!({"t":"toolfull","v":"terminal","s":"error","id":"call_2","r":"Error executing tool 'terminal': timed out after 420.0s","at":422_000}),
                json!({"t":"tool","v":"terminal","s":"error","id":"m:terminal#2","at":422_001}),
            ])
        );
        let p = read_tail(&t2, false);
        assert!(
            p.step.is_none(),
            "nothing is running after the killed build"
        );
        let last = p.last.unwrap();
        assert_eq!(
            (last.tool.as_str(), last.ok, last.ms),
            ("terminal", false, Some(420_001))
        );
        assert_eq!(last.detail, "cargo build -p talaria-api");
        assert_eq!(
            p.failures, 1,
            "one failure, reported by the stream and the plugin both"
        );
        assert!(p.failure.unwrap().text.contains("timed out after 420.0s"));
        assert_eq!(
            p.output.as_deref(),
            Some("Error executing tool 'terminal': timed out after 420.0s")
        );
        assert_eq!(p.steps, 3);
    }

    /// What Hermes v0.22.4 actually sends (the workobs proof stack): the
    /// plugin's frame for a call lands BEFORE the stream's, and the stream's
    /// preview is Hermes' display label. One call, one step, the full command.
    #[test]
    fn one_call_reported_plugin_first_is_one_step_with_the_full_command() {
        let cmd = "for i in $(seq 1 12); do echo \"omp step $i\"; sleep 5; done";
        let t = tail(&[
            json!({"t":"toolfull","v":"terminal","s":"running","id":"call_1","p": json!({"command": cmd, "timeout": 360}).to_string(),"at":100}),
            json!({"t":"tool","v":"terminal","s":"running","id":"m:terminal#1","p":"for i in $(seq 1 12) + 4 commands","at":101}),
        ]);
        let p = read_tail(&t, false);
        let step = p.step.unwrap();
        assert_eq!(step.detail, cmd);
        assert_eq!(step.at, Some(100));
        assert!(p.outer.is_none(), "one call is not its own parent");
        let t = format!(
            "{t}{}",
            tail(&[
                json!({"t":"toolfull","v":"terminal","s":"completed","id":"call_1","r": json!({"output": "omp step 12\ntests passed", "exit_code": 0}).to_string(),"at":60_100}),
                json!({"t":"tool","v":"terminal","s":"completed","id":"m:terminal#1","at":60_101}),
            ])
        );
        let p = read_tail(&t, false);
        assert!(p.step.is_none());
        assert_eq!(p.steps, 1);
        let last = p.last.unwrap();
        assert_eq!((last.ms, last.detail.as_str()), (Some(60_001), cmd));
        assert_eq!(p.output.as_deref(), Some("omp step 12 ⏎ tests passed"));
    }

    #[test]
    fn a_plugin_only_tail_still_has_a_step_and_an_output() {
        let t = tail(&[
            json!({"t":"toolfull","v":"terminal","s":"running","id":"c1","p":"{\"command\":\"omp --mode json -c\"}","at":10}),
        ]);
        assert_eq!(
            read_tail(&t, false).step.unwrap().detail,
            "omp --mode json -c"
        );
        let t = format!(
            "{t}{}",
            tail(&[
                json!({"t":"toolfull","v":"terminal","s":"completed","id":"c1","r":"{\"output\":\"a\\n\\nb\\nc\",\"exit_code\":0}","at":20})
            ])
        );
        let p = read_tail(&t, false);
        assert!(p.step.is_none());
        assert_eq!(p.output.as_deref(), Some("b ⏎ c"));
        assert_eq!(p.last.unwrap().ms, Some(10));
    }

    #[test]
    fn a_cut_window_drops_its_first_line_and_says_so() {
        let t = format!(
            "{{\"t\":\"tool\",\"v\":\"half\n{}",
            tail(&[json!({"t":"r","v":"hi","at":5})])
        );
        let p = read_tail(&t, true);
        assert!(p.partial);
        assert_eq!(p.said.as_deref(), Some("hi"));
        assert!(p.step.is_none());
    }

    #[test]
    fn an_unstamped_tail_reads_without_times() {
        let t = tail(&[json!({"t":"tool","v":"terminal","s":"running","id":"x","p":"ls"})]);
        let p = read_tail(&t, false);
        assert_eq!(p.step.as_ref().unwrap().at, None);
        assert_eq!(p.last_at, None);
        let w = wire(&p, parse_turn("3:99"));
        assert_eq!(w["turn"], 3);
        assert_eq!(w["turnAt"], 99);
        assert_eq!(w["step"]["tool"], "terminal");
    }

    #[test]
    fn a_workbench_frame_is_a_finished_step_with_its_result() {
        let t = tail(&[
            json!({"t":"wtool","v":"prepare_env","ms":2102,"p":"{\"jobId\":\"j\"}","r":"{\"ok\":true}","at":50}),
        ]);
        let p = read_tail(&t, false);
        let last = p.last.unwrap();
        assert_eq!(
            (last.tool.as_str(), last.ms, last.ok),
            ("prepare_env", Some(2102), true)
        );
        assert_eq!(p.output.as_deref(), Some("{\"ok\":true}"));
        assert_eq!(p.failures, 0);
        // A refused start_job is a failure the reader must see.
        let t = tail(&[
            json!({"t":"wtool","v":"start_job","ms":12,"ok":false,"p":"{}","r":"live-job cap reached","at":60}),
        ]);
        let p = read_tail(&t, false);
        assert_eq!(p.failures, 1);
        assert_eq!(p.failure.unwrap().text, "live-job cap reached");
    }
}
