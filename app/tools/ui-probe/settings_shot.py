"""设置窗口的视觉自检：七组各拍一张。

和 shot.py 的关系：那个拍悬浮窗（302×242 的那块），这个拍设置窗口（980×720）。
一起用才好对：改了「强调色」，得同时看设置里那个圆点和悬浮窗主数字是不是一个色。

它验的是版式：分组切换对不对、控件有没有换行挤在一起、长路径会不会把行撑破、
「待接入」的行是不是明显灰掉、实时折算那四行数字有没有对齐。

它**不验**：Rust 侧的合并与校验（那是 `cargo test` + `settings_win::tests` 的事）。

用法：
    python settings_shot.py                 # 七组各一张
    python settings_shot.py pay look        # 只拍这两组
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
UI = HERE.parent.parent / "ui"

GROUPS = ["pay", "schedule", "look", "behavior", "notify", "privacy", "about"]

# 逻辑尺寸。窗口在真机上是 980×720，截图按同一尺寸排，比例才对得上。
W, H = 980, 720

EDGE_CANDIDATES = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
]


def pin() -> str:
    """钉死视口。

    Edge 的 `--window-size` 在有些机器上被策略覆盖（视口恒为 496），
    不钉的话整个设置窗口会按 496 宽排版 —— 两栏挤成一栏，看着像布局塌了。
    同时关过渡：无头模式的虚拟时间跑不满 150ms，截出来是半截动画。
    """
    return f"""<style>
  html, body {{ width: {W}px !important; height: {H}px !important;
               overflow: hidden !important; background: #0B0E13 !important; }}
  .app {{ height: {H}px !important; }}
  *, *::before, *::after {{ transition: none !important; animation: none !important; }}
</style>
"""


def find_browser():
    for p in EDGE_CANDIDATES:
        if os.path.exists(p):
            return p
    raise SystemExit("找不到 Edge / Chrome，无法截图")


def build_probe(tmp: Path):
    for name in ("settings.html", "settings.css", "settings.js"):
        shutil.copy(UI / name, tmp / name)
    shutil.copy(HERE / "settings-stub.js", tmp / "settings-stub.js")

    html = (tmp / "settings.html").read_text(encoding="utf-8")
    html = html.replace(
        '<link rel="stylesheet" href="settings.css" />',
        '<link rel="stylesheet" href="settings.css" />\n' + pin(),
    )
    html = html.replace(
        '<script src="settings.js"></script>',
        '<script src="settings-stub.js"></script>\n<script src="settings.js"></script>',
    )
    (tmp / "settings.html").write_text(html, encoding="utf-8")


def shoot(browser, page: Path, query: str, out: Path):
    subprocess.run(
        [
            browser,
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            f"--window-size={W},{H}",
            "--virtual-time-budget=2500",
            f"--screenshot={out}",
            f"{page.as_uri()}?{query}",
        ],
        check=False,
        capture_output=True,
    )


def crop(png: Path):
    try:
        from PIL import Image
    except ImportError:
        return
    im = Image.open(png)
    # 只裁掉超出的部分，绝不用超过原图的框——PIL 会用黑边补齐，
    # 于是「视口没到位」看起来就像「右边有一块黑」，反而把真问题藏住了。
    if im.height > H or im.width > W:
        im.crop((0, 0, min(im.width, W), min(im.height, H))).save(png)


def main():
    groups = sys.argv[1:] or GROUPS
    for g in groups:
        if g not in GROUPS:
            raise SystemExit(f"不认识的分组：{g}")
    browser = find_browser()
    out_dir = HERE / "out"
    out_dir.mkdir(exist_ok=True)

    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        build_probe(tmp)
        page = tmp / "settings.html"

        cases = [(g, f"g={g}", f"set-{g}.png") for g in groups]
        # 同一组里的两种关键状态，画出来才知道条件渲染有没有接上
        if "pay" in groups:
            cases.append(("pay/ot", "g=pay&ot=paid&leave=1", "set-pay-alt.png"))
            # 首启那一次的样子：一边是引导条，一边是默认的 8000 —— 就是要看它们挤不挤
            cases.append(("pay/首启", "g=pay&welcome=1", "set-pay-welcome.png"))
        if "look" in groups:
            cases.append(("look", "g=look&size=expanded&thru=1", "set-look-alt.png"))

        for label, q, name in cases:
            png = out_dir / name
            shoot(browser, page, q, png)
            crop(png)
            print(f"{label:10s} -> {name}")


if __name__ == "__main__":
    main()
