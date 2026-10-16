"""Talaria tool-event and delegation reporting.

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

Deliberately defensive: a hook must never block or break the agent's tool
loop. Everything is best-effort — bounded queue (oldest events drop first
under pressure), one in-flight POST at a time, short timeouts, and any
failure just drops the event. The platform re-clamps and scrubs what it
stores; the plugin only clamps to keep the POST small.
"""

from __future__ import annotations

import json
import os
import queue
import threading
import time
import urllib.request
from typing import Any

_MAX_CHARS = 4000
_MAX_TOOLS = 40
_QUEUE_CAP = 512
_POST_TIMEOUT = 4.0
_TOOL_EVENTS_PATH = "/api/agents/tool-events"
_SUBAGENT_EVENTS_PATH = "/api/agents/subagent-events"

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
    """A full URL for one of the platform's event doors, for an event that
    does not go to the default one. Read from the environment here rather
    than passed in: the base is the container's own and fixed for its life."""
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
        # A per-event destination, defaulting to the tool-events door. The key
        # is POPPED rather than posted: a tool event carries none at all, so
        # its body stays byte-identical to the wire the platform parses today.
        url = event.pop("url", None) or url
        try:
            req = urllib.request.Request(
                url, data=json.dumps(event).encode("utf-8"), headers=headers, method="POST"
            )
            urllib.request.urlopen(req, timeout=_POST_TIMEOUT).close()
        except Exception:
            # Fire-and-forget by contract; a dropped event costs one line of
            # visibility, never agent work.
            pass


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
    # and the chip's whole point is that the stop replaces the start.
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


def register(ctx) -> None:
    """The loader's entry point — Hermes refuses a plugin without it
    ("Plugin 'talaria-events' has no register() function", the line that
    spent a deploy silently tool-event-dark). The hook names are the
    loader's own spelling, on_pre_tool_call / on_post_tool_call — the
    functions above; the yaml's pre/post section is documentation, not
    the registration. The delegation pair is registered the same way:
    subagent_start / subagent_stop are the names in hermes_cli's VALID_HOOKS,
    fired by the delegate_task tool around every child it spawns."""
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("post_tool_call", on_post_tool_call)
    ctx.register_hook("subagent_start", on_subagent_start)
    ctx.register_hook("subagent_stop", on_subagent_stop)
