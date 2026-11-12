# 发版流程（检查更新 + Releases）

「检查更新」走的是**纯静态**方案：一份签好名的 `manifest.json` + 一堆安装包，
都放在 GitHub Releases 上，没有跑应用的服务器。

> 🚀 **一键脚本**：仓根目录的 `publish.sh` 把下面大部分步骤脚本化了：
>
> ```bash
> export GITHUB_TOKEN="ghp_xxxx"          # 你的 PAT（repo + workflow 权限）
> bash publish.sh setup-secrets           # 把 PAT + 签名私钥写进仓库 Secrets
> bash publish.sh gen-key                 # 生成正式 Ed25519 密钥对
> bash publish.sh release v0.1.2          # 打 tag 触发发版
> bash publish.sh verify-release v0.1.2   # 校验 Release 与 manifest
> bash publish.sh revoke-help             # PAT/私钥泄露时的 revoke 与轮换指引
> ```
>
> 脚本的 token 全程**不落盘**：remote 保持干净 HTTPS URL，push 时用一次性
> askpass 注入。`.git/config` 里永远不会出现明文 token。

## 一、密钥对（一次性）

发版前在一台**安全机器**上生成 Ed25519 密钥对：

```bash
bash publish.sh gen-key                       # 私钥默认写到 ~/.paypulse-signing-key-<日期>.txt
bash publish.sh check-key ~/.paypulse-signing-key-20261112.txt   # 核对与 update.rs 是否配对
```

终端只显示**公钥**和**指纹**，私钥直接写文件（Unix 权限 600）：

- **公钥 hex**（32 字节）→ 填进 `app/src-tauri/src/update.rs` 的 `UPDATE_PUBKEY_HEX`
- **私钥 hex**（32 字节种子）→ 导入密码管理器后**立刻删掉那个文件**

> ⚠️ 私钥泄露 = 能给所有用户推任意程序，而且**没法撤销、只能整对轮换**。
>
> 私钥**不要**打印到终端：终端回滚缓冲、截图、聊天记录都是泄露面。本工程
> 2026-11-12 就因为旧版 `gen-key` 直接 `println!` 私钥而轮换过一次，代价见第五节。
> 非要在终端生成（没有文件系统的环境）才用 `cargo run --release -- --stdout`，
> 生成完立刻 `clear`。

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

**换公钥之后，旧客户端收不到任何东西——包括强制更新。**

这一点容易想错：`update.rs` 的 `evaluate` 是「先验签，再比版本」，
验签不过就在第一步 `return`，`force_below` 那段代码根本执行不到。
所以「发一个强制更新版让旧客户端自己升上来」是走不通的，旧版本只会显示
「签名校验失败：清单可能被篡改」。

实际可用的路只有两条：

**A. 手动装一次（默认走这条）**
1. `bash publish.sh gen-key` → 新公钥填进 `update.rs` → `check-key` 核对
2. 用新私钥配 secret：`export UPDATE_SIGNING_KEY=... && bash publish.sh setup-secrets`
3. 正常发版；旧版本用户（含自用机）**手动下载装一次**，之后自动更新恢复
4. 装完之后旧版本就再也收不到新清单了，所以公告里要写清楚「这次必须手动装」

**B. 过渡期双公钥（要给一批旧客户端兜底时才值得）**
1. 客户端代码先改成「旧、新公钥任一验过即通过」`[旧, 新]`，用**旧私钥**签 manifest，发一版
2. 旧客户端用旧钥验过 → 正常自动更新到这一版，此时它手里已经有了新公钥
3. 再改成只认新公钥、用新私钥签，发下一版
4. 还没更新的那批会在第 2 步之后失效，所以窗口期内要盯着更新率

代价是过渡期里旧私钥仍然有效——而它已经泄露过，那条路等于把「谁能签 manifest」
交给了拿到私钥的人。**只有自用/小范围时才选 B，公开发布一律选 A。**

## 六、manifest.json 契约

签名原文（`update.rs` 的 `verify_manifest_signature` 对齐）：

```
{version}\n{platform}\n{arch}\n{sha256}
```

用换行拼接，Ed25519 签名。platform 取值 `windows` / `darwin` / `linux`，
arch 取值 `x64` / `arm64`。
