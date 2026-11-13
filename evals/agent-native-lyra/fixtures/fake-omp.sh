#!/bin/sh
set -eu
case "${1:-}" in
  models) printf '{"models":[]}\n'; exit 0 ;;
  usage) printf '{"providers":[]}\n'; exit 0 ;;
esac
cfg="${LYRA_WORKER_CONFIG:?}"
python3 - "$cfg" <<'PY'
import json, os, sys, time, uuid
cfg = json.load(open(sys.argv[1]))
ch = cfg["channel"]
ticket = cfg["ticketId"]
parent = os.environ.get("FAKE_PARENT", "")
mode = os.environ.get("FAKE_MODE", "done")
os.makedirs(os.path.dirname(ch["state"]), exist_ok=True)
if mode == "slow":
    time.sleep(1.5)
if mode == "blocked" and ticket == parent and not cfg.get("continuation"):
    rid = str(uuid.uuid4())
    request = {"requestId": rid, "kind": "blocker", "reason": "need helper", "subject": "Helper", "prompt": "Do helper"}
    with open(os.path.join(ch["requests"], rid + ".json"), "w") as handle:
        json.dump(request, handle)
    response = None
    deadline = time.time() + 10
    while time.time() < deadline:
        path = os.path.join(ch["responses"], rid + ".json")
        if os.path.exists(path):
            response = json.load(open(path))
            break
        time.sleep(0.05)
    if response is None:
        result = {"schemaVersion": 1, "ticketId": ticket, "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "failed", "summary": "blocker request timed out", "verification": "", "evidence": [], "blockers": [], "finishedAt": "2026-01-01T00:00:00.000Z"}
    else:
        result = {"schemaVersion": 1, "ticketId": ticket, "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "blocked", "summary": "waiting for helper", "verification": "", "evidence": [], "blockers": [response["ticketId"]], "finishedAt": "2026-01-01T00:00:00.000Z"}
elif mode == "needs_input" and ticket == parent and not cfg.get("continuation"):
    result = {"schemaVersion": 1, "ticketId": ticket, "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "needs_input", "summary": "Which environment should this target?", "verification": "", "evidence": [], "blockers": [], "finishedAt": "2026-01-01T00:00:00.000Z"}
else:
    result = {"schemaVersion": 1, "ticketId": ticket, "runId": cfg["runId"], "claimId": cfg["claimId"], "status": "done", "summary": "completed", "verification": "fake checks", "evidence": [], "blockers": [], "finishedAt": "2026-01-01T00:00:00.000Z"}
tmp = ch["result"] + ".tmp"
with open(tmp, "w") as handle:
    json.dump(result, handle)
os.replace(tmp, ch["result"])
PY
