//! 生成「检查更新」的 Ed25519 密钥对。
//!
//! 用法（在发版机上跑一次）：
//!
//! ```bash
//! cd app/tools/gen-key && cargo run --release
//! ```
//!
//! 输出三样：
//! - **公钥 hex**：32 字节，填进 `app/src-tauri/src/update.rs` 的
//!   `UPDATE_PUBKEY_HEX` 常量。
//! - **私钥 hex**：32 字节种子，**离线保管**（写进密码管理器 / 纸面备份），
//!   绝不出现在仓库、CI、日志里。
//!
//! 私钥泄露 = 能给所有用户推任意程序，所以它只在这台发版机上存在。
//!
//! 签名原文档（和 update.rs 里 `verify_manifest_signature` 对齐）：
//! `{version}\n{platform}\n{arch}\n{sha256}`，用换行拼接，Ed25519 签名。

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

fn main() {
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    let pub_hex = hex::encode(verifying_key.as_bytes());
    let priv_hex = hex::encode(signing_key.to_bytes());

    println!("════════════════════════════════════════════════════════════");
    println!("  Ed25519 密钥对已生成");
    println!("════════════════════════════════════════════════════════════");
    println!();
    println!("【公钥】（填进 update.rs 的 UPDATE_PUBKEY_HEX）");
    println!("{}", pub_hex);
    println!();
    println!("【私钥】（离线保管，绝不进仓库/CI）");
    println!("{}", priv_hex);
    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  警告：私钥一旦泄露，攻击者可给所有用户推送任意程序。");
    println!("════════════════════════════════════════════════════════════");
}
