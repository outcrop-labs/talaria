"""Talaria tool-event and screenshot reporting.

Every tool call's lifecycle (name, arguments, RESULT) is visible here and
nowhere else on the platform's wire — the chat-completions SSE the work
session consumes reports tool NAMES and previews only. This plugin bridges
that: pre/post_tool_call enqueue one small JSON record each, and a daemon
thread POSTs them to the Talaria api, which appends them to the live run's
watch stream and transcript.

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
import queue
import threading
import time
import urllib.request
from typing import Any

_MAX_CHARS = 4000
_QUEUE_CAP = 512
_POST_TIMEOUT = 4.0
_TOOL_EVENTS_PATH = "/api/agents/tool-events"
_UPLOADS_PATH = "/api/agents/uploads"
# The tool that hands back a screenshot, and the ceiling its door enforces.
# Reading the file is bounded by the same number so an oversized image is
# dropped here rather than posted to be refused.
_VISION_TOOL = "browser_vision"
_MAX_SHOT_BYTES = 5 * 1024 * 1024

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


def _screenshot_event(result: Any) -> dict | None:
    """The uploads POST for a screenshot tool's PNG, or None. The bytes have
    to travel because the file does not: it lives in this container, and the
    container is thrown away on the next roll. Bounded by the door's own
    ceiling, and every failure to read is a dropped image rather than a raised
    exception inside the agent's tool loop."""
    path = _shot_path(result)
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
    if status == "completed" and tool_name == _VISION_TOOL:
        shot = _screenshot_event(result)
        if shot is not None:
            _enqueue(shot)


def register(ctx) -> None:
    """The loader's entry point — Hermes refuses a plugin without it
    ("Plugin 'talaria-events' has no register() function", the line that
    spent a deploy silently tool-event-dark). The hook names are the
    loader's own spelling, on_pre_tool_call / on_post_tool_call — the
    functions above; the yaml's pre/post section is documentation, not
    the registration."""
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("post_tool_call", on_post_tool_call)
