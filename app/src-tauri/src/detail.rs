//! 「今日明细」那扇窗。
//!
//! ## 为什么这时候才做
//!
//! 原型稿里「今日明细」只有一行字（右键菜单里的一个项名），没画过界面。
//! 凭空做一扇窗就是自创，所以一直搁着。现在按现有的视觉语言补出来：
//! 深色玻璃 + 等宽数字 + 绿色主强调，和 `extra` 窗同一套变量。
//!
//! ## 这扇窗展示什么
//!
//! 一张「今天」的实时账目，全部数字由 Rust 算好，前端一行算术都没有：
//!
//! - 今日已赚（含加班计费部分）、已工作时长、白干价值
//! - 日薪 / 时薪 / 每秒（口径参考，不是累加出来的）
//! - 午休加班段（收口 + 未收口）、暂停段、今日额外进账
//!
//! 它只读，不改任何东西。真正的写路径还是 `AppState::mutate` 一条。

use chrono::{Datelike, TimeZone, Timelike};
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

/// 窗口标签。和 `extra` / `toast` / `settings` 区分开。
pub const LABEL: &str = "detail";

const DETAIL_W: f64 = 360.0;
const DETAIL_H: f64 = 460.0;

/// 一段午休加班。收口/未收口都算一条。
#[derive(Serialize)]
pub struct LunchSpanView {
    /// `HH:MM`
    pub start: String,
    /// `HH:MM`；还在记就是 `—`
    pub end: String,
    /// 这一段记了多久，`H 小时 M 分`（Rust 算好）
    pub len: String,
    /// 白干价值（毫）
    pub value_milli: i64,
    /// 白干价值（格式化好的 `¥x.xx`）
    pub value: String,
    /// 还在记吗
    pub open: bool,
}

/// 一段暂停。今天内开始/结束的都算。
#[derive(Serialize)]
pub struct PauseSpanView {
    pub start: String,
    pub end: String,
    pub len: String,
    /// 还在暂停吗
    pub open: bool,
}

/// 一笔额外进账（只列今天的）。
#[derive(Serialize)]
pub struct ExtraView {
    pub time: String,
    pub amount: String,
}

/// 今日明细的完整视图。
#[derive(Serialize)]
pub struct DetailView {
    /// `2026-11-11 周二`
    pub today: String,
    /// 今日已赚（毫）
    pub amount_milli: i64,
    /// `¥x.xx`
    pub amount: String,
    /// 白干价值（毫）
    pub shadow_milli: i64,
    pub shadow: String,
    /// 已工作时长 `H 小时 M 分`
    pub worked: String,
    /// 今日暂停累计
    pub paused: String,
    /// 加班秒数（下班后）
    pub ot_sec: u32,
    /// `H 小时 M 分`
    pub ot: String,
    /// 日薪 / 时薪 / 每秒（口径参考）
    pub day: String,
    pub hourly: String,
    pub per_sec: String,
    /// 今日额外进账合计
    pub extra_total: String,
    /// 状态字符串：on / break / ot / unpaid / done / leave / rest
    pub state: String,
    /// 午休加班段
    pub lunch_spans: Vec<LunchSpanView>,
    /// 暂停段
    pub pause_spans: Vec<PauseSpanView>,
    /// 今日额外进账明细
    pub extras: Vec<ExtraView>,
}

fn hm(ts: i64) -> String {
    let dt = chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| chrono::Local::now());
    format!("{:02}:{:02}", dt.hour(), dt.minute())
}

fn hms_len(sec: u32) -> String {
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    if h == 0 {
        format!("{m} 分")
    } else if m == 0 {
        format!("{h} 小时")
    } else {
        format!("{h} 小时 {m} 分")
    }
}

/// 组装今日明细。`snap` 是 `tick` 已经算好的实时快照，
/// 这里只从它和配置里挑出「今天」的那几条，不做任何金额换算。
pub fn view(app: &AppHandle) -> DetailView {
    let st = app.state::<AppState>();
    let s = st.settings();
    let now = crate::clock(&s);
    let snap = crate::snapshot_at(now, &s);

    let today = now.date_naive();
    let lo = crate::day_start(now);
    let now_ts = now.timestamp();

    let today_iso = today.format("%Y-%m-%d").to_string();

    // 午休加班段：只列今天的（跨天封账，昨天的不算）。
    // 每段的白干价值用引擎的 `shadow_sum` 单独算——分段展示要的就是「这一段值多少」，
    // 不能把总额均摊回去（那会引入外壳里的除法）。
    let p = crate::params(&s);
    let lunch_spans: Vec<LunchSpanView> = s
        .lunch_ots
        .iter()
        .filter(|l| l.start >= lo)
        .map(|l| {
            let len = l.len(now_ts);
            let cap_sec = if l.end.is_some() { len } else { l.cap_sec };
            let span = paypulse_core::ShadowSpan {
                start: l.start.max(0) as u64,
                rate_x100: p.lunch_shadow_rate_x100,
                cap_sec,
            };
            let value_milli = paypulse_core::shadow_sum(std::slice::from_ref(&span), now_ts.max(0) as u64, &p);
            LunchSpanView {
                start: hm(l.start),
                end: l.end.map_or("—".to_string(), hm),
                len: hms_len(len),
                value_milli,
                value: crate::yuan(value_milli, 2),
                open: l.end.is_none(),
            }
        })
        .collect();

    // 暂停段：今天内开始、或跨今天仍在记的。
    let pause_spans: Vec<PauseSpanView> = s
        .pauses
        .iter()
        .filter(|p| p.start >= lo || p.end.map_or(false, |e| e >= lo))
        .map(|p| {
            let start = p.start.max(lo);
            let end = p.end.unwrap_or(now_ts);
            let len = (end - start).max(0) as u32;
            PauseSpanView {
                start: hm(start),
                end: p.end.map_or("—".to_string(), hm),
                len: hms_len(len),
                open: p.end.is_none(),
            }
        })
        .collect();

    // 今天的额外进账。日期存 `YYYY-MM-DD`，直接比字符串。
    let extras: Vec<ExtraView> = s
        .extras
        .iter()
        .filter(|e| e.date == today_iso)
        .map(|e| ExtraView {
            time: "今天".to_string(),
            amount: crate::yuan(e.amount_milli, 2),
        })
        .collect();

    let extra_total = s.extras.iter().filter(|e| e.date == today_iso).map(|e| e.amount_milli).sum::<i64>();

    let wd = match today.weekday() {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    };

    DetailView {
        today: format!("{} {}", today_iso, wd),
        amount_milli: snap.amount_milli,
        amount: crate::yuan(snap.amount_milli, 2),
        shadow_milli: snap.shadow_milli,
        shadow: crate::yuan(snap.shadow_milli, 2),
        worked: hms_len(snap.worked_sec),
        paused: hms_len(snap.pause_sec),
        ot_sec: snap.ot_sec,
        ot: hms_len(snap.ot_sec),
        day: crate::yuan(snap.day_milli, 2),
        hourly: crate::yuan(snap.hourly_milli, 2),
        per_sec: format!("¥{:.4}", snap.per_sec_micro as f64 / 1_000_000.0),
        extra_total: crate::yuan(extra_total, 2),
        state: snap.state.clone(),
        lunch_spans,
        pause_spans,
        extras,
    }
}

/// 今日明细的命令。窗口每次显示前都会调一次，保证是最新的。
#[tauri::command]
pub fn detail_view(app: AppHandle) -> DetailView {
    view(&app)
}

/// 收起今日明细窗（Esc 或失焦）。
#[tauri::command]
pub fn close_detail(app: AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

/// 打开（或聚焦）今日明细窗口。
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        // 每次打开都刷一次数据：`detail_view` 命令由前端在 `focus` 时再拉，
        // 这里先把窗口亮出来。
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }

    // 懒创建透明窗口 show 出来是空白的，正常情况窗口已在启动时预热。
    crate::log::w("今日明细：窗口没预热，现建（内容可能空白）");
    let w = build(app)?;
    let _ = w.show();
    let _ = w.set_focus();
    Ok(())
}

/// 启动时把今日明细窗口建好藏着：和通知窗、右键菜单窗一样，
/// 懒创建的透明窗口第一次 `show()` 是空白的，先建好让页面加载一次。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Ok(_w) => {
            crate::log::w("今日明细窗已预热（藏起）");
        }
        Err(e) => crate::log::w(&format!("今日明细窗预热失败：{e}")),
    }
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("detail.html".into()))
        .title("今日明细")
        .inner_size(DETAIL_W, DETAIL_H)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .center()
        // 建的时候就藏着：配合 prewarm 让页面先加载，打开时只 show。
        .visible(false)
        .focused(false)
        .build()?;

    // 失焦就收起来，和 `extra` 窗一致：这是一扇「看一眼就走」的窗。
    let me = w.clone();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::Focused(false) = e {
            let _ = me.hide();
        }
    });
    Ok(w)
}
