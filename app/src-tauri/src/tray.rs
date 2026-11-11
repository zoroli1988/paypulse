//! 系统托盘。
//!
//! 它在这里承担一件超出「快捷菜单」的职责：**证明程序还活着**。
//! 悬浮窗可以隐藏、可以被别的置顶窗口压住、可以因为鼠标穿透点不到，
//! 但托盘图标一直在——用户看到它，就知道计时没停、钱还在涨。
//! 反过来说，如果没有托盘，用户关掉悬浮窗之后会以为软件崩了。
//!
//! 图标用 `icons/tray.png` 而不是主图标：托盘在 100% 缩放下只有 16×16，
//! 主图标那套「方块 + 鼻环 + 鼻孔」在这个尺寸上会糊成一个点。
//! 生成脚本见 `tools/make-icons/gen_tray.py`。

use crate::AppState;
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuEvent, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::image::Image;
use tauri::{AppHandle, Manager, Wry};

const ID_SHOW: &str = "show";
const ID_SIZE: &str = "size";
const ID_PAUSE: &str = "pause";
const ID_LEAVE: &str = "leave-today";
const ID_HIDE: &str = "hide";
const ID_THRU: &str = "through";
const ID_OT_PAID: &str = "ot-paid";
const ID_SETTINGS: &str = "settings";
const ID_DETAIL: &str = "detail";
const ID_RESET: &str = "reset-pos";
const ID_QUIT: &str = "quit";

const TRAY_ICON: &[u8] = include_bytes!("../icons/tray.png");

/// 菜单项的句柄。
///
/// 为什么要单独管起来：勾选状态有两个入口会改它——托盘菜单自己，
/// 以及窗口上的 ◎ / 「恢复计时」按钮。后者改完如果不通知菜单，
/// 就会出现「按钮亮着、菜单没勾」的分裂状态。`TrayIcon` 上拿不到 menu，
/// 所以只能在建的时候留一份。
pub struct TrayHandles {
    show: CheckMenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    leave: CheckMenuItem<Wry>,
    hide: CheckMenuItem<Wry>,
    thru: CheckMenuItem<Wry>,
    ot_paid: CheckMenuItem<Wry>,
}

/// 把勾选项按当前配置刷一遍。
pub fn sync_checks(app: &AppHandle) {
    let Some(h) = app.try_state::<TrayHandles>() else {
        return;
    };
    let s = app.state::<AppState>().settings();
    let _ = h.hide.set_checked(s.hide_money);
    let _ = h.thru.set_checked(s.click_through);
    let _ = h.pause.set_checked(pause_checked(&s));
    let _ = h
        .leave
        .set_checked(s.is_leave_day(crate::clock(&s).date_naive()));
    let _ = h
        .ot_paid
        .set_checked(matches!(s.ot_mode, crate::settings::OtMode::Paid));
    // 「显示悬浮窗」跟窗口可见性走，不跟配置走
    if let Some(win) = window(app) {
        let _ = h.show.set_checked(win.is_visible().unwrap_or(true));
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let st = app.state::<AppState>();
    let s = st.settings();

    let show = CheckMenuItem::with_id(app, ID_SHOW, "显示悬浮窗", true, true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, ID_PAUSE, "暂停计时", true, pause_checked(&s), None::<&str>)?;
    let leave = CheckMenuItem::with_id(
        app,
        ID_LEAVE,
        "今天请假",
        true,
        s.is_leave_day(crate::clock(&s).date_naive()),
        None::<&str>,
    )?;
    let hide = CheckMenuItem::with_id(app, ID_HIDE, "隐藏金额", true, s.hide_money, None::<&str>)?;
    let thru = CheckMenuItem::with_id(app, ID_THRU, "鼠标穿透", true, s.click_through, None::<&str>)?;
    let ot_paid = CheckMenuItem::with_id(
        app,
        ID_OT_PAID,
        "加班按 1.5× 计薪",
        true,
        matches!(s.ot_mode, crate::settings::OtMode::Paid),
        None::<&str>,
    )?;
    let reset = MenuItemBuilder::with_id(ID_RESET, "重置位置").build(app)?;
    // 原型 01-D 的右键菜单里有「切换尺寸」，这里照搬。
    // 用普通项而不是勾选项：三档是循环的，勾选表达不了「下一档是谁」。
    let size = MenuItemBuilder::with_id(ID_SIZE, "切换尺寸").build(app)?;
    let detail = MenuItemBuilder::with_id(ID_DETAIL, "今日明细").build(app)?;
    let settings = MenuItemBuilder::with_id(ID_SETTINGS, "设置…").build(app)?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "退出").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show)
        .item(&size)
        .item(&detail)
        .separator()
        .item(&pause)
        .item(&leave)
        .item(&hide)
        .item(&thru)
        .separator()
        // 「加班怎么算」原本是设置页里的下拉（原型稿 07 章）。
        // 设置窗口起来之后它在那里也有一份，这里保留：临时改一下不用开窗口。
        // 放在托盘而不是悬浮窗上，是因为它一年也改不了一次，
        // 悬浮窗那一小块地方要留给每天都会点的按钮。
        .item(&ot_paid)
        .separator()
        .item(&settings)
        .item(&reset)
        .separator()
        .item(&quit)
        .build()?;

    app.manage(TrayHandles {
        show: show.clone(),
        pause: pause.clone(),
        leave: leave.clone(),
        hide: hide.clone(),
        thru: thru.clone(),
        ot_paid: ot_paid.clone(),
    });

    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(TRAY_ICON)?)
        .tooltip("牛马薪动了！")
        .menu(&menu)
        // 左键不弹菜单，改成「显示/隐藏悬浮窗」——这是最高频的动作，
        // 弹了菜单反而多点一次。
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
                sync_checks(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn pause_checked(s: &crate::settings::Settings) -> bool {
    paypulse_clock::is_paused(&s.spans())
}

/// 换下一档尺寸。宽度在这里就改掉（前端下一帧再回填高度），
/// 否则会有一帧窗口比内容宽。
pub(crate) fn cycle_size_now(app: &AppHandle) {
    let st = app.state::<AppState>();
    let next = st.cycle_size(1);
    if let Some(win) = window(app) {
        crate::apply_width(&win, next);
    }
}

pub(crate) fn toggle_pause_now(app: &AppHandle) {
    let st = app.state::<AppState>();
    // 用真实时钟的 Unix 秒，区间就是这么存的
    let now_ts = chrono::Local::now().timestamp();
    st.toggle_pause(now_ts);
}

/// 退出。`from` 只用来写日志——上次「它自己没了」查不出原因，
/// 就吃亏在退出路径一声不响。
pub(crate) fn quit_now(app: &AppHandle, from: &str) {
    let st = app.state::<AppState>();
    // 先落盘再退：节流可能让最后一次位移还没写
    st.flush();
    crate::log::w(&format!("退出：{from}点了「退出」"));
    app.exit(0);
}

fn window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("hud")
}

fn toggle_window(app: &AppHandle) -> bool {
    let Some(win) = window(app) else { return false };
    let visible = win.is_visible().unwrap_or(true);
    if visible {
        let _ = win.hide();
        false
    } else {
        let _ = win.show();
        let _ = win.set_focus();
        true
    }
}

fn on_menu(app: &AppHandle, ev: MenuEvent) {
    let st = app.state::<AppState>();

    match ev.id().as_ref() {
        ID_SHOW => {
            toggle_window(app);
        }

        // 和窗口上的双击 / 滚轮走同一个入口，保证三条路径的档位顺序一致。
        ID_SIZE => cycle_size_now(app),

        ID_PAUSE => toggle_pause_now(app),

        ID_HIDE => {
            let on = !st.settings().hide_money;
            st.set_hide_money(on);
        }

        ID_LEAVE => {
            let today = crate::clock(&st.settings()).date_naive();
            st.toggle_leave_today(today);
        }

        ID_SETTINGS => {
            if let Err(e) = crate::open_settings(app) {
                eprintln!("打开设置窗口失败：{e}");
            }
        }

        ID_DETAIL => {
            if let Err(e) = crate::detail::open(app) {
                eprintln!("打开今日明细失败：{e}");
            }
        }

        ID_THRU => {
            let on = !st.settings().click_through;
            if let Some(win) = window(app) {
                if win.set_ignore_cursor_events(on).is_ok() {
                    st.set_click_through(on);
                }
            }
        }

        ID_OT_PAID => {
            let paid = !matches!(st.settings().ot_mode, crate::settings::OtMode::Paid);
            st.set_ot_mode(paid);
        }

        ID_RESET => crate::reset_position(app),

        ID_QUIT => quit_now(app, "托盘菜单"),

        _ => {}
    }

    // 统一在最后刷一次勾选：每个分支自己 set_checked 的话，
    // 迟早有一个分支忘了写，菜单就开始说假话。
    sync_checks(app);
}
