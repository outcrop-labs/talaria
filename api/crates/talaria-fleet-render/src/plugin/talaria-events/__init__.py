"""Talaria tool-event, delegation, cron-output and screenshot reporting.

Every tool call's lifecycle (name, arguments, RESULT) is visible here and
nowhere else on the platform's wire — the chat-completions SSE the work
session consumes reports tool NAMES and previews only. This plugin bridges
that: pre/post_tool_call enqueue one small JSON record each, and a daemon
thread POSTs them to the Talaria api, which appends them to the live run's
watch stream and transcript.

Delegation is the same gap, wider. subagent_start/subagent_stop are the only
place a parent handing work to a child is observable at all: the child runs
under its own session id, so its tool events carry a correlation the parent's
reader cannot join, and the parent's stream shows a minutes-long silence that
reads as a hang. Those two hooks go to their own door, which lands one chip
per child on the turn that delegated.

It carries a second report on the same wire: a CRON turn's final text.
A cron fires the agent's own loop from inside the container, so its answer
reaches no Talaria surface at all — `api_server` is a non-push origin (the
gateway has nowhere to deliver to) and `on_session_end` carries no text,
only the session's end. `post_llm_call` is the one hook that holds both
halves of what the platform needs: `assistant_response`, the turn's final
text, and the cron task id (`cron:<job>:<execution>`) that names which
schedule — and so which conversation — this output belongs to.

Metering needs one fact only Hermes holds. The gateway relay books every LLM
call this agent makes, and Hermes names the conversation on each one in the
`x-talaria-session` header (the rendered `providers.talaria` entry's
`session_affinity_header`). On a turn Talaria drives, that name is Hermes'
own digest of the session's route (`gwk_...`), which the platform cannot
compute. `pre_llm_call` runs inside the turn, where the same function that
builds the header answers, so it reports the key with the conversation's own
id — at every turn until the platform has confirmed it, then no more — and
the relay can then book the turn's calls, auxiliary and subagent calls
included, to the flow that drove it.

A screenshot is the same gap with bytes in it. `browser_vision` writes a PNG
inside this container and reports its PATH — a path that means nothing to the
platform and is gone with the next roll. When that tool answers with one, the
bytes themselves ride to the uploads door, which stamps them on the turn the
agent is answering so the person watching sees the page instead of a filename.

Deliberately defensive: a hook must never block or break the agent's tool
loop. Everything is best-effort — bounded queue (oldest events drop first
under pressure), one in-flight POST at a time, short timeouts, and any
failure just drops the event. The platform re-clamps and scrubs what it
stores; the plugin only clamps to keep the POST small.
"""

from __future__ import annotations

import base64
import json
import os
import re
import stat
import queue
import threading
import time
import urllib.request
from typing import Any

_MAX_CHARS = 4000
_MAX_TOOLS = 40
# A cron turn is prose, not a tool argument: the api stores it as a message
# and clamps it to the same figure at the boundary.
_MAX_TEXT = 20000
_QUEUE_CAP = 512
_POST_TIMEOUT = 4.0
# The platform's three doors. Tool events are the default; the other two are
# named per event (see `_poster`).
_TOOL_EVENTS_PATH = "/api/agents/tool-events"
_SUBAGENT_EVENTS_PATH = "/api/agents/subagent-events"
_CRON_OUTPUT_PATH = "/api/agents/cron-output"
_AFFINITY_PATH = "/api/agents/affinity"
_UPLOADS_PATH = "/api/agents/uploads"
# The tool that hands back a screenshot, and the ceiling its door enforces.
# Reading the file is bounded by the same number so an oversized image is
# dropped here rather than posted to be refused.
_VISION_TOOL = "browser_vision"
_MAX_SHOT_BYTES = 5 * 1024 * 1024
# Any other browser tool (browser_exec above all — the browser-use harness the
# skill steers the model toward saves screenshots to a file and PRINTS the
# path) hands over an image only as a fresh PNG it names: on disk, a regular
# file, written within this window. A stale path mentioned in prose is not
# an offer of an image.
_FRESH_SHOT_SECONDS = 180
_recent_shots: dict[str, float] = {}

# (conversation, key) pairs the platform has CONFIRMED: one delivered report
# per conversation per container life. A pair joins this set only when its POST
# answered 2xx (see `_poster`), never when it is queued — a report lost to a
# timeout, an api restart or the queue dropping its oldest event under a burst
# is simply sent again at the next turn of that conversation, and the door's
# backfill then claims the calls booked before it landed. Bounded — a
# long-lived agent forgets the oldest and at worst reports a pair twice,
# which the door takes as a no-op.
_reported_affinity: set[tuple[str, str]] = set()
_AFFINITY_SEEN_CAP = 4096

_queue: "queue.Queue[dict]" = queue.Queue(maxsize=_QUEUE_CAP)
_started = False
_start_lock = threading.Lock()


def _config() -> tuple[str, dict[str, str]] | None:
    base = os.environ.get("TALARIA_API_URL", "").rstrip("/")
    key = os.environ.get("TALARIA_AGENT_KEY", "")
    name = os.environ.get("API_SERVER_MODEL_NAME", "")
    if not base or not key or not name:
        return None
    return base + _TOOL_EVENTS_PATH, {
        "Content-Type": "application/json",
        "X-Agent-Name": name,
        "X-Api-Key": key,
    }


def _door(path: str) -> str:
    """A full URL for one of the platform's doors, for an event that does not
    go to the default one. Read from the environment here rather than passed
    in: the base is the container's own and fixed for its life."""
    return os.environ.get("TALARIA_API_URL", "").rstrip("/") + path


def _clamp(value: Any) -> str:
    try:
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, default=str)
    except Exception:
        text = str(value)
    return text[:_MAX_CHARS]


def _clip(value: Any, limit: int) -> str:
    """A field bounded to what its door stores. The plugin clamps only to
    keep the POST small — the platform re-clamps at the boundary and is the
    authority on what is kept."""
    text = value if isinstance(value, str) else _clamp(value)
    return text[:limit]


def _enqueue(event: dict) -> None:
    global _started
    if not _started:
        with _start_lock:
            if not _started:
                threading.Thread(target=_poster, name="talaria-events", daemon=True).start()
                _started = True
    try:
        _queue.put_nowait(event)
    except queue.Full:
        # Observability yields to the agent: drop the oldest and retry once.
        try:
            _queue.get_nowait()
            _queue.put_nowait(event)
        except Exception:
            pass


def _poster() -> None:
    while True:
        event = _queue.get()
        cfg = _config()
        if cfg is None:
            continue
        url, headers = cfg
        # Three doors, one queue: a per-event destination, defaulting to the
        # tool-events door. The key is POPPED rather than posted: a tool event
        # carries none at all, so its body stays byte-identical to the wire
        # the platform parses today, and a cron turn's or a screenshot's body
        # reaches its own door with nothing extra in it.
        url = event.pop("url", None) or url
        # An affinity report names the pair it confirms, popped like the url
        # so the body stays the door's own shape.
        confirms = event.pop("confirms", None)
        try:
            req = urllib.request.Request(
                url, data=json.dumps(event).encode("utf-8"), headers=headers, method="POST"
            )
            urllib.request.urlopen(req, timeout=_POST_TIMEOUT).close()
        except Exception:
            # Fire-and-forget by contract; a dropped event costs one line of
            # visibility, never agent work. An unconfirmed affinity pair is
            # reported again at its conversation's next turn.
            continue
        if confirms is not None:
            _confirm_affinity(confirms)


def _confirm_affinity(pair: tuple[str, str]) -> None:
    """The platform has the pair: stop reporting it for this container's
    life. urlopen raises on anything but a 2xx, so only a stored report gets
    here."""
    if len(_reported_affinity) >= _AFFINITY_SEEN_CAP:
        _reported_affinity.clear()
    _reported_affinity.add(pair)


def on_pre_tool_call(
    *,
    tool_name: str = "",
    args: Any = None,
    session_id: str = "",
    tool_call_id: str = "",
    turn_id: str = "",
    **_: Any,
) -> None:
    if not tool_name:
        return
    _enqueue(
        {
            "sessionId": session_id,
            "toolName": tool_name,
            "toolCallId": tool_call_id,
            "status": "running",
            "args": _clamp(args),
            "turnId": turn_id,
            "at": int(time.time() * 1000),
        }
    )


def _is_error_result(result: Any) -> bool:
    """The persona's tool-error convention: a JSON object whose top level
    says so — `"success": false` or a non-empty `"error"`. Everything else
    (plain text, data payloads, empty) is a success-shaped result; a tool
    that legitimately returns such an object AS data is the documented
    trade-off for having a failure signal at all."""
    try:
        parsed = json.loads(result) if isinstance(result, str) else result
    except (ValueError, TypeError):
        return False
    if not isinstance(parsed, dict):
        return False
    if parsed.get("success") is False:
        return True
    err = parsed.get("error")
    return isinstance(err, str) and bool(err.strip())


def _shot_path(result: Any) -> str | None:
    """The PNG a screenshot tool wrote, from the two shapes the persona uses:
    a JSON result whose `data.path` names the file, or a `MEDIA:<path>` token
    in text. Anything else is not an offer of an image — a tool that merely
    mentions a .png in prose is reporting, not handing one over."""
    parsed: Any = result
    if isinstance(result, str):
        try:
            parsed = json.loads(result)
        except (ValueError, TypeError):
            parsed = None
    if isinstance(parsed, dict):
        data = parsed.get("data")
        if isinstance(data, dict):
            path = data.get("path")
            if isinstance(path, str) and path.lower().endswith(".png"):
                return path
    text = result if isinstance(result, str) else ""
    marker = "MEDIA:"
    at = text.find(marker)
    if at != -1:
        rest = text[at + len(marker) :].split(maxsplit=1)
        token = rest[0].strip("\"'") if rest else ""
        if token.lower().endswith(".png"):
            return token
    return None


_PNG_TOKEN = re.compile(r"(?<![\w/])(/[^\s\"'`<>|]+\.png)\b", re.IGNORECASE)


def _fresh_shot_path(tool_name: str, args: Any, result: Any) -> str | None:
    """For a browser tool other than browser_vision: the newest PNG the call
    names (in its result or its arguments) that exists, is a regular file,
    fits the door's ceiling and was written inside the freshness window —
    and that this process has not already forwarded at that mtime. Absolute
    paths only: the harness prints them that way, and a relative token has
    no cwd this hook could trust."""
    if not tool_name.startswith("browser_"):
        return None
    text = " ".join(
        part if isinstance(part, str) else json.dumps(part, default=str)
        for part in (result, args)
        if part is not None
    )
    now = time.time()
    best: tuple[float, str] | None = None
    for token in set(_PNG_TOKEN.findall(text)):
        try:
            st = os.stat(token)
        except OSError:
            continue
        if not stat.S_ISREG(st.st_mode) or st.st_size == 0 or st.st_size > _MAX_SHOT_BYTES:
            continue
        if now - st.st_mtime > _FRESH_SHOT_SECONDS:
            continue
        if _recent_shots.get(token) == st.st_mtime:
            continue
        if best is None or st.st_mtime > best[0]:
            best = (st.st_mtime, token)
    if best is None:
        return None
    # Forget shots older than the window so the map cannot grow for the life
    # of the container.
    for key in [k for k, m in _recent_shots.items() if now - m > _FRESH_SHOT_SECONDS]:
        _recent_shots.pop(key, None)
    _recent_shots[best[1]] = best[0]
    return best[1]


def _screenshot_event(result: Any, path: str | None = None, session_id: str = "") -> dict | None:
    """The uploads POST for a screenshot tool's PNG, or None. The bytes have
    to travel because the file does not: it lives in this container, and the
    container is thrown away on the next roll. Bounded by the door's own
    ceiling, and every failure to read is a dropped image rather than a raised
    exception inside the agent's tool loop. The session id rides with it: for
    a pinned agent it is the conversation the screenshot belongs to, and the
    door lands the image there rather than on whichever turn is newest."""
    path = path or _shot_path(result)
    if not path:
        return None
    try:
        with open(path, "rb") as handle:
            # One byte past the ceiling, so an oversized file is recognised as
            # oversized without reading all of it.
            raw = handle.read(_MAX_SHOT_BYTES + 1)
    except OSError:
        return None
    if not raw or len(raw) > _MAX_SHOT_BYTES:
        return None
    return {
        "url": _door(_UPLOADS_PATH),
        "sessionId": session_id,
        "filename": os.path.basename(path) or "screenshot.png",
        "mime": "image/png",
        "dataBase64": base64.b64encode(raw).decode("ascii"),
    }


def on_post_tool_call(
    *,
    tool_name: str = "",
    args: Any = None,
    result: Any = None,
    session_id: str = "",
    tool_call_id: str = "",
    turn_id: str = "",
    error: Any = None,
    **_: Any,
) -> None:
    if not tool_name:
        return
    # FAILURE IS A FIRST-CLASS STATUS. During the 2026-09-22 incident every
    # tool call failed for half an hour while every wire the platform could
    # see said "completed" — the persona's own convention ({"success":
    # false} / {"error": ...}) is the only failure signal that exists, so it
    # rides the status instead of being inferred downstream from a clamped
    # result string. `error` is accepted when the harness passes one.
    failed = error is not None and error != "" and error is not False
    status = "error" if (failed or _is_error_result(result)) else "completed"
    event = {
        "sessionId": session_id,
        "toolName": tool_name,
        "toolCallId": tool_call_id,
        "status": status,
        "args": _clamp(args),
        "turnId": turn_id,
        "at": int(time.time() * 1000),
    }
    if result is not None:
        event["result"] = _clamp(result)
    _enqueue(event)
    # The image rides AFTER its tool event, deliberately: the event is what
    # the run watch and the failure streak read, and several MB of base64
    # must never be the thing standing in front of it in the queue.
    if status == "completed" and tool_name.startswith("browser_"):
        if tool_name == _VISION_TOOL:
            shot = _screenshot_event(result, session_id=session_id)
        else:
            shot = _screenshot_event(
                result, _fresh_shot_path(tool_name, args, result), session_id=session_id
            )
        if shot is not None:
            _enqueue(shot)


def _tool_names(history: Any) -> list[str]:
    """The names of the tools a child used, in order. The stop hook is handed
    the whole call history — entries shaped differently across harness
    versions, and carrying arguments and results that have no business on this
    wire (the child's own pre/post hooks already reported them). Names only,
    bounded, and never a reason to raise."""
    names: list[str] = []
    if not isinstance(history, (list, tuple)):
        return names
    for entry in history:
        name = ""
        if isinstance(entry, str):
            name = entry
        elif isinstance(entry, dict):
            for key in ("tool_name", "toolName", "name", "tool"):
                value = entry.get(key)
                if isinstance(value, str) and value:
                    name = value
                    break
        if name:
            names.append(name[:120])
        if len(names) >= _MAX_TOOLS:
            break
    return names


def on_subagent_start(
    *,
    parent_session_id: str = "",
    parent_turn_id: str = "",
    parent_subagent_id: str = "",
    child_session_id: str = "",
    child_subagent_id: str = "",
    child_role: str = "",
    child_goal: str = "",
    **_: Any,
) -> None:
    # THE CHILD'S SESSION ID IS THE CHIP'S IDENTITY. The subagent ids are the
    # delegation tool's own handles and the stop hook does not carry one, so
    # the session id is the only key both halves of a child's life share —
    # and the chip's whole point is that the stop replaces the start. The
    # PARENT'S session id is the chip's address: for a pinned agent it is the
    # conversation the delegating turn belongs to, and the door lands the chip
    # there rather than on whichever of the agent's turns is newest.
    if not child_session_id:
        return
    _enqueue(
        {
            "kind": "start",
            "parentSessionId": parent_session_id,
            "parentTurnId": parent_turn_id,
            "childSessionId": child_session_id,
            "childRole": _clip(child_role, 400),
            "childGoal": _clip(child_goal, 2000),
            "url": _door(_SUBAGENT_EVENTS_PATH),
            "at": int(time.time() * 1000),
        }
    )


def on_subagent_stop(
    *,
    parent_session_id: str = "",
    child_session_id: str = "",
    child_role: str = "",
    child_summary: str = "",
    child_status: str = "",
    tool_call_history: Any = None,
    **_: Any,
) -> None:
    if not child_session_id:
        return
    # The status is reported as the harness words it; the platform maps it to
    # the chip's done|error. Mapping it here would bake one harness version's
    # vocabulary into every agent container that already has this file.
    _enqueue(
        {
            "kind": "stop",
            "parentSessionId": parent_session_id,
            "childSessionId": child_session_id,
            "childRole": _clip(child_role, 400),
            "childSummary": _clip(child_summary, 4000),
            "childStatus": _clip(child_status, 80),
            "tools": _tool_names(tool_call_history),
            "url": _door(_SUBAGENT_EVENTS_PATH),
            "at": int(time.time() * 1000),
        }
    )


def on_post_llm_call(
    *,
    session_id: str = "",
    task_id: str = "",
    turn_id: str = "",
    assistant_response: Any = None,
    platform: str = "",
    **_: Any,
) -> None:
    # CRON ONLY. Every chat and work-session turn already reaches the
    # platform through the gateway's own stream, so reporting them here
    # would double every reply; `platform` is the scheduler's own marker for
    # the turns no other wire carries.
    if platform != "cron":
        return
    if not isinstance(assistant_response, str) or not assistant_response.strip():
        return
    _enqueue(
        {
            "url": _door(_CRON_OUTPUT_PATH),
            "sessionId": session_id,
            "taskId": task_id,
            "turnId": turn_id,
            "text": assistant_response[:_MAX_TEXT],
        }
    )


def on_pre_llm_call(*, session_id: str = "", platform: str = "", **_: Any) -> None:
    """Report the affinity key this turn's LLM calls carry, with the id of the
    conversation it names. Read here because `pre_llm_call` runs inside the
    turn, after Hermes has bound the conversation's scope — the values below
    are exactly what `merge_session_affinity_headers` stamps on every main,
    auxiliary and subagent call of the turn.

    Returns None, always: a value returned from this hook is injected into
    the user's message as context."""
    if platform == "cron":
        return None
    try:
        from agent.opencode_affinity import resolve_affinity_key
        from agent.portal_tags import get_conversation_context

        key = str(resolve_affinity_key(session_id or None) or "")
        conversation = str(get_conversation_context() or session_id or "")
    except Exception:
        return None
    # A cron's key names its job and needs no conversation; a key that IS the
    # conversation's id already says everything.
    if not key or not conversation or key == conversation or key.startswith("cron_"):
        return None
    pair = (conversation, key)
    if pair in _reported_affinity:
        return None
    # Marked reported by the poster once the door answers 2xx, not here: a
    # report that never lands must be sent again.
    _enqueue(
        {
            "url": _door(_AFFINITY_PATH),
            "conversation": conversation,
            "key": key,
            "confirms": pair,
        }
    )
    return None


def register(ctx) -> None:
    """The loader's entry point — Hermes refuses a plugin without it
    ("Plugin 'talaria-events' has no register() function", the line that
    spent a deploy silently tool-event-dark). The hook names are the
    loader's own spelling, on_pre_tool_call / on_post_tool_call /
    on_post_llm_call — the functions above; the yaml's hooks section is
    documentation, not the registration. The delegation pair is registered
    the same way: subagent_start / subagent_stop are the names in
    hermes_cli's VALID_HOOKS, fired by the delegate_task tool around every
    child it spawns. `pre_llm_call` is the metering report above."""
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("post_tool_call", on_post_tool_call)
    ctx.register_hook("post_llm_call", on_post_llm_call)
    ctx.register_hook("subagent_start", on_subagent_start)
    ctx.register_hook("subagent_stop", on_subagent_stop)
    ctx.register_hook("pre_llm_call", on_pre_llm_call)
