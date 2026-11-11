//! 生成「检查更新」的 manifest.json。
//!
//! 用法：
//!
//! ```bash
//! cd app/tools/make-manifest && cargo run --release -- \
//!   --version 0.1.0 \
//!   --channel stable \
//!   --priv <私钥 hex> \
//!   --pkg windows-x64 /path/to/paypulse_0.1.0_x64-setup.exe \
//!   --pkg windows-x64 https://github.com/.../releases/download/.../setup.exe \
//!   --out manifest.json
//! ```
//!
//! 每个 `--pkg <platform-arch> <本地文件或 URL>` 指定一个安装包：
//! - 第二个参数如果是本地存在的文件，就算它的 sha256；
//! - 如果是一个 http(s) URL，就把它当下载地址（sha256 必须另给，见下）。
//!
//! 签名原文与 `update.rs` 的 `verify_manifest_signature` 对齐：
//! `{version}\n{platform}\n{arch}\n{sha256}`，用换行拼接，Ed25519 签名。
//!
//! 产物是符合原型 05-E2 契约的 manifest.json，可直接放 GitHub Releases。

use ed25519_dalek::Signer;
use ed25519_dalek::SigningKey;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Serialize)]
struct Package {
    url: String,
    size: u64,
    sha256: String,
    sig: String,
}

#[derive(Serialize)]
struct Manifest {
    channel: String,
    latest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    force_below: Option<String>,
    released_at: String,
    packages: BTreeMap<String, Package>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollout: Option<Rollout>,
}

#[derive(Serialize)]
struct Rollout {
    percent: u8,
    salt: String,
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut version = String::new();
    let mut channel = "stable".to_string();
    let mut priv_hex = String::new();
    let mut force_below: Option<String> = None;
    let mut out = "manifest.json".to_string();
    let mut pkg_specs: Vec<(String, String)> = Vec::new(); // (platform-arch, 文件或 URL)

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                i += 1;
                version = args[i].clone();
            }
            "--channel" => {
                i += 1;
                channel = args[i].clone();
            }
            "--priv" => {
                i += 1;
                priv_hex = args[i].clone();
            }
            "--force-below" => {
                i += 1;
                force_below = Some(args[i].clone());
            }
            "--out" => {
                i += 1;
                out = args[i].clone();
            }
            "--pkg" => {
                let key = args[i + 1].clone();
                let val = args[i + 2].clone();
                pkg_specs.push((key, val));
                i += 2;
            }
            other => {
                eprintln!("不认识的参数：{other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if version.is_empty() || priv_hex.is_empty() || pkg_specs.is_empty() {
        eprintln!("用法：--version <v> --priv <私钥hex> --pkg <platform-arch> <文件|URL> [--out file.json]");
        std::process::exit(2);
    }

    let priv_bytes = hex::decode(&priv_hex).expect("私钥不是合法 hex");
    let signing_key: SigningKey = SigningKey::from_bytes(&priv_bytes.try_into().expect("私钥长度不对"));

    let mut packages = BTreeMap::new();
    for (key, val) in &pkg_specs {
        // key 形如 windows-x64 / darwin-arm64，拆 platform + arch
        let (platform, arch) = match key.split_once('-') {
            Some((p, a)) => (p, a),
            None => {
                eprintln!("包 key 必须是 platform-arch 格式（如 windows-x64），got {key}");
                std::process::exit(2);
            }
        };

        // 本地文件 → 算 sha256；URL → sha256 得从别处给（这一版不支持，直接报错）
        let (url, size, sha256) = if std::path::Path::new(val).exists() {
            let data = std::fs::read(val).expect("读安装包失败");
            let h = sha256_hex(&data);
            let u = format!("{val}"); // 本地路径先占位，发版时替换成 Release 下载 URL
            (u, data.len() as u64, h)
        } else if val.starts_with("http://") || val.starts_with("https://") {
            eprintln!(
                "URL 形式的包（{}）需要手动给 sha256——这一版工具只支持本地文件。",
                val
            );
            eprintln!("先把包下到本地，用文件路径再跑一次。");
            std::process::exit(2);
        } else {
            eprintln!("找不到文件：{val}");
            std::process::exit(2);
        };

        // 签名原文：version\nplatform\narch\nsha256
        let signed = format!("{version}\n{platform}\n{arch}\n{sha256}");
        let sig = signing_key.sign(signed.as_bytes());
        let sig_hex = hex::encode(sig.to_bytes());

        packages.insert(
            key.clone(),
            Package {
                url,
                size,
                sha256,
                sig: sig_hex,
            },
        );
    }

    let manifest = Manifest {
        channel,
        latest: version,
        force_below,
        released_at: chrono_now(),
        packages,
        rollout: None,
    };

    let json = serde_json::to_string_pretty(&manifest).expect("序列化失败");
    std::fs::write(&out, json).expect("写 manifest 失败");
    println!("已生成 {out}");
    println!("包含 {} 个包：{:?}", packages_len(&pkg_specs), pkg_specs.iter().map(|(k, _)| k).collect::<Vec<_>>());
}

fn packages_len(specs: &[(String, String)]) -> usize {
    specs.len()
}

/// 当前 UTC 时间，RFC3339。
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 简化：输出 Unix 秒的 ISO 近似（不引 chrono，避免额外依赖）
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 从 1970-01-01 起算日期（不精确到闰秒，发版时间戳够用）
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 天数 → 公历日期（Howard Hinnant 的算法）。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
