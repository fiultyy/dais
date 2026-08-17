# FORK.md — dais 血统与血源政策

> 本仓原名 zap(zap-oss),2026-08-17 起更名 **dais**(点将台:指挥官点将派票、全军听令之处)。

## 血统链

```
warpdotdev/warp          原始上游,活跃(本地镜像 ~/tools/warp-dev,remote 名: warp)
  └─ zerx-lab/zap        中间 fork,止于 5d874456 (2026-07-09),已停更(转向云端编排业务)
      └─ fiultyy/dais    本仓。自 5d874456 起自立血源,含 zap 期 T1-T11 intercept 系列
```

## 血源政策

- **warp remote 只做引擎输血**:仅 cherry-pick 终端核心 / warpui / rendering / grid 相关修复与优化;
- **不跟 Warp 产品方向**:cloud / AI 订阅 / 账号体系 / 遥测 / 更新通道一律不吸收;
- zerx-lab/zap 已死,不再作为 merge 目标,历史仅作考古。

## 已砍面

zap 层已移除遥测 / 更新通道 / 云功能(尾部如有残留,随票清点)。`remote_server` 的
自更新 release URL 已指向本仓 releases。

## 命名映射

| 旧 | 新 |
|---|---|
| 仓库名 zap | dais (github.com/fiultyy/dais) |
| binary/crate 名 zap-oss | dais |
| 配置路径 `~/.config/zap` | **保持不动**(用户数据兼容,不改) |

注:crates.io 的 `dais` 名被一个无关小 crate 占用;本工程为 workspace binary,
不经 crates.io 发布,不受影响;如未来需发布,cargo 包名加 `-app` 后缀即可。

## License

沿用上游 Warp 的 MIT / AGPL-3.0 双许可(见 LICENSE-MIT, LICENSE-AGPL),版权头部保留。
