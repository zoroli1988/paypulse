#!/usr/bin/env bash
# =============================================================================
# 一键建仓 + 推送 + 发版脚本（牛马薪动了！/ paypulse）
#
# 三种用法（子命令）：
#
#   bash publish.sh                    # 建仓 + 推送（首次/改代码后）
#   bash publish.sh setup-secrets      # 把 PAT + 签名私钥配进 paypulse 仓的 Secrets
#   bash publish.sh gen-key            # 生成正式发版用的 Ed25519 密钥对
#   bash publish.sh release v0.1.2     # 打 tag 触发 Actions 发版，并校验 manifest
#   bash publish.sh revoke-help        # 打印旧 PAT / 泄露私钥的 revoke 指引
#
# 建仓 + 推送（默认动作）：
#   1. 用 GitHub REST API 在 zoroli1988 名下创建两个仓库：
#        - paypulse        (public)   外壳 + 时钟库 + 原型（AGPL-3.0）
#        - paypulse-core   (private)  计薪引擎（闭源）
#   2. 先推私有 core 仓，再推公开外壳仓（外壳通过 submodule 引用 core）。
#
# 使用前准备（只做一次）：
#   1. 生成一个 Personal Access Token（PAT）：
#       https://github.com/settings/tokens
#     勾选权限：repo（全部）、workflow（如果需要 Actions 发版）
#   2. 把 token 存进环境变量（不要写进本脚本、不要提交进仓库）：
#       export GITHUB_TOKEN="ghp_xxxx..."
#
# 幂等：仓库已存在时跳过建仓、直接推送；可反复运行。
# =============================================================================

set -euo pipefail

# ---- 可配置项 ----
OWNER="${GITHUB_OWNER:-zoroli1988}"
TOKEN="${GITHUB_TOKEN:-}"

PUBLIC_REPO="paypulse"
PRIVATE_REPO="paypulse-core"

DEFAULT_BRANCH="master"

# 脚本所在目录。两种摆放方式都支持：
#   1. 放在发布仓 paypulse 根目录里（推荐）：PUBLIC_DIR = 脚本目录本身
#   2. 放在 release-split/ 下（子目录是 paypulse/ 与 paypulse-core/）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$SCRIPT_DIR/.gitmodules" && -d "$SCRIPT_DIR/app" ]]; then
  # 脚本就在 paypulse 仓根目录内
  PUBLIC_DIR="$SCRIPT_DIR"
  PRIVATE_DIR="$SCRIPT_DIR/$PRIVATE_REPO"
else
  # 脚本在 release-split/ 下
  PUBLIC_DIR="$SCRIPT_DIR/$PUBLIC_REPO"
  PRIVATE_DIR="$SCRIPT_DIR/$PRIVATE_REPO"
fi

# ---- 颜色 ----
C_RED=$'\033[31m'; C_GRN=$'\033[32m'; C_YEL=$'\033[33m'; C_CYN=$'\033[36m'; C_RST=$'\033[0m'
info()  { printf "%s[信息]%s %s\n" "$C_CYN" "$C_RST" "$*"; }
ok()    { printf "%s[完成]%s %s\n" "$C_GRN" "$C_RST" "$*"; }
warn()  { printf "%s[提示]%s %s\n" "$C_YEL" "$C_RST" "$*"; }
die()   { printf "%s[错误]%s %s\n" "$C_RED" "$C_RST" "$*" >&2; exit 1; }

# ---- 前置检查 ----
command -v curl >/dev/null 2>&1 || die "未找到 curl，请先安装。"
command -v git  >/dev/null 2>&1 || die "未找到 git，请先安装。"

# revoke-help / gen-key 不需要 token；其余子命令及默认 push 都需要。
CMD_PRE="${1:-push}"
case "$CMD_PRE" in
  revoke-help|gen-key) ;;
  *)
    if [[ -z "$TOKEN" ]]; then
      die "缺少 GITHUB_TOKEN 环境变量。\n  请先执行：export GITHUB_TOKEN=\"ghp_xxxx...\"\n  （PAT 生成入口：https://github.com/settings/tokens）"
    fi
    [[ -d "$PUBLIC_DIR/.git"  ]] || die "找不到公开仓目录：$PUBLIC_DIR"
    [[ -d "$PRIVATE_DIR/.git" ]] || die "找不到私有仓目录：$PRIVATE_DIR"
    ;;
esac

# ---- GitHub API 封装 ----
API="https://api.github.com"
AUTH_HEADER="Authorization: Bearer $TOKEN"

# 建仓；已存在则返回 0（幂等）。$1=repo名 $2=private(true/false)
create_repo() {
  local repo="$1" private="$2" code
  code=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$API/user/repos" \
    -H "$AUTH_HEADER" -H "Accept: application/vnd.github+json" \
    -d "{\"name\":\"$repo\",\"private\":$private,\"auto_init\":false}")
  case "$code" in
    201) ok "仓库 $OWNER/$repo 已创建";;
    422) warn "仓库 $OWNER/$repo 已存在，跳过创建";;
    401) die "Token 无效或过期（401）。请重新生成 PAT 并更新 GITHUB_TOKEN。";;
    *)   die "建仓失败，HTTP $code";;
  esac
}

# ---- 推送单个仓库 ----
# token 不落盘：remote URL 保持干净 HTTPS，push 时用临时 askpass 注入 token。
# 这样 .git/config 里永远不会出现明文 token（即使 token 泄露被撤销，本地也无残留）。
push_repo() {
  local dir="$1" repo="$2" clean_url token_helper
  clean_url="https://github.com/$OWNER/$repo.git"

  cd "$dir"
  if ! git remote get-url origin >/dev/null 2>&1; then
    git remote add origin "$clean_url"
    info "$repo: 已添加 remote origin（干净 URL）"
  else
    # 已有 remote：若含明文 token（历史遗留），统一清回干净 URL
    if git remote get-url origin | grep -q "x-access-token\|@github"; then
      git remote set-url origin "$clean_url"
      warn "$repo: 已把 remote URL 里的明文 token 清掉"
    fi
  fi

  # 用一次性 askpass 脚本把 token 传给 git，用完即删，不写进 config
  token_helper="$(mktemp)"
  printf '#!/bin/sh\necho "%s"\n' "$TOKEN" > "$token_helper"
  chmod +x "$token_helper"
  export GIT_ASKPASS="$token_helper"

  # 确保本地分支名与远端一致
  local br
  br="$(git rev-parse --abbrev-ref HEAD)"
  if [[ "$br" != "$DEFAULT_BRANCH" ]]; then
    warn "$repo: 当前分支是 $br，将推送到 origin/$br（非 $DEFAULT_BRANCH）"
  fi

  info "$repo: 推送到 origin $br ..."
  git push -u origin "$br"
  ok "$repo: 推送完成"

  # 清理一次性 askpass（token 不留盘）
  rm -f "$token_helper"
  unset GIT_ASKPASS
}

# ---- GitHub API 封装（续）：公共密钥 / secrets ----

# 获取仓库用于加密 secrets 的 public key。$1=repo名，返回 base64 key + key_id（以空格分隔）
get_repo_pubkey() {
  local repo="$1"
  curl -s "$API/repos/$OWNER/$repo/actions/secrets/public-key" \
    -H "$AUTH_HEADER" -H "Accept: application/vnd.github+json"
}

# 用 LibSodium sealed box 加密一个 secret 值，返回 base64 密文。
# 依赖 Python + PyNaCl；无 PyNaCl 时给出安装提示。
seal_secret() {
  local plain="$1" key_b64="$2"
  python3 - "$plain" "$key_b64" <<'PY'
import sys, base64
plain = sys.argv[1].encode()
key = base64.b64decode(sys.argv[2])
try:
    from nacl.public import PublicKey, SealedBox
except ImportError:
    print("NO_NACL", file=sys.stderr)
    sys.exit(3)
box = SealedBox(PublicKey(key))
print(base64.b64encode(box.encrypt(plain)).decode())
PY
}

# 写入一个 repo secret（幂等：已存在则覆盖）。$1=repo $2=name $3=plain value
put_secret() {
  local repo="$1" name="$2" value="$3" key_json key_id key_b64 enc code
  key_json="$(get_repo_pubkey "$repo")"
  key_id="$(printf '%s' "$key_json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["key_id"])' 2>/dev/null)"
  key_b64="$(printf '%s' "$key_json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["key"])' 2>/dev/null)"
  if [[ -z "$key_id" || -z "$key_b64" ]]; then
    die "无法获取 $repo 的 secrets public key（token 可能没有 admin 权限）。"
  fi
  enc="$(seal_secret "$value" "$key_b64")" || {
    warn "缺少 PyNaCl，无法加密 secret。请先：pip install pynacl"
    return 1
  }
  code=$(curl -s -o /dev/null -w "%{http_code}" \
    -X PUT "$API/repos/$OWNER/$repo/actions/secrets/$name" \
    -H "$AUTH_HEADER" -H "Accept: application/vnd.github+json" \
    -d "{\"encrypted_value\":\"$enc\",\"key_id\":\"$key_id\"}")
  case "$code" in
    201|204) ok "$repo: secret $name 已写入";;
    401) die "写入 $name 失败：token 无效（401）";;
    403) die "写入 $name 失败：token 无 admin 权限（403），需要 repo scope 的经典 PAT";;
    422) die "写入 $name 失败：值格式非法（422）";;
    *)   die "写入 $name 失败，HTTP $code";;
  esac
}

# ---- 子命令：setup-secrets ----
cmd_setup_secrets() {
  [[ -z "$TOKEN" ]] && die "缺少 GITHUB_TOKEN。"
  info "配置 $PUBLIC_REPO 仓的 Secrets ..."

  # 1) PAYPULSE_CORE_PAT：能读私有 core 仓的 PAT（这里直接复用当前 TOKEN，
  #    但它必须同时有 repo 权限；若用户想用独立只读 PAT，可覆盖环境变量）
  local core_pat="${PAYPULSE_CORE_PAT:-$TOKEN}"
  put_secret "$PUBLIC_REPO" "PAYPULSE_CORE_PAT" "$core_pat"

  # 2) UPDATE_SIGNING_KEY：正式签名私钥。绝不用测试私钥，且要求用户显式提供。
  local signing_key="${UPDATE_SIGNING_KEY:-}"
  if [[ -z "$signing_key" ]]; then
    warn "未提供 UPDATE_SIGNING_KEY（跳过签名密钥写入）。"
    warn "  先用 'bash publish.sh gen-key' 生成正式密钥，把私钥存进密码管理器，"
    warn "  再 export UPDATE_SIGNING_KEY='<私钥hex>' 后重跑本命令。"
    warn "  同时把公钥 hex 填进 app/src-tauri/src/update.rs 的 UPDATE_PUBKEY_HEX。"
  else
    put_secret "$PUBLIC_REPO" "UPDATE_SIGNING_KEY" "$signing_key"
  fi

  echo
  ok "Secrets 配置完成。"
  echo "  下一步：bash publish.sh release v0.1.2"
}

# ---- 子命令：gen-key ----
cmd_gen_key() {
  local gk_dir="$PUBLIC_DIR/app/tools/gen-key"
  [[ -d "$gk_dir" ]] || die "找不到 gen-key：$gk_dir"
  info "生成正式发版用的 Ed25519 密钥对 ..."
  ( cd "$gk_dir" && cargo run --release )
  echo
  warn "请把上面的私钥 hex 离线保管（密码管理器 / 纸面），绝不进仓库或 CI。"
  warn "然后把公钥 hex 填进 app/src-tauri/src/update.rs 的 UPDATE_PUBKEY_HEX，"
  warn "再 export UPDATE_SIGNING_KEY='<私钥hex>' 并跑 'bash publish.sh setup-secrets'。"
}

# ---- 子命令：release ----
cmd_release() {
  local tag="$1"
  [[ -z "$TOKEN" ]] && die "缺少 GITHUB_TOKEN。"
  [[ "$tag" == v* ]] || die "tag 必须以 v 开头，如 v0.1.2"

  info "在 $PUBLIC_REPO 打 tag $tag 触发发版 ..."
  cd "$PUBLIC_DIR"

  # 本地 tag 若已存在则跳过（幂等）；否则打 tag 并推送
  if git tag -l "$tag" | grep -q .; then
    warn "本地已存在 tag $tag，直接推送"
  else
    git tag "$tag"
    ok "本地已打 tag $tag"
  fi
  # 用一次性 askpass 注入 token（remote 是干净 URL，不落盘）
  local token_helper
  token_helper="$(mktemp)"
  printf '#!/bin/sh\necho "%s"\n' "$TOKEN" > "$token_helper"
  chmod +x "$token_helper"
  GIT_ASKPASS="$token_helper" git push origin "$tag" 2>&1 || {
    warn "git push tag 走 schannel 可能偶发失败（TLS 不稳）。"
    warn "改用 GitHub API 打 tag（Git Data API 稳定）："
    create_tag_via_api "$tag"
  }
  rm -f "$token_helper"

  echo
  ok "已触发发版。等待 Actions 完成后校验："
  echo "  bash publish.sh verify-release $tag"
}

# git push 失败时的兜底：用 Git Data API 打 annotated/lightweight tag
create_tag_via_api() {
  local tag="$1" target_sha
  cd "$PUBLIC_DIR"
  target_sha="$(git rev-parse HEAD)"
  # 若 tag 已在远端则跳过
  local existing
  existing="$(curl -s -o /dev/null -w "%{http_code}" "$API/repos/$OWNER/$PUBLIC_REPO/git/refs/tags/$tag")"
  if [[ "$existing" == "200" ]]; then
    warn "远端已存在 tag $tag，跳过"
    return 0
  fi
  local code
  code=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$API/repos/$OWNER/$PUBLIC_REPO/git/refs" \
    -H "$AUTH_HEADER" -H "Accept: application/vnd.github+json" \
    -d "{\"ref\":\"refs/tags/$tag\",\"sha\":\"$target_sha\"}")
  case "$code" in
    201) ok "已通过 API 打 tag $tag（lightweight）";;
    *)   die "API 打 tag 失败，HTTP $code";;
  esac
}

# ---- 子命令：verify-release ----
# 轮询 GitHub Actions 直到该 tag 的 Release 出来，并校验 manifest.json 指向正确。
cmd_verify_release() {
  local tag="$1" version="${tag#v}"
  [[ -z "$tag" ]] && die "用法：bash publish.sh verify-release v0.1.2"

  info "校验 $tag 的 Release（最多等 30 分钟，可 Ctrl-C 中断）..."
  local attempt=0 release_json=""
  while [[ $attempt -lt 90 ]]; do
    release_json="$(curl -s "$API/repos/$OWNER/$PUBLIC_REPO/releases/tags/$tag" -H "Accept: application/vnd.github+json")"
    if printf '%s' "$release_json" | python3 -c 'import sys,json; d=json.load(sys.stdin); print("tag_name" in d and not d.get("draft",False))' 2>/dev/null | grep -q True; then
      break
    fi
    attempt=$((attempt+1))
    sleep 20
  done

  if ! printf '%s' "$release_json" | python3 -c 'import sys,json; d=json.load(sys.stdin); print("tag_name" in d)' 2>/dev/null | grep -q True; then
    die "未等到 $tag 的 Release，请到 Actions 页查看是否失败：https://github.com/$OWNER/$PUBLIC_REPO/actions"
  fi

  # 校验 manifest.json
  local manifest_url="https://github.com/$OWNER/$PUBLIC_REPO/releases/download/$tag/manifest.json"
  local manifest
  manifest="$(curl -sL "$manifest_url")" || die "下载 manifest 失败"
  echo "$manifest" | python3 - "$version" <<'PY'
import sys, json
ver = sys.argv[1]
m = json.load(sys.stdin)
print("manifest 校验结果：")
print("  channel     =", m.get("channel"))
print("  latest      =", m.get("latest"), "(期望", ver + ")")
pkgs = m.get("packages", {})
for k, p in pkgs.items():
    print(f"  packages.{k}: url={p.get('url')}")
    print(f"                 sha256={p.get('sha256')[:16]}... sig={p.get('sig')[:16]}...")
if m.get("latest") != ver:
    print("!! 警告：latest 与 tag 不一致，可能下载到旧包")
else:
    print("OK：latest 与 tag 一致，manifest 指向正确版本")
PY
  ok "Release $tag 就绪，产物：$manifest_url"
}

# ---- 子命令：revoke-help ----
cmd_revoke_help() {
  cat <<'EOF'

【PAT / 私钥 revoke 与轮换指引】

若某个 PAT 或签名私钥 hex 曾在聊天/日志/仓库里出现过，必须立即作废并轮换：

1) 撤销旧 PAT（Personal Access Token）
   - 经典 PAT： https://github.com/settings/tokens
     找到对应 token → 点其右侧 "..." → "Revoke"（或直接 Delete）。
   - 细粒度 PAT： https://github.com/settings/tokens?type=beta
     找到对应 token → "Revoke"。
   撤销后，任何用旧 PAT 的脚本/CI 会立即失效，需换新 PAT 并 export GITHUB_TOKEN。

2) 轮换签名私钥（UPDATE_SIGNING_KEY）
   - 重新生成： bash publish.sh gen-key
   - 把新公钥 hex 写进 app/src-tauri/src/update.rs 的 UPDATE_PUBKEY_HEX，提交并随新版发版。
   - 用新私钥重跑： export UPDATE_SIGNING_KEY='<新私钥hex>' && bash publish.sh setup-secrets
   - 注意：已发出的旧版客户端只认旧公钥；轮换后旧版会拒绝新 manifest，
     所以要么立刻发新版本覆盖，要么在 manifest 里保留对旧公钥的兼容（本工程当前为单公钥，直接换发新版）。

3) 检查是否泄漏进了仓库/历史
   - 本地历史里搜 token/私钥： git log -p --all | grep -n '<token前几位>'
   - 若已推上远端且含敏感信息，用 BFG / filter-repo 清洗历史并 force push，
     但最稳妥是 revoke 旧凭据（token 作废后，历史里的旧值就没用了）。
   - 私钥无法 revoke，只能轮换 + 确认未进远端公开仓。

EOF
}

# ---- 主流程（默认动作：建仓 + 推送）----
CMD="${1:-push}"
case "$CMD" in
  setup-secrets)     cmd_setup_secrets; exit 0;;
  gen-key)           cmd_gen_key; exit 0;;
  release)           cmd_release "${2:-}"; exit 0;;
  verify-release)    cmd_verify_release "${2:-}"; exit 0;;
  revoke-help)       cmd_revoke_help; exit 0;;
  push)              ;;   # 继续下面的主流程
  *) die "未知子命令：$CMD（可选：push / setup-secrets / gen-key / release / verify-release / revoke-help）";;
esac

info "开始一键建仓 + 推送（owner=$OWNER）"

# 1. 私有引擎仓（先推，外壳的 submodule 依赖它）
info "==> 处理私有仓 $PRIVATE_REPO =="
create_repo "$PRIVATE_REPO" true
push_repo   "$PRIVATE_DIR" "$PRIVATE_REPO"

# 2. 公开外壳仓
info "==> 处理公开仓 $PUBLIC_REPO =="
create_repo "$PUBLIC_REPO" false
push_repo   "$PUBLIC_DIR" "$PUBLIC_REPO"

echo
ok "全部完成。"
echo "  - https://github.com/$OWNER/$PRIVATE_REPO   (private)"
echo "  - https://github.com/$OWNER/$PUBLIC_REPO    (public)"
echo
warn "后续步骤（均已脚本化）："
echo "  1. 配置 Secrets：      bash publish.sh setup-secrets"
echo "  2. 生成签名密钥：      bash publish.sh gen-key"
echo "  3. 打 tag 发版：       bash publish.sh release v0.1.2"
echo "  4. 校验发版产物：      bash publish.sh verify-release v0.1.2"
echo "  5. PAT/私钥安全指引：  bash publish.sh revoke-help"
