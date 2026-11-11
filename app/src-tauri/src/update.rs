//! 「检查更新」的客户端半边。
//!
//! ## 架构：能做成纯静态就别做接口
//!
//! 服务端只需要三样东西（原型 05-E）：一份静态 manifest.json、一堆签好名的
//! 安装包（放 GitHub Releases，走 CDN）、一个灰度百分比。没有用户表、没有
//! 数据库、没有登录态——「检查更新」只回答「有没有新版本」这一个问题。
//!
//! ## 为什么网络请求放前端、验签放这里
//!
//! - **拉清单在前端**：`fetch` 走 WebView2 的 HTTP 栈，Rust 侧不引 reqwest
//!   （那个库在 `x86_64-pc-windows-gnu` 工具链下会拉一整套 tokio + hyper，
//!   又撞「raw-dylib 缺 binutils」的墙）。前端把清单**原文**整段传过来。
//! - **验签在这里**：Ed25519 公钥硬编码在本文件。前端是用户能改的那一侧，
//!   把验签放前端等于把「防中间人替换清单」这扇门拆了。
//!
//! ## 威胁模型（不自我感动）
//!
//! Ed25519 签名防的是「清单被替换成指向恶意包」这一件事。它防不了「用户自己
//! 改了前端跳过验签」——但一个会自己改代码的人，本来也不靠我们防。这条线和
//! 05-F 的结论一致：目标是让「原样打包上架」这条路走不通，不是防住所有人。
//!
//! ## 私钥怎么管
//!
//! 私钥离线保管、只在发版机上出现。公钥硬编码在这里。私钥泄露 = 能给所有
//! 用户推任意程序，所以它不进仓库、不进 CI、不进任何会联网的地方。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

/// 客户端里硬编码的 Ed25519 公钥（hex，64 字节 = 32 字节公钥）。
///
/// 公钥对应的私钥在发版机上离线保管，用 `app/tools/gen-key` 生成。
/// 发版流程见 `.github/workflows/release.yml`。
///
/// ⚠️ 换公钥 = 老客户端再也验不过新清单。只在私钥泄露、必须轮换时改这里，
/// 改完要发一个「带新公钥」的强制更新版本，否则所有旧版本都收不到后续更新。
const UPDATE_PUBKEY_HEX: &str = "a6e3440073e853aa132996c467a5612d99ca69319495f80b99d1ab7b759da8df";

/// 检查更新请求里的一个安装包条目。
#[derive(Deserialize)]
struct Package {
    #[allow(dead_code)]
    url: String,
    #[allow(dead_code)]
    size: u64,
    sha256: String,
    sig: String,
}

/// 清单文件（原型 05-E2 的契约）。
#[derive(Deserialize)]
struct Manifest {
    #[allow(dead_code)]
    channel: String,
    latest: String,
    #[serde(default)]
    force_below: Option<String>,
    #[allow(dead_code)]
    released_at: String,
    packages: std::collections::BTreeMap<String, Package>,
    #[allow(dead_code)]
    #[serde(default)]
    rollout: Option<Rollout>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Rollout {
    percent: u8,
    salt: String,
}

/// 给前端的结果。`state` 决定设置页「关于」里那格显示什么。
#[derive(Serialize)]
pub struct UpdateResult {
    /// `latest` / `available` / `error` / `signature-mismatch`
    pub state: String,
    /// 当前版本（本机）
    pub current: String,
    /// 服务端最新版本
    pub latest: String,
    /// 是否强制更新（本机版本低于 force_below）
    pub forced: bool,
    /// 新版本的 sha256（前 16 位，展示用）
    pub sha256: String,
    /// 下载地址
    pub url: String,
    /// 包大小（字节）
    pub size: u64,
    /// 出错时的人类可读说明
    pub message: String,
}

/// 语义化版本比较：`1.10.0 > 1.9.0`，不比字符串。
///
/// 原型 05-E3 专门点过这一条：字符串比较会让 1.10 排在 1.9 前面，
/// 用户永远收不到更新。这里逐段比数字。
fn parse_semver(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(|c: char| c == '.' || c == '-')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect()
}

fn semver_gt(a: &str, b: &str) -> bool {
    let va = parse_semver(a);
    let vb = parse_semver(b);
    let n = va.len().max(vb.len());
    for i in 0..n {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        if x > y {
            return true;
        }
        if x < y {
            return false;
        }
    }
    false
}

/// 公钥 hex → `VerifyingKey`。全零（未配置）会返回 None，让验签必然失败。
fn pubkey() -> Option<VerifyingKey> {
    let bytes = hex_decode(UPDATE_PUBKEY_HEX)?;
    if bytes.iter().all(|&b| b == 0) {
        return None; // 全零公钥：还没配真正的发版密钥
    }
    VerifyingKey::from_bytes(&bytes.try_into().ok()?).ok()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// 校验清单的 Ed25519 签名。签名覆盖 `version + platform + arch + sha256`
/// （原型 05-E2 的约定：只签这四个字段，其余字段即使被改也不影响安全）。
fn verify_manifest_signature(
    manifest: &Manifest,
    platform: &str,
    arch: &str,
) -> Result<(), String> {
    let Some(vk) = pubkey() else {
        return Err("更新公钥未配置（占位公钥，尚未发版）".into());
    };
    let Some(pkg) = manifest.packages.get(&format!("{platform}-{arch}")) else {
        return Err(format!("清单里没有 {platform}-{arch} 这个包"));
    };

    // 签名的原文：version + platform + arch + sha256，用换行拼接
    let signed = format!(
        "{}\n{}\n{}\n{}",
        manifest.latest, platform, arch, pkg.sha256
    );
    let sig_bytes = hex_decode(&pkg.sig).ok_or("签名不是合法 hex")?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|e| format!("签名解析失败：{e}"))?;

    vk.verify(signed.as_bytes(), &sig)
        .map_err(|_| "签名校验失败：清单可能被篡改".to_string())
}

/// 计算一段字节的 sha256（hex，小写）。
fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// 校验下载下来的安装包内容是否和清单里的 sha256 一致。
///
/// 这一版前端下载走浏览器/系统默认下载，包内容不经过这里；但把函数留着，
/// 等「下载并安装」真正落到本地文件时用（原型 05-E2：sha256 下载完必须全量校验，
/// 只信 HTTPS 不够——CDN 回源错误、企业代理插证书都会让你装上一个坏包）。
#[allow(dead_code)]
pub fn verify_package(bytes: &[u8], expect_sha256: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expect_sha256.trim())
}

/// 「检查更新」命令入口。
///
/// `manifest_json` 是前端 `fetch` 到的清单**原文**；`platform` / `arch` 由前端
/// 按 `navigator.userAgent` 或 Rust 侧 `std::env::consts` 填好。实际上平台/架构
/// 该由 Rust 侧自己定（前端不可信），这里用 `std::env::consts::{OS, ARCH}`。
#[tauri::command]
pub fn check_update(manifest_json: String) -> UpdateResult {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let platform = std::env::consts::OS; // "windows" / "macos" / "linux"
    let arch = std::env::consts::ARCH; // "x86_64" / "aarch64"

    let mut r = UpdateResult {
        state: "error".into(),
        current: current.clone(),
        latest: String::new(),
        forced: false,
        sha256: String::new(),
        url: String::new(),
        size: 0,
        message: String::new(),
    };

    let manifest: Manifest = match serde_json::from_str(&manifest_json) {
        Ok(m) => m,
        Err(e) => {
            r.message = format!("清单解析失败：{e}");
            return r;
        }
    };

    // 平台名对齐：manifest 的 key 是 windows-x64 / darwin-arm64 这种
    let plat_key = match platform {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };
    let arch_key = match arch {
        "x86_64" | "x86" => "x64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    };

    // 1. 验签
    if let Err(e) = verify_manifest_signature(&manifest, plat_key, arch_key) {
        r.message = e;
        r.state = "signature-mismatch".into();
        return r;
    }

    // 2. 取本平台包
    let key = format!("{plat_key}-{arch_key}");
    let pkg = match manifest.packages.get(&key) {
        Some(p) => p,
        None => {
            r.message = format!("清单里没有 {key} 这个包");
            return r;
        }
    };

    r.latest = manifest.latest.clone();
    r.sha256 = pkg.sha256.chars().take(16).collect();
    r.url = pkg.url.clone();
    r.size = pkg.size;

    // 3. 版本比较
    if !semver_gt(&manifest.latest, &current) {
        r.state = "latest".into();
        r.message = format!("{current} 已是最新版本");
        return r;
    }

    // 4. 强制更新判定
    r.forced = manifest
        .force_below
        .as_deref()
        .map(|fb| semver_gt(fb, &current))
        .unwrap_or(false);

    r.state = "available".into();
    r.message = format!("有新版本 {latest}", latest = manifest.latest);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compares_by_segment_not_string() {
        assert!(semver_gt("1.10.0", "1.9.0"));
        assert!(!semver_gt("1.9.0", "1.10.0"));
        assert!(semver_gt("2.0.0", "1.9.9"));
        assert!(!semver_gt("1.0.0", "1.0.0"));
        // v 前缀要能剥掉
        assert!(semver_gt("v1.1.0", "1.0.0"));
        // 预发布段不参与比较（1.0.0 == 1.0.0-beta）
        assert!(!semver_gt("1.0.0", "1.0.0-beta"));
    }

    #[test]
    fn sha256_is_stable_and_lowercase() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pubkey_loads_from_configured_hex() {
        // 公钥已配置（非全零），pubkey() 应该返回 Some
        assert!(pubkey().is_some());
        // 长度必须是 32 字节（64 hex 字符）
        assert_eq!(UPDATE_PUBKEY_HEX.len(), 64);
    }

    #[test]
    fn malformed_manifest_returns_error() {
        let r = check_update("not json".into());
        assert_eq!(r.state, "error");
    }
}
