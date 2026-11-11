"""生成托盘图标（剪影版）。

为什么托盘要另一套图，而不是复用 `icon.png`：

主图标是「圆角方块 + 描边牛头 + 鼻环」，在 128px 下很好看。
但系统托盘在 100% 缩放下只有 **16×16 像素**——那点尺寸里，
鼻环占 1px、鼻孔占 1px、方块圆角吃掉 2px，最后得到一团灰糊。

所以托盘版只保留 **角 + 头** 两个形状，且靠两层色块保证可辨识：

    深色描边（膨胀出来的）+ 品牌绿填充

单色剪影在亮色任务栏上会消失，在暗色任务栏上的深色剪影也会消失；
两层色块则在两种主题下都能被看见——这是托盘图标的通行做法，
不是审美选择。

用法：
    python gen_tray.py

输出：
    ../../src-tauri/icons/tray.png       32×32  给应用用
    ../../src-tauri/icons/tray-16.png    16×16  只用于人眼检查小尺寸效果
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
OUT = HERE.parent.parent / "src-tauri" / "icons"

# 8× 超采样：在 256 的画布上画，最后降到 32 / 16。
# 直接在小画布上画圆角和斜线会得到锯齿，降采样能自然抗锯齿。
SS = 8
BASE = 32  # 设计尺寸（以 32px 为准，1px 描边 = 1 个设计像素）
S = BASE * SS

DARK = (17, 21, 27, 255)     # 描边
# 填充用品牌绿，而不是白色。
# 第一版填的是 #F4F7FA，在亮色任务栏（#F0F2F5 附近）上跟背景几乎同色，
# 只剩一圈细描边，16px 下直接被看成「空心细线」，反而更糊。
# 绿色在深浅两种任务栏上都有对比度，也和应用主色一致。
FILL = (34, 197, 94, 255)


def px(v: float) -> float:
    """设计像素 → 超采样画布坐标。"""
    return v * SS


def silhouette() -> Image.Image:
    """画剪影蒙版（L 模式，255 = 在形状内）。

    形状只用两个：梯形头和两只角。耳朵、鼻孔、鼻环全砍掉——
    16px 下它们加起来不到 4 个像素，只会让轮廓变脏。
    """
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)

    # 头：上半是圆角矩形，下半收窄成下巴。分两笔拼，接缝处被同一颜色填满。
    d.rounded_rectangle(
        [px(9.0), px(12.0), px(23.0), px(24.0)], radius=px(5.0), fill=255
    )
    d.polygon(
        [
            (px(11.0), px(21.0)),
            (px(21.0), px(21.0)),
            (px(19.2), px(28.2)),
            (px(12.8), px(28.2)),
        ],
        fill=255,
    )

    # 角：从头顶两侧往外上翘。做锥形（根部粗、尖端细），
    # 等宽直线在 16px 下会缩成两个点，锥形才认得出「这是角」。
    d.polygon(
        [
            (px(9.6), px(14.4)),
            (px(12.4), px(14.0)),
            (px(7.6), px(3.6)),
            (px(5.6), px(5.2)),
        ],
        fill=255,
    )
    d.polygon(
        [
            (px(19.6), px(14.0)),
            (px(22.4), px(14.4)),
            (px(26.4), px(5.2)),
            (px(24.4), px(3.6)),
        ],
        fill=255,
    )
    return m


def compose(size: int) -> Image.Image:
    mask = silhouette()

    # 描边：把剪影蒙版向外膨胀 1 个设计像素（= SS 个超采样像素）。
    # 用膨胀而不是「再画一遍更大的形状」，是因为形状是拼出来的，
    # 手工算外轮廓会在接缝处露馅。
    r = SS  # 1 设计像素
    dilated = mask.filter(ImageFilter.MaxFilter(r * 2 + 1))

    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.paste(Image.new("RGBA", (S, S), DARK), (0, 0), dilated)
    out.paste(Image.new("RGBA", (S, S), FILL), (0, 0), mask)

    return out.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    tray = compose(32)
    tray.save(OUT / "tray.png")

    small = compose(16)
    small.save(OUT / "tray-16.png")

    # 放大预览图：把 16px 按最近邻放大 8 倍，方便人眼判断「糊没糊」。
    # 用 NEAREST 而不是 LANCZOS——要先看到像素本身，而不是被插值美化的样子。
    small.resize((128, 128), Image.NEAREST).save(OUT / "tray-16-preview.png")

    for name in ("tray.png", "tray-16.png", "tray-16-preview.png"):
        p = OUT / name
        print(f"{name:22s} {p.stat().st_size:>6d} B")


if __name__ == "__main__":
    main()
