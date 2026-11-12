//! 生成「检查更新」的 Ed25519 密钥对。
//!
//! 用法（在发版机上跑一次）：
//!
//! ```bash
//! cd app/tools/gen-key
//! cargo run --release -- --out ~/.paypulse-signing-key.txt
//! ```
//!
//! ## 为什么私钥不再打印到终端
//!
//! 上一版直接把私钥 hex `println!` 出来。发版机上跑一次，它就同时存在于：
//! 终端回滚缓冲、CI 日志（如果有人把这段贴过去）、以及任何人截的屏。
//! 这个工程的私钥确实就是这样泄露过一次，只能整对轮换——而换公钥意味着
//! 所有旧客户端从此验不过新清单，代价由用户承担。
//!
//! 所以现在默认走文件：私钥写盘（权限 600），终端只给公钥、指纹和文件路径。
//! 要在没有文件系统的环境里生成，才显式加 `--stdout`（会在终端打印，慎用）。
//!
//! 输出三样：
//! - **公钥 hex**：32 字节，填进 `app/src-tauri/src/update.rs` 的
//!   `UPDATE_PUBKEY_HEX` 常量。
//! - **私钥 hex**：32 字节种子，**离线保管**（写进密码管理器 / 纸面备份），
//!   绝不出现在仓库、CI、日志、聊天记录里。
//! - **指纹**：公钥 sha256 的前 16 个 hex 字符，用来核对「这把钥匙是哪把」，
//!   不用把整串公钥念出来。
//!
//! 私钥泄露 = 能给所有用户推任意程序，所以它只在这台发版机上存在。
//!
//! 签名原文档（和 update.rs 里 `verify_manifest_signature` 对齐）：
//! `{version}\n{platform}\n{arch}\n{sha256}`，用换行拼接，Ed25519 签名。

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--check <私钥文件>`：从已有私钥反推公钥与指纹，用来核对
    // 「写进 update.rs 的公钥」和「配进 CI 的私钥」是不是同一对。
    // 换钥匙时最容易出的错就是把新私钥配上、旧公钥留着——那会让所有新版本
    // 的清单全部验签失败，而且只有装到用户机器上才看得见。
    if let Some(i) = args.iter().position(|a| a == "--check") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("用法：gen-update-key --check <私钥文件>");
            std::process::exit(2);
        };
        match check(path) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("校验失败：{e}");
                std::process::exit(1);
            }
        }
    }

    let to_stdout = args.iter().any(|a| a == "--stdout");
    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    let pub_hex = hex::encode(verifying_key.as_bytes());
    let priv_hex = hex::encode(signing_key.to_bytes());

    // 指纹：公钥的 sha256 前 16 位。认钥匙用这个，不用念整串。
    let mut h = Sha256::new();
    h.update(verifying_key.as_bytes());
    let digest = h.finalize();
    let fingerprint: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();

    println!("════════════════════════════════════════════════════════════");
    println!("  Ed25519 密钥对已生成");
    println!("════════════════════════════════════════════════════════════");
    println!();
    println!("【公钥】（填进 update.rs 的 UPDATE_PUBKEY_HEX）");
    println!("{pub_hex}");
    println!();
    println!("【指纹】{fingerprint}");

    if to_stdout {
        println!();
        println!("【私钥】（--stdout 模式，会留在终端回滚缓冲里）");
        println!("{priv_hex}");
        println!();
        println!("════════════════════════════════════════════════════════════");
        println!("  警告：私钥一旦泄露，攻击者可给所有用户推送任意程序。");
        println!("  跑完请清屏（clear），别把这段贴进聊天或 issue。");
        println!("════════════════════════════════════════════════════════════");
        return;
    }

    // 默认：写文件，权限尽量收紧。
    let path = out_path.unwrap_or_else(|| {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        format!("{home}/.paypulse-signing-key.txt")
    });

    let body = format!(
        "# paypulse 更新签名私钥（Ed25519 种子，hex）\n\
         # 指纹 {fingerprint}\n\
         # 生成于 {}，仅供 GitHub secret UPDATE_SIGNING_KEY 使用。\n\
         # 导入密码管理器后立刻删除本文件。\n\
         {priv_hex}\n",
        chrono_like_now()
    );

    match write_private(&path, &body) {
        Ok(()) => {
            println!();
            println!("【私钥】已写入文件（不在终端显示）：{path}");
            println!("  1) 导入密码管理器 / 抄到纸上");
            println!("  2) export UPDATE_SIGNING_KEY=\"$(grep -v '^#' '{path}' | tr -d '\\n')\"");
            println!("  3) bash publish.sh setup-secrets");
            println!("  4) 删除该文件：rm '{path}'");
        }
        Err(e) => {
            eprintln!();
            eprintln!("写私钥文件失败：{e}");
            eprintln!("改用 --stdout 手动抄写（会留在终端里，注意清理）：");
            eprintln!("  cargo run --release -- --stdout");
            std::process::exit(1);
        }
    }
}

/// `--check`：读私钥文件（跳过 `#` 注释行），反推公钥与指纹。
///
/// 同时把 `update.rs` 里当前的 `UPDATE_PUBKEY_HEX` 读出来比对——如果两处不一致，
/// 就明确告诉用户「CI 会用这把私钥签名，客户端却拿另一把公钥验」，而不是让他
/// 等到装到用户机器上才发现更新全红。
fn check(path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读不到 {path}：{e}"))?;
    let priv_hex: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("");
    if priv_hex.len() != 64 {
        return Err(format!(
            "私钥应该是 64 个 hex 字符（32 字节种子），读到 {} 个",
            priv_hex.len()
        ));
    }
    let seed: [u8; 32] = hex::decode(&priv_hex)
        .map_err(|e| format!("私钥不是合法 hex：{e}"))?
        .try_into()
        .map_err(|_| "私钥长度不对".to_string())?;

    let vk = SigningKey::from_bytes(&seed).verifying_key();
    let pub_hex = hex::encode(vk.as_bytes());
    let mut h = Sha256::new();
    h.update(vk.as_bytes());
    let fingerprint: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();

    println!("私钥文件：{path}");
    println!("对应公钥：{pub_hex}");
    println!("指纹　　：{fingerprint}");

    // 顺带跟源码里那把比一下（找不到源码就只打印）
    let src = std::path::Path::new("../../src-tauri/src/update.rs");
    if let Ok(rs) = std::fs::read_to_string(src) {
        if let Some(line) = rs.lines().find(|l| l.contains("UPDATE_PUBKEY_HEX: &str")) {
            if let Some(start) = line.find('"') {
                if let Some(end) = line[start + 1..].find('"') {
                    let in_src = &line[start + 1..start + 1 + end];
                    if in_src == pub_hex {
                        println!("\n与 update.rs 一致 ✓");
                    } else {
                        println!("\n⚠ 与 update.rs 不一致！");
                        println!("  源码里：{in_src}");
                        println!("  本私钥：{pub_hex}");
                        println!("  → 这对密钥签出来的清单，客户端会一律验签失败。");
                        return Err("私钥与客户端公钥不匹配".into());
                    }
                }
            }
        }
    }
    Ok(())
}

/// 写私钥文件。Unix 上先建成 600 再写内容（避免「先建 644 再 chmod」那段窗口期），
/// Windows 上没有 unix 权限位，靠用户目录的 ACL 兜着。
fn write_private(path: &str, body: &str) -> std::io::Result<()> {
    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(p)?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(p)?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
        Ok(())
    }
}

/// 只要一个能看懂的日期戳。为这一个字符串引入 chrono 不值当。
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 粗略换算成 UTC 日期（不处理闰秒，够用）
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant 的 days→(y,m,d) 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
