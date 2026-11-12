//! 「时区变了，按哪边的钟算？」这扇窗。
//!
//! ## 为什么需要它
//!
//! 规格 02-B 写的是：**以设备当前时区解释作息表；若变更超过 2 小时，弹窗询问
//! 「按哪边算」，一次确认、不反复打扰。** 上一版只做了设置里那个
//! 「跟随系统 / 固定 UTC+8」的手动开关，一个字的检测都没有——于是人在东京打开
//! 电脑，作息被整体平移一小时，界面上看不出任何异样，只是「今天 10 点才显示在岗」。
//! 那种错最难自查：数字都在跳，跳的却是别人的钟。
//!
//! ## 为什么是窗口，不是通知
//!
//! `toast` 是一条 5 秒后自己消失、点一下就关的信息条，承载不了「选一个」。
//! 而这一次选择会改掉接下来所有天的算法，必须留下一次明确的交互。
//!
//! ## 生命周期
//!
//! 和 menu / detail / toast / onboard 完全一致：启动时 `prewarm()` 建好藏起来，
//! 要问的时候只 `show()`；`CloseRequested` 一律拦成 `hide()`，**永不销毁**。
//! 懒创建的透明窗口第一次 show 出来是空白（见 `toast::prewarm` 的注释），
//! 而这扇窗如果白着弹出来，用户既看不懂也点不动，等于把「出差那天算错账」的
//! 毛病换成了「出差那天弹白框」。

use crate::settings::Settings;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "ask";

/// 420×256。宽度按最长那行算：`按公司时间 UTC+8　北京 09:00 = 当地 10:00` 一行排得下。
/// 高度是量出来的：标题栏 32 + 正文（标题 20 / 副标 17 / 对照表 69 / 提示 15）
/// + 按钮 34 + 各处间距（14+4+11+9+12+16），合计 249，取 256 留 7px 余量。
const ASK_W: f64 = 420.0;
const ASK_H: f64 = 256.0;

/// 给前端的一屏数据。
///
/// 时区换算**全部在这里算完**：外壳 JS 里不做算术是这个项目从第一天起的规矩
/// （`Settings` 视图、`Snapshot` 都是这个路数），换算尤其不能破例——
/// 「UTC+9 的 09:00 换成北京几点」正是最容易写反的那一步。
#[derive(Clone, serde::Serialize)]
pub struct AskView {
    /// 现在：`UTC+9`
    now_label: String,
    /// 上次记录：`UTC+8`
    prev_label: String,
    /// 差多久：`1 小时` / `2 小时 30 分`
    diff_label: String,
    /// 按本地时间：当地几点开始算钱
    local_start: String,
    /// 按本地时间：那一刻北京几点
    local_start_china: String,
    /// 按公司时间：北京几点开始算钱
    company_start: String,
    /// 按公司时间：那一刻当地几点
    company_start_local: String,
}

/// 现在有没有一扇开着的询问窗。
///
/// 用来挡住「1 秒一拍」的重复弹窗。用可见性而不是标志位：用户点了 × 之后
/// 窗口只是藏起来，标志位忘了清就会永远不再问；可见性不会说谎。
pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// 这一轮要问的内容。
///
/// 为什么要存一份：窗口是**启动时就建好、一直藏着**的，`show()` 之后页面才开始
/// 收事件也不一定——`ask()` 里那一发 `emit` 很可能先于页面的监听器。
/// 页面起来后自己来 `ask_pull` 捞一次，两条路任一条通就够（和 toast 同一套）。
///
/// 注意它存的是**这一次问的那两个偏移**，不是「读现在的配置再算一遍」：
/// 后者会把基准（`tz_off`）当成上次的时区，于是文案变成「比上次的 UTC+8 差 0 分」——
/// 一个自问自答的空问题。第一版就是这么错的，锁屏截图上肉眼可见。
static PENDING: Mutex<Option<AskView>> = Mutex::new(None);

/// 弹一次。`prev` / `now` 都是本地 UTC 偏移（秒）。
pub fn ask(app: &AppHandle, prev: i32, now: i32) {
    let s = app.state::<crate::AppState>().settings();
    let view = view_of(&s, prev, now);
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(view.clone());
    }

    let win = match app.get_webview_window(LABEL) {
        Some(w) => w,
        None => {
            // 正常情况启动时就预热好了。走到这里说明预热失败，
            // 内容可能是白的，但至少给用户一个能点的东西。
            crate::log::w("时区询问窗：没预热，现建（内容可能空白）");
            match build(app) {
                Ok(w) => w,
                Err(e) => {
                    crate::log::w(&format!("时区询问窗创建失败：{e}"));
                    return;
                }
            }
        }
    };

    // 先发数据再显示：页面若刚起来监听还没挂上，这一发会丢，
    // 由页面加载完的 `ask_pull` 补上。
    let _ = app.emit_to(LABEL, "ask", view);
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    crate::log::w(&format!(
        "时区询问窗已弹出：{} → {}",
        crate::tz_label(prev),
        crate::tz_label(now)
    ));
}

pub fn close(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

/// 启动时建好藏着。理由同其他窗口：懒创建第一次 show 是空白。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Ok(_) => crate::log::w("时区询问窗已预热（藏起）"),
        Err(e) => crate::log::w(&format!("时区询问窗预热失败：{e}")),
    }
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("ask.html".into()))
        .title("时区变了 · 牛马薪动了！")
        .inner_size(ASK_W, ASK_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .center()
        .visible(false)
        .focused(false)
        .build()?;

    // 关闭 = 隐藏。理由见模块注释最后一段。
    let me = w.clone();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
            api.prevent_close();
            let _ = me.hide();
            // 关掉也算「回答过了」：规格要的是「一次确认，不反复打扰」，
            // 而用户点 × 的意思就是「别问了」。选择保持原样（跟随系统），
            // 但基准时区要跟着更新，否则下一秒又会弹回来。
            crate::tz_remember(me.app_handle(), crate::local_offset_sec());
        }
    });

    Ok(w)
}

/// 页面上要显示的那几个数。
fn view_of(s: &Settings, prev: i32, now: i32) -> AskView {
    let company = crate::EAST8_SEC;
    // 两边差多少：正数表示当地比北京早（比如东京 +9）。
    let delta_min = (now - company) / 60;

    AskView {
        now_label: crate::tz_label(now),
        prev_label: crate::tz_label(prev),
        diff_label: hm_gap(now - prev),
        local_start: hhmm(s.sched.start_min as i64),
        // 按本地时间算 = 「当地 09:00 开始」= 北京 (9:00 − delta)
        local_start_china: hhmm(s.sched.start_min as i64 - delta_min as i64),
        company_start: hhmm(s.sched.start_min as i64),
        // 按公司时间算 = 「北京 09:00 开始」= 当地 (9:00 + delta)
        company_start_local: hhmm(s.sched.start_min as i64 + delta_min as i64),
    }
}

/// 分钟偏移 → `HH:MM`。跨零点和负数都要能读，所以先取模再加一天。
fn hhmm(min_of_day: i64) -> String {
    let m = ((min_of_day % 1440) + 1440) % 1440;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// 两个偏移差多久，说人话：`1 小时` / `2 小时 30 分` / `45 分`。
fn hm_gap(sec: i32) -> String {
    let m = sec.abs() / 60;
    let (h, mi) = (m / 60, m % 60);
    if h == 0 {
        format!("{mi} 分")
    } else if mi == 0 {
        format!("{h} 小时")
    } else {
        format!("{h} 小时 {mi} 分")
    }
}

/// 用户在窗口里点了哪个。
///
/// `local` = 按本地时间（跟随系统时区）；`company` = 按公司时间（固定 UTC+8）。
/// 两种都把当前偏移记下来——这一趟出差到此问完，回程再变才会再问。
#[tauri::command]
pub fn ask_answer(app: AppHandle, mode: String) {
    let fixed = mode == "company";
    let off = crate::local_offset_sec();
    app.state::<crate::AppState>().mutate(|x| {
        x.tz_fixed = fixed;
        x.tz_off = Some(off);
    });
    crate::log::w(&format!(
        "时区询问已答：{}（{}）",
        if fixed { "按公司时间 UTC+8" } else { "按本地时间" },
        crate::tz_label(off)
    ));
    // 设置窗口若正开着，它上面那个下拉框现在是旧的。刷新一次，
    // 免得「窗口里写着跟随系统、实际已经固定 UTC+8」这种分裂。
    crate::refresh_settings_view(&app);
    close(&app);
}

/// 页面加载完来要数据（事件比监听器先到的那条路）。
#[tauri::command]
pub fn ask_pull() -> Option<AskView> {
    PENDING.lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s_default() -> Settings {
        Settings::default() // 09:00 上班
    }

    /// 东京（UTC+9）：按本地时间算，等于北京 08:00；按公司时间算，等于当地 10:00。
    #[test]
    fn tokyo_shift_reads_both_ways() {
        let v = view_of(&s_default(), 8 * 3600, 9 * 3600);
        assert_eq!(v.now_label, "UTC+9");
        assert_eq!(v.prev_label, "UTC+8");
        assert_eq!(v.diff_label, "1 小时");
        assert_eq!(v.local_start, "09:00");
        assert_eq!(v.local_start_china, "08:00");
        assert_eq!(v.company_start, "09:00");
        assert_eq!(v.company_start_local, "10:00");
    }

    /// 往西飞（伦敦 UTC+0）：当地 09:00 上班 = 北京 17:00；北京 09:00 = 当地 01:00。
    /// 跨零点必须能读，不能出现 `-8:00` 或 `-1:00`。
    #[test]
    fn westbound_crosses_midnight() {
        let v = view_of(&s_default(), 8 * 3600, 0);
        assert_eq!(v.company_start_local, "01:00");
        assert_eq!(v.local_start_china, "17:00");
    }

    /// 半小时时区（印度 +5:30）也要对：差 2.5 小时，不是 2 或 3。
    #[test]
    fn half_hour_zone_is_not_rounded() {
        let v = view_of(&s_default(), 8 * 3600, 5 * 3600 + 1800);
        assert_eq!(v.now_label, "UTC+5:30");
        assert_eq!(v.diff_label, "2 小时 30 分");
        assert_eq!(v.company_start_local, "06:30");
        assert_eq!(v.local_start_china, "11:30");
    }
}
