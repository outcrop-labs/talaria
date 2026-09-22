"""Talaria tool-event reporting.

Every tool call's lifecycle (name, arguments, RESULT) is visible here and
nowhere else on the platform's wire — the chat-completions SSE the work
session consumes reports tool NAMES and previews only. This plugin bridges
that: pre/post_tool_call enqueue one small JSON record each, and a daemon
thread POSTs them to the Talaria api, which appends them to the live run's
watch stream and transcript.

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
_QUEUE_CAP = 512
_POST_TIMEOUT = 4.0

_queue: "queue.Queue[dict]" = queue.Queue(maxsize=_QUEUE_CAP)
_started = False
_start_lock = threading.Lock()


def _config() -> tuple[str, dict[str, str]] | None:
    base = os.environ.get("TALARIA_API_URL", "").rstrip("/")
    key = os.environ.get("TALARIA_AGENT_KEY", "")
    name = os.environ.get("API_SERVER_MODEL_NAME", "")
    if not base or not key or not name:
        return None
    return base + "/api/agents/tool-events", {
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
        url, headers = cfg
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
