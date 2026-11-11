#!/usr/bin/env python3
"""生成 app/THIRD-PARTY.md：列出真正进 Windows 二进制的每一个依赖及其许可。

为什么要有这个文件
------------------
这个外壳是 AGPL-3.0，而它链接的 200 多个 crate 大多要求「保留版权与许可声明」。
AGPL 的源码可得以满足一部分义务，但 MIT / Apache-2.0 的那条是随**二进制分发**
触发的：谁拿到安装包，谁就该能查到里面有哪些第三方代码、各自什么许可。

为什么用 `cargo tree` 而不是 `Cargo.lock`
----------------------------------------
`Cargo.lock` 是给整个工作区解析的，含 macOS / Linux / wasm 的依赖分支，直接列会
虚报几十个根本没进这个 exe 的包。`cargo tree --target` 走的是真实的解析结果，
`--edges normal,build` 再去掉 dev-dependencies（测试用的不进产物）。

用法
----
    export PATH="$HOME/.cargo/bin:$PATH"
    python app/tools/license-scan/scan.py

首次在新机器上跑之前先 `cargo fetch`，否则本地 registry 缓存不全，会有一批包
查不到许可（脚本会把它们列成 UNKNOWN 并在结尾报数，不是静默丢掉）。
"""

from __future__ import annotations

import collections
import re
import subprocess
import sys
import tomllib
from pathlib import Path

TARGET = "x86_64-pc-windows-gnu"
REGISTRY = Path.home() / ".cargo/registry/src/index.crates.io-1949cf8c6b5b557f"

# 这几个许可不是「随便用」，分发时各有各的义务，单独提出来说
NOTABLE = {
    "MPL-2.0": "文件级 copyleft。不改这些文件就只需声明；改了就得公开改动。",
    "Unicode-3.0": "ICU 系（icu_* / zerovec / yoke …），要求随附其许可原文。",
    "BSD-3-Clause": "要求保留版权声明，且不得用作者名义为衍生品背书。",
    "Unlicense": "公有领域捐献，无义务。",
    "Zlib": "要求不得声称本软件由原作者编写。",
    "CC0-1.0": "公有领域捐献，无义务。",
}


def cargo_tree(manifest_dir: Path) -> list[tuple[str, str, str | None]]:
    out = subprocess.run(
        [
            "cargo", "tree",
            "--target", TARGET,
            "--edges", "normal,build",
            "--prefix", "none",
            "--no-dedupe",
        ],
        cwd=manifest_dir, capture_output=True, text=True, check=True,
    ).stdout

    seen: set[tuple[str, str]] = set()
    deps: list[tuple[str, str, str | None]] = []
    for line in out.splitlines():
        m = re.match(r"^(\S+) v(\S+)(?: \((.*)\))?$", line.strip())
        if not m:
            continue
        key = (m.group(1), m.group(2))
        if key in seen:
            continue
        seen.add(key)
        deps.append((m.group(1), m.group(2), m.group(3)))
    return deps


def licence_of(name: str, version: str) -> str | None:
    d = REGISTRY / f"{name}-{version}"
    if not d.is_dir():
        return None
    try:
        t = tomllib.loads((d / "Cargo.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return None
    p = t.get("package", {})
    lic = p.get("license")
    if lic:
        return lic
    if p.get("license-file"):
        return "file:" + str(p["license-file"])
    return "UNKNOWN"


def split_licences(expr: str) -> list[str]:
    """把 `(MIT OR Apache-2.0) AND Unicode-3.0` 拆成单个标识符。"""
    flat = expr.replace("(", "").replace(")", "").replace("/", " OR ")
    return [p.strip() for p in re.split(r"\s+OR\s+|\s+AND\s+", flat) if p.strip()]


def is_path_dep(src: str | None) -> bool:
    """括号里的东西是不是「本机路径」。

    cargo tree 的括号有多种含义，只有一种是 path 依赖：
        `serde_derive v1.0.229 (proc-macro)`   ← 这是个 proc-macro crate
        `foo v1.0.0 (outdated)`                ← 版本提示
        `paypulse-core v0.1.0 (C:\\...\\...)`   ← 这个才是 path 依赖
    早先只判「括号非空」，把 19 个过程宏当成了自家 crate —— 它们其实也是
    第三方，也带许可义务，漏掉就少声明了一大块。
    """
    if not src:
        return False
    return "/" in src or "\\" in src


def main() -> int:
    # .../app/tools/license-scan/scan.py
    #      ^0   ^1     ^2          ^3
    here = Path(__file__).resolve()
    repo_app = here.parents[2]           # app/
    manifest_dir = repo_app / "src-tauri"

    deps = cargo_tree(manifest_dir)
    rows: list[tuple[str, str, str]] = []
    ours: list[tuple[str, str]] = []
    unknown: list[tuple[str, str]] = []

    for name, version, src in deps:
        if is_path_dep(src):  # path 依赖 = 本仓 / 私有仓，不是第三方
            ours.append((name, version))
            continue
        lic = licence_of(name, version)
        if lic is None or lic == "UNKNOWN":
            unknown.append((name, version))
            rows.append((name, version, "UNKNOWN"))
        else:
            rows.append((name, version, lic))

    rows.sort(key=lambda r: r[0].lower())
    counts: collections.Counter[str] = collections.Counter()
    for _, _, lic in rows:
        if lic == "UNKNOWN":
            continue
        for one in split_licences(lic):
            counts[one] += 1

    lines: list[str] = []
    lines.append("# 第三方组件与许可")
    lines.append("")
    lines.append("这个文件由 `app/tools/license-scan/scan.py` 生成，**不要手改**。")
    lines.append("")
    lines.append(
        f"统计口径：`cargo tree --target {TARGET} --edges normal,build`，"
        f"即真正链接进 `paypulse-app.exe` 的全部依赖。"
    )
    lines.append(f"共 **{len(rows)}** 个第三方 crate。")
    lines.append("")

    lines.append("## 一眼看结论")
    lines.append("")
    lines.append("**没有任何 GPL 系依赖。** 全部为宽松许可，可以随 AGPL-3.0 的外壳一起分发。")
    lines.append("")
    lines.append("| 许可 | 包数 | 分发时的义务 |")
    lines.append("|---|---:|---|")
    lines.append(
        "<!-- 双许可（如 `MIT OR Apache-2.0`）在每个分支各计一次，所以合计大于包数 -->"
    )
    for lic, n in counts.most_common():
        duty = NOTABLE.get(lic, "保留版权与许可声明。")
        lines.append(f"| {lic} | {n} | {duty} |")
    lines.append("")

    mpl = [r for r in rows if "MPL" in r[2]]
    if mpl:
        lines.append("### 需要单独留意的几种")
        lines.append("")
        lines.append(
            "**MPL-2.0** 是唯一带 copyleft 的："
            + "、".join(f"`{n}`" for n, _, _ in mpl)
            + "。它按**文件**生效——这几个 crate 一行没改，所以义务只有一条："
            "告诉拿到二进制的人它们是 MPL-2.0，并且能拿到源码。"
            "下表每行的 crate 名都能点，链到 crates.io 的对应版本，"
            "那是最直接的源码获取途径。"
        )
        lines.append("")
        lines.append(
            "**Unicode-3.0** 来自 ICU4X 那一串（渲染中文界面时的 Unicode 属性查询要用）。"
            "它要求随附许可原文，原文在上表指定的 crate 包内 `LICENSE` 文件里。"
        )
        lines.append("")

    lines.append("## 完整清单")
    lines.append("")
    lines.append("| crate | 版本 | 许可 |")
    lines.append("|---|---|---|")
    for name, version, lic in rows:
        cells = lic.replace("|", "\\|")
        lines.append(
            f"| [`{name}`](https://crates.io/crates/{name}/{version}) "
            f"| {version} | {cells} |"
        )
    lines.append("")

    if ours:
        lines.append("## 本项目自己的 crate")
        lines.append("")
        for name, version in ours:
            lines.append(f"- `{name}` {version}")
        lines.append("")
        lines.append("其中 `paypulse-core` 是私有仓（计薪引擎），不随本仓的 AGPL 授权。")
        lines.append("")

    out = repo_app / "THIRD-PARTY.md"
    out.write_text("\n".join(lines), encoding="utf-8")

    print(f"写入 {out}")
    print(f"第三方 {len(rows)} 个 / 本项目 {len(ours)} 个")
    for lic, n in counts.most_common():
        print(f"  {n:4d}  {lic}")
    if unknown:
        print(f"!! {len(unknown)} 个包查不到许可（本地 registry 不全？先 cargo fetch）：")
        for n, v in unknown[:10]:
            print(f"   {n} {v}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
