# 发版流程（检查更新 + Releases）

「检查更新」走的是**纯静态**方案：一份签好名的 `manifest.json` + 一堆安装包，
都放在 GitHub Releases 上，没有跑应用的服务器。

## 一、密钥对（一次性）

发版前先在一台**安全机器**（不联网最好）上生成 Ed25519 密钥对：

```bash
cd app/tools/gen-key && cargo run --release
```

会输出：
- **公钥 hex**（32 字节）→ 填进 `app/src-tauri/src/update.rs` 的 `UPDATE_PUBKEY_HEX`
- **私钥 hex**（32 字节种子）→ **离线保管**，绝不进仓库、CI、日志

> ⚠️ 私钥泄露 = 能给所有用户推任意程序。把它存进密码管理器或纸面备份，
> 只在这台发版机上用。

## 二、配 secret（GitHub 仓库 Settings → Secrets）

| Secret 名 | 内容 | 用途 |
|---|---|---|
| `PAYPULSE_CORE_PAT` | 有私有仓 `paypulse-core` 读权限的 personal access token | CI 拉私有引擎 submodule |
| `UPDATE_SIGNING_KEY` | 上面生成的私钥 hex | release 时给安装包签名 |

## 三、发一个版本

1. 改 `app/src-tauri/Cargo.toml` 的 `version`（和 `app/package.json` 保持一致）
2. 打 tag 并 push：

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. GitHub Actions 自动跑 `.github/workflows/release.yml`：
   - Windows / macOS 编译安装包
   - 用 `UPDATE_SIGNING_KEY` 给每个包算 sha256 + Ed25519 签名
   - 生成 `manifest.json`，随安装包一起上传到 Releases

## 四、客户端怎么检查

客户端「检查更新」时：
1. 前端 `fetch` 拉 `manifest.json`（GitHub Releases 的 raw 地址）
2. 传给 Rust 的 `check_update` 命令
3. Rust 用硬编码的 `UPDATE_PUBKEY_HEX` 验签 + 语义化版本比较
4. 有新版 → 提示，下载走系统默认浏览器

## 五、轮换公钥（私钥泄露时）

换公钥 = 老客户端再也验不过新清单。流程：

1. 重新生成密钥对，公钥填进 `update.rs`
2. 发一个**带新公钥的强制更新版本**（`force_below` 填当前所有旧版本号）
3. 旧版本收到强制更新后，装上带新公钥的版本，才能继续收后续更新

## 六、manifest.json 契约

签名原文（`update.rs` 的 `verify_manifest_signature` 对齐）：

```
{version}\n{platform}\n{arch}\n{sha256}
```

用换行拼接，Ed25519 签名。platform 取值 `windows` / `darwin` / `linux`，
arch 取值 `x64` / `arm64`。
