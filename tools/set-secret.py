#!/usr/bin/env python3
"""把文件内容写进 GitHub Actions secret（libsodium sealed box）。

用法：
    python tools/set-secret.py <SECRET_NAME> <value_file>

PAT 来源优先级：
    1. 环境变量 GITHUB_TOKEN
    2. ~/.paypulse-pat 文件（推荐：token 不落命令行、不进 shell history）

为什么需要它：`PAYPULSE_CORE_PAT` 一旦失效，release.yml 的 checkout 会在第一步
就挂掉，报错长得像网络问题（could not read Username for 'https://github.com'），
实际是凭据没了。轮换 PAT 后必须用它把新值写回去。
"""
import base64
import json
import os
import sys
import urllib.request

from nacl.public import PublicKey, SealedBox

OWNER = "zoroli1988"
REPO = "paypulse"
API = "https://api.github.com"
TOKEN_FILE = os.path.expanduser("~/.paypulse-pat")


def read_token() -> str:
    env = os.environ.get("GITHUB_TOKEN", "").strip()
    if env:
        return env
    if os.path.exists(TOKEN_FILE):
        with open(TOKEN_FILE, "r", encoding="utf-8") as fh:
            return fh.read().strip()
    raise SystemExit("没有可用的 PAT：设置 GITHUB_TOKEN，或把 token 存进 ~/.paypulse-pat")


def call(method: str, url: str, tok: str, payload=None):
    data = None
    headers = {
        "Authorization": f"Bearer {tok}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if payload is not None:
        data = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.status, resp.read()


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    name, value_file = sys.argv[1], sys.argv[2]
    with open(value_file, "r", encoding="utf-8") as fh:
        value = fh.read().strip()
    if not value:
        print("值为空，放弃")
        return 1

    tok = read_token()
    status, body = call("GET", f"{API}/repos/{OWNER}/{REPO}/actions/secrets/public-key", tok)
    if status != 200:
        print(f"取公钥失败 HTTP {status}")
        return 1
    key = json.loads(body)
    sealed = SealedBox(PublicKey(base64.b64decode(key["key"])))
    enc = base64.b64encode(sealed.encrypt(value.encode())).decode()

    status, _ = call(
        "PUT",
        f"{API}/repos/{OWNER}/{REPO}/actions/secrets/{name}",
        tok,
        {"encrypted_value": enc, "key_id": key["key_id"]},
    )
    print(f"{name} -> HTTP {status}（201/204 表示写入成功）")
    return 0 if status in (201, 204) else 1


if __name__ == "__main__":
    sys.exit(main())
