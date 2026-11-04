"""The snapshot half of the talaria-events plugin, against a real git repo.

Run with the plugin's path: `python3 -I snapshot_test.py __init__.py`. The
fleet-render crate's tests run it the same way. Every case drives the hook
functions as Hermes would, with the poster replaced by a list, and reads the
result back through git.
"""

import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile

spec = importlib.util.spec_from_file_location("talaria_events", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

events = []
mod._enqueue = lambda e: events.append(e) if e.get("kind") == "snapshot" else None


def git(root, *a, **kw):
    return subprocess.run(["git", "-C", root, *a], capture_output=True, text=True, check=True, **kw).stdout.strip()


def snapshots():
    return [e for e in events if e.get("kind") == "snapshot"]


base = tempfile.mkdtemp(prefix="ckpt-selftest-")
jobs = os.path.join(base, "jobs")
mod._JOBS_ROOT = jobs
job_id = "2f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5b"
workdir = os.path.join(jobs, job_id)
os.makedirs(workdir)
git(workdir, "init", "-q")
git(workdir, "config", "user.email", "a@b")
git(workdir, "config", "user.name", "a")
open(os.path.join(workdir, "tracked.txt"), "w").write("base\n")
open(os.path.join(workdir, ".gitignore"), "w").write("node_modules\n")
git(workdir, "add", "-A")
git(workdir, "commit", "-qm", "base")
head = git(workdir, "rev-parse", "HEAD")
session = "7b1e4c1a-9f8d-8a1b-9c2d-3e4f5a6b7c8d"


def answer(obj):
    return json.dumps({"result": json.dumps(obj)})


def post(tool, args, result, call_id="c0"):
    mod.on_post_tool_call(tool_name=tool, args=args, result=result, session_id=session, tool_call_id=call_id)


def pre(history, sid=session, req="t1:api:2"):
    mod.on_pre_api_request(session_id=sid, turn_id="t1", api_request_id=req, conversation_history=history)


# A queued start_job binds nothing; job_status with the started job binds.
post("mcp__workbench__start_job", {"ticket": "T-1"}, answer({"jobId": job_id, "status": "queued"}))
assert session not in mod._checkouts, "queued start_job must not bind"
post("mcp__workbench__job_status", {"jobId": job_id}, answer({"jobs": [{"id": job_id, "status": "started", "workdir": workdir}]}))
assert mod._checkouts[session] == (job_id, workdir), mod._checkouts
# A structuredContent answer binds too, and a path with a dot segment is normalised.
mod._checkouts.pop(session)
post("mcp__wb__start_job", {}, json.dumps({"result": "ok", "structuredContent": {"jobId": job_id, "status": "started", "workdir": os.path.join(jobs, ".", job_id) + "/"}}))
assert mod._checkouts[session] == (job_id, workdir), mod._checkouts

# Only the job's own directory under the jobs root binds, and only from a workbench tool.
for tool, obj in [
    ("mcp__workbench__start_job", {"jobId": job_id, "status": "started", "workdir": os.path.join(jobs, job_id, "..", "..", "evil")}),
    ("mcp__workbench__start_job", {"jobId": job_id, "status": "started", "workdir": os.path.join(jobs, "3f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5c")}),
    ("mcp__workbench__start_job", {"jobId": "not-a-uuid", "status": "started", "workdir": os.path.join(jobs, "not-a-uuid")}),
    ("mcp__workbench__start_job", {"jobId": job_id, "status": "started", "workdir": "/etc"}),
    ("evil__start_job", {"jobId": job_id, "status": "started", "workdir": workdir}),
    ("terminal", {"jobId": job_id, "status": "started", "workdir": workdir}),
]:
    mod._checkouts.pop(session, None)
    post(tool, {}, answer(obj))
    assert session not in mod._checkouts, (tool, obj, mod._checkouts)
post("mcp__workbench__start_job", {}, answer({"jobId": job_id, "status": "started", "workdir": workdir}))
assert mod._checkouts[session] == (job_id, workdir)

# Nothing the plugin reads may raise into the agent's turn: a NUL in a path, and a
# history that is not a list.
post("mcp__workbench__start_job", {}, answer({"jobId": job_id, "status": "started", "workdir": workdir + "\0x"}))
assert mod._checkouts[session] == (job_id, workdir), "a NUL path binds nothing and raises nothing"
pre({"not": "a list"})
pre([{"role": "tool", "tool_call_id": object()}])
assert snapshots() == []

# The agent works: a tracked edit, an untracked file, an ignored file, a staged
# file, and the platform's own env file.
open(os.path.join(workdir, "tracked.txt"), "w").write("v1\n")
open(os.path.join(workdir, "untracked.txt"), "w").write("u1\n")
os.makedirs(os.path.join(workdir, "node_modules"))
open(os.path.join(workdir, "node_modules", "x"), "w").write("big\n")
os.makedirs(os.path.join(workdir, ".talaria"))
open(os.path.join(workdir, ".talaria", "env"), "w").write("SECRET=1\n")
open(os.path.join(workdir, "staged.txt"), "w").write("s\n")
git(workdir, "add", "staged.txt")
status_before = git(workdir, "status", "--porcelain")
log_before = git(workdir, "log", "--oneline")
index_before = hashlib.sha256(open(os.path.join(workdir, ".git", "index"), "rb").read()).hexdigest()

for cid, result in (("c1", '{"output":"y"}'), ("c2", '{"error":"boom"}'), ("c3", '{"output":"y"}')):
    post("terminal", {"command": "x"}, result, call_id=cid)
history = [
    {"role": "user", "content": "turn 1"},
    {"role": "assistant", "tool_calls": [{"id": "c1"}, {"id": "c2"}, {"id": "c3"}]},
    {"role": "tool", "tool_call_id": "c1", "tool_name": "terminal", "content": "a"},
    {"role": "tool", "tool_call_id": "c2", "tool_name": "terminal", "content": "b"},
    {"role": "tool", "tool_call_id": "c3", "tool_name": "write_file", "content": "c"},
]
# The first model call of a turn has no trailing batch.
pre(history[:1], req="t1:api:1")
assert snapshots() == []
# The boundary after the batch snapshots.
pre(history)
assert len(snapshots()) == 1, events
e = snapshots()[0]
assert e["jobId"] == job_id and "reason" not in e, e
assert [c["id"] for c in e["calls"]] == ["c1", "c2", "c3"], e["calls"]
assert e["calls"][1]["status"] == "error" and e["calls"][0]["status"] == "completed"
assert e["calls"][2]["name"] == "write_file"
tree, commit = e["tree"], e["commit"]
listed = git(workdir, "ls-tree", "-r", "--name-only", tree).split("\n")
assert "untracked.txt" in listed and "staged.txt" in listed and "tracked.txt" in listed, listed
assert not any(p.startswith("node_modules") for p in listed), listed
assert ".talaria/env" not in listed, "the platform's own secret file stays out of a snapshot"
assert git(workdir, "show", f"{tree}:tracked.txt") == "v1"
assert git(workdir, "rev-parse", f"refs/talaria/ckpt/{session}/c3") == commit
assert git(workdir, "rev-parse", f"{commit}^") == head, "the snapshot's parent is the agent's HEAD"
# Invisible to the agent.
assert git(workdir, "status", "--porcelain") == status_before
assert git(workdir, "log", "--oneline") == log_before
assert hashlib.sha256(open(os.path.join(workdir, ".git", "index"), "rb").read()).hexdigest() == index_before
assert git(workdir, "rev-parse", "HEAD") == head
# A retried model call on the same history reports nothing new.
pre(history)
assert len(snapshots()) == 1
# An unchanged tree points the row at the commit it already has and writes no new ref.
refs_before = git(workdir, "for-each-ref", "refs/talaria", "--format=%(refname)").split("\n")
history2 = history + [{"role": "assistant", "tool_calls": [{"id": "c4"}]}, {"role": "tool", "tool_call_id": "c4", "tool_name": "terminal", "content": "d"}]
pre(history2, req="t1:api:3")
e2 = snapshots()[1]
assert e2["commit"] == commit and e2["tree"] == tree and e2["calls"] == [{"id": "c4", "name": "terminal", "status": "completed"}], e2
assert git(workdir, "for-each-ref", "refs/talaria", "--format=%(refname)").split("\n") == refs_before, "an unchanged tree writes no ref"
# A failing snapshot reports its reason and raises nothing.
os.chmod(os.path.join(workdir, ".git", "objects"), 0o500)
open(os.path.join(workdir, "tracked.txt"), "w").write("v2\n")
try:
    history3 = history2 + [{"role": "assistant", "tool_calls": [{"id": "c5"}]}, {"role": "tool", "tool_call_id": "c5", "tool_name": "terminal", "content": "e"}]
    pre(history3, req="t1:api:4")
finally:
    os.chmod(os.path.join(workdir, ".git", "objects"), 0o755)
e3 = snapshots()[2]
assert e3["reason"].startswith("snapshot failed: RuntimeError: git "), e3
assert "commit" not in e3 and e3["jobId"] == job_id
assert not [f for f in os.listdir(tempfile.gettempdir()) if f.startswith("talaria-ckpt-")], "temp index left behind"
# A git that hangs leaves neither the temp index nor its lock behind.
real_run = mod.subprocess.run


def hanging(cmd, *a, **kw):
    if "add" in cmd:
        open(kw["env"]["GIT_INDEX_FILE"] + ".lock", "w").close()
        raise subprocess.TimeoutExpired(cmd, 1)
    return real_run(cmd, *a, **kw)


mod.subprocess.run = hanging
try:
    history4 = history3 + [{"role": "assistant", "tool_calls": [{"id": "c6"}]}, {"role": "tool", "tool_call_id": "c6", "tool_name": "terminal", "content": "f"}]
    pre(history4, req="t1:api:5")
finally:
    mod.subprocess.run = real_run
e4 = snapshots()[3]
assert e4["reason"].startswith("snapshot failed: TimeoutExpired"), e4
assert not [f for f in os.listdir(tempfile.gettempdir()) if f.startswith("talaria-ckpt-")], "temp index or its lock left behind"
# A checkout with no index yet, as a clone has before its first add, still snapshots.
os.unlink(os.path.join(workdir, ".git", "index"))
history5 = history4 + [{"role": "assistant", "tool_calls": [{"id": "c7"}]}, {"role": "tool", "tool_call_id": "c7", "tool_name": "terminal", "content": "g"}]
pre(history5, req="t1:api:6")
e5 = snapshots()[4]
assert "commit" in e5 and "reason" not in e5, e5
assert "tracked.txt" in git(workdir, "ls-tree", "-r", "--name-only", e5["tree"])
assert not os.path.exists(os.path.join(workdir, ".git", "index")), "the agent's missing index is not created"
git(workdir, "reset", "-q")
# No checkout known, a workdir with no clone, and a clone beneath the workdir.
other = "11111111-2222-4333-8444-555555555555"
pre(history, sid=other)
assert snapshots()[5]["reason"] == "no workbench job known for this session" and "jobId" not in snapshots()[5], snapshots()[5]
other_job = "3f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5c"
w2 = os.path.join(jobs, other_job)
os.makedirs(w2)
mod._checkouts[other] = (other_job, w2)
mod._last_boundary.pop(other)
pre(history, sid=other)
assert snapshots()[6]["reason"] == f"no git checkout under {w2}", snapshots()[6]
shutil.copytree(workdir, os.path.join(w2, "work"))
mod._last_boundary.pop(other)
pre(history, sid=other)
assert "commit" in snapshots()[7] and "reason" not in snapshots()[7], snapshots()[7]
assert git(os.path.join(w2, "work"), "rev-parse", f"refs/talaria/ckpt/{other}/c3") == snapshots()[7]["commit"]
# A turn that is not the session's own, such as Hermes's post-turn background
# review on the same session object, carries another task id: it is not a
# step of this run and reports nothing.
mod._last_boundary.pop(session, None)
mod.on_pre_api_request(session_id=session, task_id="7991ecb1-0000-4000-8000-000000000000", turn_id=f"{session}:7991ecb1-0000-4000-8000-000000000000:309793a7", api_request_id="x", conversation_history=history)
assert len(snapshots()) == 8, "another task's turn must not report"
mod.on_pre_api_request(session_id=session, task_id=session, turn_id=f"{session}:{session}:0a0b0c0d", api_request_id="x", conversation_history=history)
assert len(snapshots()) == 9, "the session's own turn reports"
mod._last_boundary.pop(session, None)

# Cron turns and sessions with no id report nothing.
mod.on_pre_api_request(session_id=session, platform="cron", conversation_history=history)
mod.on_pre_api_request(session_id="", conversation_history=history)
assert len(snapshots()) == 9

# A job_status listing the agent did not ask about by id binds nothing, even
# when it names exactly one started job: that job may be another session's.
third = "22222222-3333-4444-8555-666666666666"
mod._checkouts.pop(third, None)
post_as = lambda sid, tool, args, result: mod.on_post_tool_call(tool_name=tool, args=args, result=result, session_id=sid, tool_call_id="cx")
post_as(third, "mcp__workbench__job_status", {}, answer({"jobs": [{"id": job_id, "status": "started", "workdir": workdir}]}))
assert third not in mod._checkouts, "an unasked listing must not bind"
post_as(third, "mcp__workbench__job_status", {"jobId": job_id}, answer({"jobs": [{"id": job_id, "status": "started", "workdir": workdir}]}))
assert mod._checkouts[third] == (job_id, workdir)

# A bound job survives a container restart: it is kept on the volume per
# session and read back when memory has nothing.
state_dir = os.path.join(base, "state")
mod._STATE_DIR = state_dir
fourth = "33333333-4444-4555-8666-777777777777"
post_as(fourth, "mcp__workbench__start_job", {}, answer({"jobId": job_id, "status": "started", "workdir": workdir}))
assert os.path.isfile(os.path.join(state_dir, fourth)), os.listdir(state_dir) if os.path.isdir(state_dir) else "no state dir"
mod._checkouts.clear()
pre(history, sid=fourth)
assert snapshots()[-1].get("jobId") == job_id and "commit" in snapshots()[-1], snapshots()[-1]
# An id that is not a session id never becomes a file name.
mod._checkouts.clear()
post_as("../../etc/passwd", "mcp__workbench__start_job", {}, answer({"jobId": job_id, "status": "started", "workdir": workdir}))
assert sorted(os.listdir(state_dir)) == [fourth], os.listdir(state_dir)

# Untracked content over the budget skips the snapshot with a reason and
# leaves the run going; the budget is bytes of untracked, unignored files.
post("mcp__workbench__start_job", {}, answer({"jobId": job_id, "status": "started", "workdir": workdir}))
big = os.path.join(workdir, "blob.bin")
with open(big, "wb") as f:
    f.truncate(mod._UNTRACKED_BUDGET + 1)
history6 = history5 + [{"role": "assistant", "tool_calls": [{"id": "c8"}]}, {"role": "tool", "tool_call_id": "c8", "tool_name": "terminal", "content": "h"}]
pre(history6, req="t1:api:7")
e6 = snapshots()[-1]
assert "commit" not in e6 and e6["reason"].startswith("snapshot skipped: untracked content"), e6
assert "over the" in e6["reason"] and "budget" in e6["reason"], e6["reason"]
os.unlink(big)
# A git killed mid-add leaves no pack garbage behind.
pack = os.path.join(workdir, ".git", "objects", "pack")
os.makedirs(pack, exist_ok=True)
stale = os.path.join(pack, "tmp_pack_old")
open(stale, "w").close()
os.utime(stale, (1, 1))


def hanging_pack(cmd, *a, **kw):
    if "add" in cmd:
        open(os.path.join(pack, "tmp_pack_new"), "w").close()
        raise subprocess.TimeoutExpired(cmd, 1)
    return real_run(cmd, *a, **kw)


mod.subprocess.run = hanging_pack
try:
    history7 = history6 + [{"role": "assistant", "tool_calls": [{"id": "c9"}]}, {"role": "tool", "tool_call_id": "c9", "tool_name": "terminal", "content": "i"}]
    pre(history7, req="t1:api:8")
finally:
    mod.subprocess.run = real_run
assert snapshots()[-1]["reason"].startswith("snapshot failed: TimeoutExpired"), snapshots()[-1]
assert not os.path.exists(os.path.join(pack, "tmp_pack_new")), "the killed add's pack garbage stays"
assert os.path.exists(stale), "another git's older temp file is not ours to remove"
shutil.rmtree(base)
print("snapshot_test passed")
