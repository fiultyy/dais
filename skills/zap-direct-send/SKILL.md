---
name: zap-direct-send
description: >-
  Drive another agent harness's turn from dais's orchestration plane: send
  messages into a target terminal's session mailbox, get an idle-pointer
  pushed into its PTY, block on replies with check-messages --wait, inject
  full prompts with bracketed paste, and assign dispatches to panes. Use
  when the user says "direct send", "inject prompt", "poke terminal",
  "message another agent", "drive the other harness", "cross-harness",
  "session mailbox", or "dais orchestration". Use plain terminal send for
  one-off shell input with no mailbox semantics; use full task dispatch
  (create-run/create-task/start-worker) for supervised work tracking.
---

# Dais Direct Send (cross-harness turn driving)

Send a message into another harness's terminal and get its agent to act on
it in a new turn — via dais's orchestration plane (`dais orchestration ...`).
The target can be any dais terminal pane (its session mailbox) or a dispatched
worker.

## When to use / not use

- Use: durable message + guaranteed pull by the target agent; blocking
  wait for a typed reply; driving an agent that polls `check-messages`.
- Use: injecting a full task prompt into an idle agent TUI (bracketed paste).
- Not use: one-off shell keystrokes with no reply tracking — plain terminal
  send is cheaper.
- Not use: supervised multi-task runs with DAG/circuit-breaker semantics —
  use `create-run`/`create-task`/`start-worker` + this skill's send for the
  worker channel.

## Prerequisites

- dais GUI app running (the router thread + PTY bridge live in the GUI
  process; CLI subcommands share its SQLite).
- Target terminal pane has a bootstrapped shell → it auto-registers a
  session mailbox `session_<sid>`. Find `<sid>` in the GUI log line
  `Shell is bootstrapped with session_id SessionId(<sid>)`.
- The target agent TUI must set an idle-signalling terminal title
  (claude/gemini/omp do; a bare bash does not — push waits until idle).

## Commands

Every command runs as `dais orchestration <sub>`. Messages persist in
SQLite; nothing is lost if the target is busy.

### Send a message into the target's turn

  # RULE: body is what the target agent reads. For lifecycle messages
  # (worker_done/heartbeat) body MUST be a JSON object matching dais's
  # reconcile schema — invalid JSON leaves it undeliverable.
  dais orchestration send-message <run_id> <from_handle> <to_handle> \
    --message-type status --subject "<short>" --body "<content>"
  # → enqueued seq=N

### Pull a mailbox (authoritative consumer)

  dais orchestration check-messages <handle>
  # RULE: pulling marks messages read. One-shot scripts pull once; agents
  # embedded in a terminal should poll or use --wait.

### Block until a typed message arrives

  # RULE: --wait registers a claim in SQLite; the push plane then skips
  # this mailbox's claimed types (no double consumption). Timeout is not
  # an error — a same-filter re-read always follows. Default 2 min.
  dais orchestration check-messages <handle> --wait --timeout-ms 120000 \
    --type worker_done
  # → message text, or "timed_out" + final re-read

### Inject a full prompt into an idle agent terminal

  # RULE: only reaches a terminal whose OSC title reads idle (or --force).
  # Bytes are bracketed-paste framed + sanitized; a lone CR submits after
  # 500 ms so the target TUI treats it as one pasted prompt.
  dais orchestration inject-prompt <dispatch_id> "<full task text>" [--force]

### Assign a dispatch to the active pane

  dais orchestration assign <dispatch_id>
  # → binds PTY write + tail read + shell-event routing to that pane

## Semantics a caller must know

- **Push on idle**: the router polls every 500 ms; when the target's title
  reads idle and unclaimed mail exists, it writes one pointer line
  (`You have N orchestration message(s). Run 'dais orchestration
  check-messages <handle>'`) + Enter. Bodies are never pushed — pull only.
- **One pointer per new watermark**: re-announcing is suppressed until a
  newer sequence arrives; `delivered_at` advances only after a successful
  PTY write.
- **Waiter mutual exclusion**: a live `--wait` claim hides its claimed
  types from push; claims expire (~15 s TTL, refreshed while polling), so a
  dead waiter cannot wedge the mailbox.
- **Retire on exit**: when the pane's shell exits, its mailbox is
  unregistered (watermark cleared) — a same-id reborn pane never receives
  a stale Enter.

## Failure modes

- Target busy → message stays pending (`read=0, delivered_at IS NULL`);
  pushed at the next idle edge. Safe to re-send.
- PTY dead → write fails, watermark frozen; shell-exit retire cleans up.
- `no terminal view registered` → pane not assigned/bootstrapped; check
  the GUI log for the session id and use `assign`.
- Headless CLI without the GUI app → router/PTY bridge absent; only the
  pull path (`check-messages`) works.
