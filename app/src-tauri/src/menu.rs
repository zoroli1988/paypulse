//! 悬浮窗右键的**自绘菜单**。
//!
//! ## 为什么不用原生菜单了
//!
//! 上一版用 `Menu::popup()`（`ContextMenu`）弹的是系统原生菜单，在 Windows 上
//! 它永远是**浅色**的（灰底黑字），而悬浮窗是深色玻璃——右键出来一块刺眼的
//! 白菜单，用户会觉得「怎么跟我们这窗不搭」。原型稿 01-D 的右键菜单画的就是
//! 深色五项，系统菜单染不成深色，所以换成自绘一扇小窗。
//!
//! 托盘菜单**保留原生**：托盘是系统通知区的一部分，菜单本来就该跟着系统走，
//! 浅色反而和「开始菜单 / 其它托盘图标」一致，不突兀。
//!
//! ## 菜单项
//!
//! 原型 01-D 五项：设置 / 切换尺寸 / 暂停计时 / 今日明细 / 退出。
//! 「暂停计时」带勾选态（暂停中就勾上），勾选状态在 `menu_state` 里算好传下去。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

pub const LABEL: &str = "hudmenu";

const MENU_W: f64 = 208.0;
/// 每项 36px，5 项 + 上下 padding 12×2 = 204。
const MENU_H: f64 = 204.0;

/// 菜单项的当前状态：暂停勾选、三档尺寸名。
#[derive(serde::Serialize)]
pub struct MenuState {
    pub paused: bool,
    pub size: String,
}

#[tauri::command]
pub fn menu_state(app: AppHandle) -> MenuState {
    let s = app.state::<AppState>().settings();
    MenuState {
        paused: paypulse_clock::is_paused(&s.spans()),
        size: s.size.as_str().to_string(),
    }
}

/// 菜单里点了一项。`action` 是 `settings / size / pause / detail / quit` 之一。
/// 动作与托盘、右键旧版共用同一批入口，保证档位顺序、暂停区间、退出落盘的行为全一致。
///
/// 先关菜单再执行动作：否则「今日明细」「设置」这两扇窗 open 时，
/// 菜单还开着（都 always_on_top），焦点会在两扇窗之间抢，新窗可能刚起就被失焦收起。
#[tauri::command]
pub fn menu_action(app: AppHandle, action: String) -> Result<(), String> {
    // 先收菜单。
    let _ = close(&app);

    match action.as_str() {
        "settings" => crate::open_settings(&app).map_err(|e| e.to_string()),
        "size" => {
            crate::tray::cycle_size_now(&app);
            Ok(())
        }
        "pause" => {
            crate::tray::toggle_pause_now(&app);
            Ok(())
        }
        "detail" => crate::detail::open(&app).map_err(|e| e.to_string()),
        "quit" => {
            crate::tray::quit_now(&app, "悬浮窗右键");
            Ok(())
        }
        _ => Err(format!("不认识的菜单动作：{action}")),
    }
}

/// 收起菜单。
#[tauri::command]
pub fn menu_close(app: AppHandle) {
    let _ = close(&app);
}

fn close(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
    Ok(())
}

/// 在鼠标当前位置弹自绘菜单。
///
/// 鼠标坐标**不从前端传**：`contextmenu` 的 `screenX/screenY` 是 CSS 逻辑像素，
/// 在混合 DPI 的多屏上换算会偏（菜单会跑到另一块屏上）。这里直接用
/// `GetCursorPos` 拿屏幕物理坐标，`PhysicalPosition` 落位，任何屏都准。
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    let win = if let Some(w) = app.get_webview_window(LABEL) {
        w
    } else {
        // 懒创建透明窗口 show 出来是空白的（见 toast::prewarm 的注释），
        // 所以窗口在启动时已经建好，这里只该拿到现成的。万一没建好，
        // 退回去现建一个——内容可能空白，但菜单位置和结构是对的。
        crate::log::w("右键菜单：窗口没预热，现建（内容可能空白）");
        let w = build(app)?;
        w
    };

    let (x, y) = crate::platform::cursor_pos();
    // 菜单紧贴鼠标，但别顶出工作区。先看鼠标所在屏的工作区，把菜单夹进去。
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = win.show();
    // 不主动 set_focus：抢焦点会触发「失焦即收」的回调，菜单刚弹就缩回去。
    // 用户点菜单项时，Windows 自然把焦点给它。
    Ok(())
}

/// 启动时把菜单窗口建好藏着：懒创建的透明窗口 `show()` 出来是空白的
/// （WebView2 还没渲染第一帧），预先建好让页面加载一次，右键时只 `show()`。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Ok(_w) => {
            crate::log::w("右键菜单窗已预热（藏起，等第一次右键）");
        }
        Err(e) => crate::log::w(&format!("右键菜单窗预热失败：{e}")),
    }
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("menu.html".into()))
        .title("")
        .inner_size(MENU_W, MENU_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        // 建的时候就藏着：配合 prewarm 让页面先加载，右键时只 show。
        .visible(false)
        .focused(false)
        .build()?;
    // 失焦就收：点菜单外任意处、或切到别的程序，菜单都该消失。
    let me = w.clone();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::Focused(false) = e {
            let _ = me.hide();
        }
    });
    Ok(w)
}
