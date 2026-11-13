#!/bin/sh
# Live OMP decision smoke. Needs a configured model/provider and consumes model tokens.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
bin="${LYRA_BIN:-$root/rust/target/release/lyra}"
if [ ! -x "$bin" ]; then
  cargo build --release --quiet --locked --manifest-path "$root/rust/Cargo.toml"
fi

tmp=$(mktemp -d)
repo="$tmp/repo"
home="$tmp/home"
mkdir -p "$repo" "$home"
trap 'rm -rf "$tmp"' EXIT

cd "$repo"
git init -q
git config user.email eval@example.com
git config user.name Eval
printf 'live eval\n' > README.md
git add README.md
git commit -qm init
cp "$here/fixtures/fake-omp.sh" "$tmp/fake-omp.sh"
chmod +x "$tmp/fake-omp.sh"
LYRA_HOME="$home" "$bin" init >/dev/null

prompt='You are testing Lyra through its typed tools. Do not use bash. Follow exactly: (1) call lyra_status; (2) if there are no tickets, call lyra_tickets_create with four tickets: alpha and beta are ready; gamma is blockedBy alpha; delta is blockedBy beta. Give every ticket a subject and prompt; (3) call lyra_start with maxParallel 2 and backend headless; (4) call lyra_wait with the returned run id and timeoutMs 5000, then repeat with the returned revision until the run is done or blocked; (5) call lyra_control close with the run id; (6) reply with one JSON object containing runId and final status. Use only the lyra_* tools.'

out="$tmp/events.jsonl"
err="$tmp/omp.err"
if ! LYRA_BIN="$bin" LYRA_HOME="$home" LYRA_PANE_BACKEND=headless LYRA_OMP_BIN="$tmp/fake-omp.sh" FAKE_MODE=done \
  omp --print --mode json --no-session --max-time 180s \
    --tools=lyra_status,lyra_tickets_create,lyra_start,lyra_wait,lyra_control \
    --approval-mode yolo "$prompt" >"$out" 2>"$err"; then
  echo "live OMP run failed:" >&2
  tail -40 "$err" >&2
  tail -40 "$out" >&2
  exit 1
fi

echo "--- live OMP tool events ---"
grep -o '"toolName":"lyra_[a-z_]*"' "$out" || true
echo "--- final text tail ---"
tail -20 "$out"
if grep -q '"toolName":"bash"' "$out"; then
  echo "live eval failed: agent used bash despite Lyra-only tools" >&2
  exit 1
fi
for tool in lyra_status lyra_tickets_create lyra_start lyra_wait lyra_control; do
  if ! grep -q "\"toolName\":\"$tool\"" "$out"; then
    echo "live eval failed: missing $tool tool event" >&2
    exit 1
  fi
done

python3 - "$out" <<'PY'
import json
import sys

last = None
with open(sys.argv[1]) as handle:
    for line in handle:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "tool_execution_end" and event.get("toolName") == "lyra_control":
            last = event
if last is None:
    raise SystemExit("live eval failed: no lyra_control result")
details = last["result"]["details"]
if details.get("status") != "closed":
    raise SystemExit(f"live eval failed: run status {details.get('status')}")
tickets = details.get("tickets", [])
if len(tickets) != 4 or not all(ticket.get("status") == "done" for ticket in tickets):
    raise SystemExit(f"live eval failed: tickets {tickets}")
if any(str(ticket.get("id", "")).startswith("T-") for ticket in tickets):
    raise SystemExit(f"live eval failed: numeric ticket ids {tickets}")
print("live tickets:", ", ".join(ticket["id"] for ticket in tickets))
PY
