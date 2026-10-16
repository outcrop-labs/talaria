"""Talaria tool-event reporting.

Every tool call's lifecycle (name, arguments, RESULT) is visible here and
nowhere else on the platform's wire — the chat-completions SSE the work
session consumes reports tool NAMES and previews only. This plugin bridges
that: pre/post_tool_call enqueue one small JSON record each, and a daemon
thread POSTs them to the Talaria api, which appends them to the live run's
watch stream and transcript.

It carries a second report on the same wire: a CRON turn's final text.
A cron fires the agent's own loop from inside the container, so its answer
reaches no Talaria surface at all — `api_server` is a non-push origin (the
gateway has nowhere to deliver to) and `on_session_end` carries no text,
only the session's end. `post_llm_call` is the one hook that holds both
halves of what the platform needs: `assistant_response`, the turn's final
text, and the cron task id (`cron:<job>:<execution>`) that names which
schedule — and so which conversation — this output belongs to.

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
# A cron turn is prose, not a tool argument: the api stores it as a message
# and clamps it to the same figure at the boundary.
_MAX_TEXT = 20000
_QUEUE_CAP = 512
_POST_TIMEOUT = 4.0

_TOOL_EVENTS_PATH = "/api/agents/tool-events"
_CRON_OUTPUT_PATH = "/api/agents/cron-output"

_queue: "queue.Queue[dict]" = queue.Queue(maxsize=_QUEUE_CAP)
_started = False
_start_lock = threading.Lock()


def _config() -> tuple[str, dict[str, str]] | None:
    base = os.environ.get("TALARIA_API_URL", "").rstrip("/")
    key = os.environ.get("TALARIA_AGENT_KEY", "")
    name = os.environ.get("API_SERVER_MODEL_NAME", "")
    if not base or not key or not name:
        return None
    return base, {
        "Content-Type": "application/json",
        "X-Agent-Name": name,
        "X-Api-Key": key,
    }


def _clamp(value: Any) -> str:
    try:
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, default=str)
    except Exception:
        text = str(value)
    return text[:_MAX_CHARS]


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
        base, headers = cfg
        # Two doors, one queue: the event names its own path and the key is
        # popped before the body is built, so nothing extra reaches the api.
        url = base + event.pop("url", _TOOL_EVENTS_PATH)
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
            "url": _CRON_OUTPUT_PATH,
            "sessionId": session_id,
            "taskId": task_id,
            "turnId": turn_id,
            "text": assistant_response[:_MAX_TEXT],
        }
    )


def register(ctx) -> None:
    """The loader's entry point — Hermes refuses a plugin without it
    ("Plugin 'talaria-events' has no register() function", the line that
    spent a deploy silently tool-event-dark). The hook names are the
    loader's own spelling, on_pre_tool_call / on_post_tool_call /
    on_post_llm_call — the functions above; the yaml's hooks section is
    documentation, not the registration."""
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("post_tool_call", on_post_tool_call)
    ctx.register_hook("post_llm_call", on_post_llm_call)
