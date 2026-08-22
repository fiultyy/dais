use super::is_dais_bundle;

#[test]
fn is_dais_bundle_recognises_zap_channels() {
    // OSS (Dais) 自身。
    assert!(is_dais_bundle("dev.zap.Dais"));
    // 上游 Warp 各 channel —— 同样视为本应用家族,允许 default-app 重定向。
    assert!(is_dais_bundle("dev.warp.Dais"));
    assert!(is_dais_bundle("dev.warp.WarpDev"));
    assert!(is_dais_bundle("dev.warp.WarpPreview"));
    assert!(is_dais_bundle("dev.warp.WarpOss"));
}

#[test]
fn is_dais_bundle_rejects_other_apps() {
    assert!(!is_dais_bundle("com.microsoft.VSCode"));
    assert!(!is_dais_bundle("com.apple.TextEdit"));
    assert!(!is_dais_bundle("dev.zed.Zed"));
    assert!(!is_dais_bundle("invalid"));
    assert!(!is_dais_bundle(""));
}
