"""HUD 视觉自检：把 ui/ 里的真页面放到浏览器里，逐个状态截图。

为什么需要它：`paypulse-app` 是真窗口，但三个非默认态（加班 / 白干 / 休息）
要么要点按钮、要么得等到周末才看得到。在 CI 或远程桌面上既点不了鼠标，
也等不到周六。这个脚本用一份假后端（stub.js）喂数据，把每种状态都画出来。

它验的是：布局有没有溢出、文字有没有被挤、按钮该不该出现、
午休时右槽是不是「午休剩余」、义务加班时是不是「已白干」、
隐藏金额时数字是不是糊掉了、白干条出现后卡片有没有被撑高、
以及三档尺寸下该出现的东西在不在、该消失的东西有没有消失。

它**不验**：Rust 侧算得对不对（那是 `cargo test` 的事）。

用法：
    python shot.py                      # 九态（标准档）+ 六个尺寸组合
    python shot.py unpaid unpaid_break  # 只截指定状态
    python shot.py on@compact on@expanded   # 指定尺寸档
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
UI = HERE.parent.parent / "ui"

STATES = ["on", "break", "done", "ot", "unpaid", "unpaid_break", "paused", "masked", "rest", "leave"]

# 三档宽度（原型 01-C）。紧档 236 / 标准 302 / 展开 322。
SIZES = {"compact": 236, "normal": 302, "expanded": 322}

# 除了「九态 × 标准档」，再补这几张看三档版式：
# 紧凑态要砍掉指标行/页脚/提示/动作行，展开态要多出等价物块，
# 白干态是最高的一档（白干条 + 动作行叠在一起）。
EXTRA = [
    ("leave", "compact"),      # 紧凑态下三列换成 类型/剩余/本月 会不会挤
    ("leave", "expanded"),
    ("on", "compact"),
    ("break", "compact"),      # 紧凑态下「我在加班」必须被藏起来
    ("unpaid", "compact"),     # 白干条也要藏
    ("on", "expanded"),
    ("unpaid", "expanded"),    # 最高的一档
    ("rest", "compact"),       # 遮罩在 236 宽下是否仍然盖满
]

EDGE_CANDIDATES = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
]


def pin(width: int) -> str:
    """把布局宽度钉成该档的真实逻辑像素。

    Edge 的 --window-size 在某些机器上被策略覆盖（视口恒为 496），
    不钉死就会按 496 排版，看起来像内容溢出，其实是假象。
    宽度必须**按档**给：body 窄于 .hud 会把展开档的右边切掉。

    同时关掉所有过渡动画：无头模式的虚拟时间跑不满 150ms 的 transition，
    截出来就是半截 —— 按钮一半绿一半灰、进度条 75% 配上「进度 100%」，
    看着像 bug，其实只是拍早了。
    """
    return f"""<style>
  html, body {{ width: {width}px !important; height: auto !important;
               overflow: visible !important; background: #3A3F46 !important; }}
  *, *::before, *::after {{ transition: none !important; animation: none !important; }}
</style>
"""


def find_browser():
    for p in EDGE_CANDIDATES:
        if os.path.exists(p):
            return p
    raise SystemExit("找不到 Edge / Chrome，无法截图")


def build_probe(tmp: Path, width: int):
    """把 ui/ 复制一份出来并注入假后端，不改动仓库里的源文件。"""
    for name in ("index.html", "hud.css", "hud.js"):
        shutil.copy(UI / name, tmp / name)
    shutil.copy(HERE / "stub.js", tmp / "stub.js")

    html = (tmp / "index.html").read_text(encoding="utf-8")
    html = html.replace(
        '<link rel="stylesheet" href="hud.css" />',
        '<link rel="stylesheet" href="hud.css" />\n' + pin(width),
    )
    html = html.replace(
        '<script src="hud.js"></script>',
        '<script src="stub.js"></script>\n  <script src="hud.js"></script>',
    )
    (tmp / "index.html").write_text(html, encoding="utf-8")


def card_height(png: Path) -> int:
    """量卡片实际占了多少像素。

    不读 `resize_hud` 回调上来的高度：无头模式下 `requestAnimationFrame`
    跑不满，白干行出现后的第二次回调经常会掉，量出来永远是第一帧。
    直接看画出来的像素反而准，而且验的是"用户真的看到什么"。

    注意：这个数**包含 CSS 投影**，所以比真实窗口高（在岗态约 186，窗口是 165）。
    它只用来横向对比——白干态比在岗态多出 42px，说明白干行确实把窗口撑高了。
    """
    try:
        from PIL import Image
    except ImportError:
        return -1

    im = Image.open(png).convert("RGB")
    bg = (0x3A, 0x3F, 0x46)  # 探针页的背景色
    h = 0
    for y in range(im.height):
        row = [im.getpixel((x, y)) for x in range(0, im.width, 4)]
        if any(sum(abs(a - b) for a, b in zip(p, bg)) > 24 for p in row):
            h = y + 1
    return h


def shoot(browser, page: Path, state: str, size: str, out: Path):
    subprocess.run(
        [
            browser,
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--virtual-time-budget=2000",
            f"--screenshot={out}",
            f"{page.as_uri()}?s={state}&z={size}",
        ],
        check=False,
        capture_output=True,
    )


def parse_cases(args):
    """`on` 表示标准档，`on@compact` 指定档位。"""
    cases = []
    for a in args:
        s, _, z = a.partition("@")
        z = z or "normal"
        if s in STATES and z in SIZES:
            cases.append((s, z))
        else:
            raise SystemExit(f"不认识的状态或档位：{a}")
    if cases:
        return cases
    return [(s, "normal") for s in STATES] + EXTRA


def main():
    cases = parse_cases(sys.argv[1:])
    browser = find_browser()
    out_dir = HERE / "out"
    out_dir.mkdir(exist_ok=True)

    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        pages = {}
        for _, z in cases:
            if z in pages:
                continue
            d = tmp / z
            d.mkdir()
            build_probe(d, SIZES[z])
            pages[z] = d / "index.html"

        for s, z in cases:
            name = f"{s}.png" if z == "normal" else f"{s}@{z}.png"
            png = out_dir / name
            shoot(browser, pages[z], s, z, png)
            w = SIZES[z]
            try:
                from PIL import Image

                im = Image.open(png)
                im.crop((0, 0, w, min(im.height, 560))).save(png)
            except ImportError:
                pass
            print(f"{s:7s} @{z:8s} 卡片高 {card_height(png):3d}px  宽 {w}  -> {name}")


if __name__ == "__main__":
    main()
