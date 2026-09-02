#!/usr/bin/env bash
# 同步 app/i18n (权威源) → 各下沉 crate 的本地 ftl 副本 (crates/*/i18n/)。
# 自动发现: 扫描 crates/*/i18n/,按物理路径去重(nav/i18n 为指向
#   crates/i18n/i18n 的软链,与本体合并为同一目标);凡目录内已含 .ftl
#   文件者即为同步目标 —— 下沉 crate 需要本地副本时(fl! 编译期校验 /
#   运行时内嵌,先例 crates/drive_ui/i18n/,见 606f57bd),由下沉提交创建
#   首份副本,此后本脚本自动发现并保持同步;仅占位 i18n.toml 而无 ftl
#   的 crate(如 env_vars_ui,资源在 crates/i18n/i18n/)自动跳过。
#   历史唯一目标 crates/i18n/i18n 亦命中该规则,无需特判。
# 幂等: 内容一致时无操作 (cmp 对账), exit 0; 有差异则覆盖并报告。
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=app/i18n

changed=0
targets=()
for DST in crates/*/i18n/; do
  [[ -d "$DST" ]] || continue                 # glob 未命中时排除字面量
  phys=$(cd "$DST" && pwd -P)                 # 物理路径去重(软链别名合并)
  [[ " ${targets[*]-} " != *" $phys "* ]] || continue
  # 仅同步已声明 ftl 资产的副本目录(至少含一个 .ftl);占位空目录跳过
  [[ -n "$(find "$phys" -type f -name '*.ftl' -print -quit)" ]] || continue
  targets+=("$phys")
  for locale_dir in "$SRC"/*/; do
    locale=$(basename "$locale_dir")
    mkdir -p "$phys/$locale"
    for ftl in "$locale_dir"*.ftl; do
      [[ -e "$ftl" ]] || continue             # locale 目录无 ftl 时跳过
      base=$(basename "$ftl")
      dst_file="$phys/$locale/$base"
      if [[ ! -f "$dst_file" ]] || ! cmp -s "$ftl" "$dst_file"; then
        cp "$ftl" "$dst_file"
        echo "synced: $ftl -> ${dst_file#"$PWD"/}"
        changed=$((changed+1))
      fi
    done
  done
done

if [[ ${#targets[@]} -eq 0 ]]; then
  echo "error: 未发现任何含 .ftl 的 crates/*/i18n/ 同步目标" >&2
  exit 1
fi

if [[ $changed -eq 0 ]]; then
  echo "i18n ftl already in sync across ${#targets[@]} target(s) (idempotent no-op)"
fi
