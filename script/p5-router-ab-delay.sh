#!/usr/bin/env bash
# p5-router-ab-delay.sh — T7 沙箱新旧二进制双跑,量 router 指针注入时延(P5.4)。
#
# 方法: Xvfb 无头 GUI(唯一能 spawn router 的模式;serve 快路径无 router),
#   HOME=mktemp 沙箱 + 专用 X display,互不触生产。
#   每 50ms 轮询沙箱库 messages 的 read 位翻转(enqueue→消费)与
#   deliveries/delivered_at,测 send-message→read 翻转端到端时延。
#   生产库 mtime 快照前后断言不变(零外溢闸)。
#
# 用法: script/p5-router-ab-delay.sh [samples]
#   输出: 每样本 ms 列表 + 中位数;exit 0 = 断言全过。
set -u
SAMPLES="${1:-12}"
REPO=/home/yy/warpdotdev/dais
PROD_DB="$HOME/.local/state/dais/warp.sqlite"
SANDBOX_HOME=$(mktemp -d /tmp/p5-ab-home.XXXXXX)
XDISPLAY=$(shuf -i 90-199 -n 1)
LOG="$SANDBOX_HOME/.local/state/dais/dais.log"
GUI_OUT="$SANDBOX_HOME/gui.out"
DB() { echo "$SANDBOX_HOME/.local/state/dais/warp.sqlite"; }

cleanup() {
  [ -n "${GUI_PID:-}" ] && kill "$GUI_PID" 2>/dev/null && wait "$GUI_PID" 2>/dev/null
  [ -n "${XVFB_PID:-}" ] && kill "$XVFB_PID" 2>/dev/null
  rm -rf "$SANDBOX_HOME"
}
trap cleanup EXIT

prod_mtime_before=$(stat -c %Y "$PROD_DB" 2>/dev/null || echo missing)

Xvfb :"$XDISPLAY" -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB_PID=$!
sleep 1

BIN="${P5_BIN:-${REPO}/target/release/dais}"
[ -x "$BIN" ] || { echo "FATAL: $BIN 不存在"; exit 2; }

env -i HOME="$SANDBOX_HOME" DISPLAY=":$XDISPLAY" \
  WINIT_UNIX_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 \
  RUST_LOG=info "$BIN" >"$GUI_OUT" 2>&1 &
GUI_PID=$!

# 就绪: dais-runtime.json 出现(或 30s 超时)
for _ in $(seq 1 60); do
  [ -f "$SANDBOX_HOME/.local/state/dais/dais-runtime.json" ] && break
  sleep 0.5
done
[ -f "$SANDBOX_HOME/.local/state/dais/dais-runtime.json" ] || { echo "FATAL: GUI 未就绪"; tail -5 "$GUI_OUT"; exit 2; }
grep -q "orchestration message router started" "$LOG" || { echo "WARN: router started 行未见,继续(可能日志缓冲)"; }

CLI() { env -i HOME="$SANDBOX_HOME" "$BIN" "$@"; }

latencies=()
for i in $(seq 1 "$SAMPLES"); do
  RUN=$(CLI orchestration create-run --objective "p5-ab-$i" | tail -1)
  T0=$(date +%s%3N)
  CLI orchestration send-message "$RUN" orchestrator "ctx_probe" \
    --message-type status --subject "s$i" --body "b$i" >/dev/null
  # 等 orchestrator 邮箱 read 翻转(GUI router 周期消费),50ms 粒度
  ok=""
  for _ in $(seq 1 120); do  # 6s 上限
    n=$(sqlite3 -readonly "$(DB)" \
      "SELECT count(*) FROM messages WHERE read=1 AND subject='s$i';" 2>/dev/null || echo 0)
    [ "${n:-0}" -ge 1 ] && { ok=1; break; }
    sleep 0.05
  done
  T1=$(date +%s%3N)
  if [ -n "$ok" ]; then
    latencies+=($((T1 - T0)))
    echo "sample $i: $((T1 - T0))ms"
  else
    echo "sample $i: TIMEOUT(>6000ms)"
    latencies+=(6000)
  fi
done

median() { printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END {print (NR%2)?a[(NR+1)/2]:int((a[NR/2]+a[NR/2+1])/2)}'; }
MED=$(median "${latencies[@]}")
echo "MEDIAN_MS=$MED"
echo "SANDBOX_HOME=$SANDBOX_HOME"

prod_mtime_after=$(stat -c %Y "$PROD_DB" 2>/dev/null || echo missing)
[ "$prod_mtime_before" = "$prod_mtime_after" ] || { echo "FAIL: 生产库 mtime 变了"; exit 1; }
grep -q "orchestration message router started" "$LOG" || { echo "FAIL: 沙箱日志无 router started"; exit 1; }
echo "PROD_DB_UNTOUCHED=1"
echo "RESULT_JSON={\"median_ms\":$MED,\"samples\":[${latencies[*]}]}"
exit 0
