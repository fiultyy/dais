//! 本地 CA: 首次生成自签 CA + server cert, 持久化到 ~/.config/zap/proxy-ca/。

use std::path::PathBuf;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};

use crate::Result;

#[derive(Debug, Clone)]
pub struct LocalCA {
    pub dir: PathBuf,
    pub ca_cert_path: PathBuf,
    pub ca_key_path: PathBuf,
    pub server_cert_path: PathBuf,
    pub server_key_path: PathBuf,
}

/// 首次生成并持久化; 后续直接加载。4 个 PEM 文件齐备即视为有效。
pub fn ensure_local_ca() -> Result<LocalCA> {
    let home = std::env::var("HOME")?;
    let dir = PathBuf::from(home)
        .join(".config")
        .join("zap")
        .join("proxy-ca");
    let ca = LocalCA {
        ca_cert_path: dir.join("ca-cert.pem"),
        ca_key_path: dir.join("ca-key.pem"),
        server_cert_path: dir.join("server-cert.pem"),
        server_key_path: dir.join("server-key.pem"),
        dir,
    };

    let loaded = [
        &ca.ca_cert_path,
        &ca.ca_key_path,
        &ca.server_cert_path,
        &ca.server_key_path,
    ];
    if loaded.iter().all(|p| p.is_file()) {
        return Ok(ca);
    }

    std::fs::create_dir_all(&ca.dir)?;

    // 自签 CA
    let mut ca_params = CertificateParams::new(Vec::new())?;
    ca_params.distinguished_name.push(DnType::CommonName, "Zap Local Proxy CA");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate()?;
    let ca_cert = ca_params.self_signed(&ca_key)?;
    std::fs::write(&ca.ca_cert_path, ca_cert.pem())?;
    std::fs::write(&ca.ca_key_path, ca_key.serialize_pem())?;

    // server cert (SAN: localhost + 127.0.0.1), 由 CA 签发
    let server_key = KeyPair::generate()?;
    let mut server_params = CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    server_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)?;
    std::fs::write(&ca.server_cert_path, server_cert.pem())?;
    std::fs::write(&ca.server_key_path, server_key.serialize_pem())?;

    Ok(ca)
}
