//! 一条不抢焦点的通知。
//!
//! ## 为什么是自己画一个窗口，而不是系统通知
//!
//! 系统通知要走 `tauri-plugin-notification`，而它在 Windows 上牵进来的是
//! 一整套 WinRT 绑定——为了弹两行字，多一个插件、多一堆编译时间，
//! 换来的好处只有「能进通知中心」这一条。
//!
//! 代价写在这里：**它不进 Windows 通知中心**，错过就没了；
//! 也不受「专注助手」管理。所以它只用来报「今天到账多少」这种
//! 看一眼就够、不需要回看的信息。真正的待办类提醒不该走这条路。
//!
//! ## 三条硬约束
//!
//! 1. **不抢焦点**：窗口建的时候 `focused(false)`，而且不激活——
//!    用户正在打字，弹个通知把光标抢走是最招人烦的。
//! 2. **不叠、不排队**：后一条直接覆盖前一条。连弹三条只会让人关掉通知。
//! 3. **自动消失**：5 秒后自己收起来；点一下可以提前关掉。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const TOAST_LABEL: &str = "toast";

/// 逻辑尺寸。宽度够放「今日已到账 ¥367.82，明天见」一行不折行。
const TOAST_W: f64 = 340.0;
const TOAST_H: f64 = 96.0;

/// 第几次通知。用来让「上一轮的定时关闭」失效——
/// 不判这个的话，5 秒前那条的定时器会把刚弹出来的新通知一起关掉。
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 最近一条通知的内容。
///
/// 为什么还要存一份：窗口是**第一次要弹的时候才建**的，`emit_to` 发出去的那一刻
/// WebView2 里那页 JS 还没跑起来，监听器也还没挂上——事件直接丢掉。
/// 表现出来就是「窗口弹出来了，但是空的、透明的，等于没有」。
/// 页面起来后自己去 `toast_pull` 捞一次，两条路任一条通就够。
static PENDING: Mutex<Option<Payload>> = Mutex::new(None);

#[derive(Clone, serde::Serialize)]
pub struct Payload {
    title: String,
    body: String,
    tone: String,
}

/// 通知页加载完了，来问「有没有要显示的」。
#[tauri::command]
pub fn toast_pull() -> Option<Payload> {
    PENDING.lock().ok().and_then(|g| g.clone())
}

/// 弹一条通知。`tone` 是 `info` / `ok` / `warn` 三种之一。
pub fn show(app: &AppHandle, title: &str, body: &str, tone: &str) {
    let payload = Payload {
        title: title.to_string(),
        body: body.to_string(),
        tone: tone.to_string(),
    };
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(payload.clone());
    }

    let Some(win) = ensure(app) else { return };

    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    place(app, &win);
    // 先 emit 再 show：页面已经在了就直接看到内容；页面是刚建的，这一发会丢，
    // 由页面起来后的 `toast_pull` 补上。
    let _ = app.emit_to(TOAST_LABEL, "toast", payload);
    let _ = win.show();
    crate::log::w(&format!("通知已弹：{title} / {body}"));


    // 5 秒后自己收。用后台线程而不是 sleep 在前端：前端一段死循环的
    // setTimeout 会在窗口被复用后叠起来。
    let app2 = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(5_000));
        // 这中间又弹了一条的话，就别关了——手上的不是那一条
        if GENERATION.load(Ordering::SeqCst) == gen {
            if let Some(win) = app2.get_webview_window(TOAST_LABEL) {
                let _ = win.hide();
            }
        }
    });
}

/// 用户点了通知：立刻收起来。
#[tauri::command]
pub fn toast_dismiss(app: AppHandle) {
    if let Some(win) = app.get_webview_window(TOAST_LABEL) {
        let _ = win.hide();
    }
}

/// 第一次要通知时才建窗口。
///
/// **现在改成启动时就建好（藏着），这里只负责「确保它在」。**
/// 原因见 `prewarm` 的注释：懒创建那条路上，窗口是弹出来了，但页面是空白的。
fn ensure(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(TOAST_LABEL) {
        return Some(w);
    }
    build(app)
}

/// 启动时先把通知窗建好，只藏不显示。
///
/// ## 为什么不再懒加载
///
/// 原设计是「第一次要弹的时候才建」，省一个 WebView 的内存。实测这条路不通：
/// 运行期新建的这扇窗，`show()` 之后整块是**透明的空白**——窗口在、位置对、
/// 截图里什么都没有。HUD 是启动时由配置建出来的，同样的 `transparent(true)`
/// 却一直是好的。差别就在「什么时候建」。
///
/// 与其去赌 WebView2 什么时候才把第一帧合成出来，不如在启动时把窗口建好、
/// 让页面正常加载一次、然后藏起来——到点只是 `show()` 一下，不涉及任何创建。
/// 代价是常驻一个 WebView（几十兆），换来的是「到点一定有东西出来」，
/// 这个交换值得：一条不该出现的通知，等于整个提醒功能不存在。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Some(_) => crate::log::w("通知窗已预热（藏起，等第一次通知）"),
        None => crate::log::w("通知窗预热失败，到点可能弹不出来"),
    }
}

fn build(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let built = WebviewWindowBuilder::new(app, TOAST_LABEL, WebviewUrl::App("toast.html".into()))
        .title("薪动了")
        .inner_size(TOAST_W, TOAST_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .shadow(false)
        .visible(false)
        .build();
    // 失败不能一口吞掉：`.ok()` 会让「通知没弹」变成一件完全无法排查的事——
    // 用户只看到该弹的没弹，日志里一个字都没有。
    match built {
        Ok(w) => {
            crate::log::w("通知窗已创建");
            Some(w)
        }
        Err(e) => {
            crate::log::w(&format!("通知窗创建失败：{e}"));
            eprintln!("通知窗创建失败：{e}");
            None
        }
    }
}

/// 摆在当前工作区的右下角。
///
/// 用**主**显示器而不是鼠标所在屏：通知要出现在一个可预期的位置，
/// 跟着鼠标满屏跑反而会让人找不到。
fn place(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Ok(Some(mon)) = app.primary_monitor() else {
        return;
    };
    let scale = mon.scale_factor();
    let mw = mon.size().width as f64;
    let mh = mon.size().height as f64;
    let margin = 18.0 * scale;
    // 任务栏在下面，留 56pt 的余量给它
    let x = mw - TOAST_W * scale - margin;
    let y = mh - TOAST_H * scale - 56.0 * scale;
    let _ = win.set_position(tauri::PhysicalPosition::new(x.max(0.0) as i32, y.max(0.0) as i32));
}
