"""抓 HUD 窗口自身的画面（DPI 感知 + PW_RENDERFULLCONTENT）。

和 shot.py 的分工：shot.py 在浏览器里画假数据，验布局；
这个抓的是**真进程真窗口**，验的是「Tauri + WebView2 真的把页面画出来了」。
第一次抓到全黑别慌——WebView 还没画完，等一两秒重抓。

用法：
    python window_shot.py out.png            # 抓窗口
    python window_shot.py out.png click 100 178   # 先点窗口内坐标再抓

注意：`click` 走 `SetCursorPos` + `mouse_event`，需要交互式桌面。
在远程/无人值守会话里会静默失败（返回 0），只截不点。
"""

import ctypes
import ctypes.wintypes as w
import sys
import time

from PIL import Image

ctypes.windll.shcore.SetProcessDpiAwareness(2)
u = ctypes.windll.user32
g = ctypes.windll.gdi32


def find_hud(must="薪"):
    hits = []
    CB = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def cb(h, _):
        if not u.IsWindowVisible(h):
            return True
        n = u.GetWindowTextLengthW(h)
        if n == 0:
            return True
        b = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(h, b, n + 1)
        if must not in b.value:
            return True
        # 只要够宽的：进程刚起来时可能还挂着一个没就位的默认窗口，
        # 抢到它就会拍到一张 303 高的空白。
        # 阈值不能再高了：紧凑档是 236 逻辑像素，125% 缩放下只有 295 物理像素，
        # 按 300 卡会正好把它筛掉。
        r = w.RECT()
        u.GetWindowRect(h, ctypes.byref(r))
        if r.right - r.left >= 200:
            hits.append(h)
        return True

    u.EnumWindows(CB(cb), 0)
    if not hits:
        raise SystemExit("没找到 HUD 窗口，程序启动了吗？")
    return hits[0]


def rect(h):
    r = w.RECT()
    u.GetWindowRect(h, ctypes.byref(r))
    return r.left, r.top, r.right - r.left, r.bottom - r.top


class BI(ctypes.Structure):
    _fields_ = [
        ("biSize", ctypes.c_uint32),
        ("biWidth", ctypes.c_int32),
        ("biHeight", ctypes.c_int32),
        ("biPlanes", ctypes.c_uint16),
        ("biBitCount", ctypes.c_uint16),
        ("biCompression", ctypes.c_uint32),
        ("biSizeImage", ctypes.c_uint32),
        ("biXPelsPerMeter", ctypes.c_int32),
        ("biYPelsPerMeter", ctypes.c_int32),
        ("biClrUsed", ctypes.c_uint32),
        ("biClrImportant", ctypes.c_uint32),
    ]


def shot(h, path):
    x, y, W, H = rect(h)
    hdc = u.GetWindowDC(h)
    mdc = g.CreateCompatibleDC(hdc)
    bmp = g.CreateCompatibleBitmap(hdc, W, H)
    g.SelectObject(mdc, bmp)
    u.PrintWindow(h, mdc, 2)
    bi = BI(ctypes.sizeof(BI), W, -H, 1, 32, 0, 0, 0, 0, 0, 0)
    buf = ctypes.create_string_buffer(W * H * 4)
    g.GetDIBits(mdc, bmp, 0, H, buf, ctypes.byref(bi), 0)
    Image.frombuffer("RGBA", (W, H), buf, "raw", "BGRA", 0, 1).convert("RGB").save(path)
    g.DeleteObject(bmp)
    g.DeleteDC(mdc)
    u.ReleaseDC(h, hdc)
    return x, y, W, H


def click(h, cx, cy):
    x, y, _, _ = rect(h)
    u.SetCursorPos(x + cx, y + cy)
    time.sleep(0.08)
    u.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
    time.sleep(0.05)
    u.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP


if __name__ == "__main__":
    path = sys.argv[1]
    # 设置窗口的标题里也有「薪」，所以抓设置时必须再加一个字区分开，
    # 否则 EnumWindows 的顺序一变就抓到悬浮窗去了。
    must = "设置" if "settings" in path or "set-" in path else "薪"
    h = find_hud(must)
    if len(sys.argv) >= 4 and sys.argv[2] == "click":
        click(h, int(sys.argv[3]), int(sys.argv[4]))
        time.sleep(0.6)
    elif len(sys.argv) >= 4 and sys.argv[2] == "move":
        x, y, _, _ = rect(h)
        u.SetCursorPos(x + int(sys.argv[3]), y + int(sys.argv[4]))
        time.sleep(0.4)
    print(shot(h, path))
