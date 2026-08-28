#!/usr/bin/env bash
# 同步 app/i18n (权威源) → crates/i18n/i18n (crate 内构建副本)。
# 幂等: 内容一致时无操作 (md5 对账), exit 0; 有差异则覆盖并报告。
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=app/i18n
DST=crates/i18n/i18n

changed=0
for locale_dir in "$SRC"/*/; do
  locale=$(basename "$locale_dir")
  mkdir -p "$DST/$locale"
  for ftl in "$locale_dir"*.ftl; do
    base=$(basename "$ftl")
    dst_file="$DST/$locale/$base"
    if [[ ! -f "$dst_file" ]] || ! cmp -s "$ftl" "$dst_file"; then
      cp "$ftl" "$dst_file"
      echo "synced: $ftl -> $dst_file"
      changed=$((changed+1))
    fi
  done
done

if [[ $changed -eq 0 ]]; then
  echo "i18n ftl already in sync (idempotent no-op)"
fi
