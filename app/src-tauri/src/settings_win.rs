//! 设置窗口（原型稿 04 章「设置中心」）。
//!
//! ## 为什么是「一个窗口 + 一堆命令」，而不是前端直接改 JSON
//!
//! 配置文件的写盘路径只有一条：`AppState::mutate` → `validate` → 原子写。
//! 如果设置窗口自己读文件、自己写文件，就会出现两条写路径、两套校验，
//! 迟早出现「设置窗口里是 8000、悬浮窗按 9000 算」这种分裂。
//! 所以这里只做三件事：把配置整理成前端好画的样子、把前端的改动合并回配置、
//! 开窗口。**校验和落盘仍然只有 `Settings::validate` 一处。**
//!
//! ## 为什么界面上没有「保存」
//!
//! 原型 04 章明写「没有保存按钮，改完即生效」。这不是偷懒：这是个看数字的工具，
//! 改月薪最想看到的是「时薪当场变了」。所以每个控件改完就调一次 `apply_settings`，
//! 返回值里带着重新算好的日薪/时薪/每秒——前端只负责填字，不做算术。

use crate::settings::{
    BonusMode, CloseAction, Density, NotifyEnd, OtMode, Settings, Size, Theme,
};
use crate::AppState;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// 设置窗口的标签。和悬浮窗的 `hud` 区分开——窗口位置记忆只认 `hud`。
pub const LABEL: &str = "settings";

// ---------------------------------------------------------------- 视图

/// 作息那一块的展示值。
///
/// 时刻在这里就格式化成 `HH:MM`，工时也在这里算成小数：
/// 让前端去 `分钟 / 60` 也能做，但那就破了「外壳 JS 不做算术」这条线——
/// 这条线是为了让所有数字都能被 Rust 的单测盖住，不该为了省几行而破例。
#[derive(Serialize)]
pub struct SchedView {
    pub start: String,
    pub end: String,
    pub lunch1: String,
    pub lunch2: String,
    /// 在岗小时（含午休）
    pub span_h: f64,
    /// 午休小时
    pub lunch_h: f64,
    /// 计薪小时
    pub paid_h: f64,
    /// 允许跨零点夜班
    pub cross_midnight: bool,
    /// 当前这套时刻是不是跨天的（由 end <= start 推出来，不是自己一个字段）
    pub overnight: bool,
}

/// 排班形态那一块。
#[derive(Serialize)]
pub struct WeekView {
    /// five / bigsmall / six / custom
    pub form: String,
    pub days: [bool; 7],
    pub alt_sat: Option<u8>,
    /// 弹性上下班：起点按当天第一次活动算
    pub flex: bool,
    /// 本月是「哪一年哪一月」（日历标题）
    pub month: String,
    /// 1 号之前要空几格（周一起算）
    pub lead: u32,
    pub cells: Vec<DayCell>,
    pub holidays: u32,
    /// 本月周末（不上班、也不是放假日）的天数
    pub rest: u32,
    pub makeup: u32,
    /// 「2027-01-01 · 还有 52 天」；没有登记过任何假日时是「—」
    pub next_holiday: String,
}

/// 日历上的一格。
#[derive(Serialize)]
pub struct DayCell {
    pub d: u32,
    /// `YYYY-MM-DD`。加/删节假日时原样回传，前端不拼日期。
    pub iso: String,
    /// work / rest / holiday / makeup
    pub kind: String,
    /// 今天
    pub today: bool,
}

#[derive(Serialize)]
pub struct BonusView {
    /// none / months / amount / rate
    pub mode: String,
    /// 年终奖折合几个月工资（1.0 = 13 薪）
    pub months: f64,
    pub amount_yuan: f64,
    pub rate_pct: f64,
    /// 年终奖总额（元）
    pub total_yuan: f64,
    /// 摊到每月多少元
    pub per_month_yuan: f64,
    /// 「相当于 13.0 薪」
    pub annual_months: f64,
}

#[derive(Serialize)]
pub struct HotkeyView {
    pub enabled: bool,
    pub show: String,
    pub passthru: String,
    pub clock: String,
    pub extra: String,
}

#[derive(Serialize)]
pub struct NotifyView {
    /// off / amount / amountwork / amounteq
    pub end: String,
    pub lunch: bool,
    pub lunch_before: u32,
    pub sit: bool,
    pub sit_min: u32,
    pub dnd: bool,
    pub dnd_from: String,
    pub dnd_to: String,
}

/// 设置界面需要的全部内容。
#[derive(Serialize)]
pub struct SettingsView {
    pub month_yuan: f64,
    pub pay_days_centi: u32,
    pub daily_hours: f64,
    pub ot_paid: bool,
    pub sched: SchedView,
    pub week: WeekView,
    pub bonus: BonusView,
    /// 税前（应发）/ 税后（到手）
    pub gross: bool,
    /// 时区策略：true = 固定 UTC+8
    pub tz_fixed: bool,

    pub theme: String,
    pub num_font: String,
    /// 文案风格：emotional / plain / silent
    pub copy_style: String,
    pub autostart: bool,
    pub silent_start: bool,
    pub snap_px: u32,
    /// 启动一律摆主屏（不跟随上次所在的屏）
    pub monitor_primary: bool,
    pub idle_detect: bool,
    pub idle_min: u32,
    pub close_action: String,
    pub hotkey: HotkeyView,
    pub notify: NotifyView,
    /// 「现在这台机器上，空闲检测读到 X 秒」——设置页照实显示，不猜
    pub idle_now_sec: u64,

    pub accent: String,
    pub opacity: u8,
    pub size: String,
    pub density: Density,

    pub eq_name: String,
    pub eq_price_yuan: f64,
    pub eq_goal_cups: u32,
    pub eq_day_cups: u32,

    pub leave_kind: String,
    pub leave_left_days: f64,
    pub today_leave: bool,

    /// 鼠标穿透（托盘那项也是它）
    pub click_through: bool,
    /// 今天这笔钱折合几杯 × 10，用于设置页那句「今天 ≈ 11.6 杯奶茶」
    pub today_cups_x10: i64,

    /// 实时折算：日薪 / 时薪 / 每秒。改上面任何一项它们就变。
    pub day_yuan: f64,
    pub hourly_yuan: f64,
    pub per_sec_yuan: f64,

    /// 关于页用
    pub version: String,
    pub data_dir: String,
    pub paused: bool,
}

fn hm(min: u32) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

/// `"09:30"` → 570。认不出来返回 `None`，绝不 panic——
/// 这个值来自 `<input type="time">`，用户清空时它就是空串。
pub(crate) fn parse_hm(s: &str) -> Option<u32> {
    let s = s.trim();
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 24 || m > 59 {
        return None;
    }
    Some((h * 60 + m).min(24 * 60))
}

fn hourly(day_milli: i64, daily_sec: u32) -> i64 {
    if daily_sec == 0 {
        return 0;
    }
    // 四舍五入，不是截断：49.038 → 49.04
    (day_milli * 3_600 + daily_sec as i64 / 2) / daily_sec as i64
}

fn view(app: &AppHandle) -> SettingsView {
    let st = app.state::<AppState>();
    let s = st.settings();
    let dir = app.path().app_config_dir().unwrap_or_default();

    // 和悬浮窗共用同一份参数换算，避免「设置里显示的时薪」和「悬浮窗上的时薪」
    // 因为两处各写一遍除法而悄悄分叉。
    let p = crate::params(&s);
    let day_milli = p.day_milli();
    let cal = s.calendar();
    let today = crate::clock(&s).date_naive();

    // ---- 本月日历 ----
    // 格子在这里排好，前端只管按顺序画：星期几、有几天、哪格是今天，
    // 全是日期算术。放到 JS 里就要写 `new Date(2026, 10, 0).getDate()` 这种，
    // 而这条线上一直的规矩是「日历在 Rust，JS 只画」。
    let (year, month) = (today.year(), today.month());
    let first = NaiveDate::from_ymd_opt(year, month, 1);
    let lead = first
        .map(|d| d.weekday().num_days_from_monday())
        .unwrap_or(0);
    let days_in_month = {
        let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
        match (NaiveDate::from_ymd_opt(ny, nm, 1), first) {
            (Some(next), Some(f)) => next.signed_duration_since(f).num_days() as u32,
            _ => 30,
        }
    };
    let mut cells = Vec::with_capacity(days_in_month as usize);
    let (mut hol, mut mk, mut rst) = (0u32, 0u32, 0u32);
    for d in 1..=days_in_month {
        let Some(date) = NaiveDate::from_ymd_opt(year, month, d) else { continue };
        let kind = if cal.makeup.contains(&date) {
            mk += 1;
            "makeup"
        } else if cal.holidays.contains(&date) {
            hol += 1;
            "holiday"
        } else if cal.is_workday(date) {
            "work"
        } else {
            rst += 1;
            "rest"
        };
        cells.push(DayCell {
            d,
            // 让前端有「这一天是谁」的完整标识：点格子要往后端发 `YYYY-MM-DD`，
            // 而前端自己拼日期就得知道「现在是几月」——那正是我们不想让它算的东西。
            iso: date.to_string(),
            kind: kind.to_string(),
            today: date == today,
        });
    }

    // 下一个法定假日：只在自己登记的日期表里找，不猜。
    let next = s.week.holiday_dates().filter(|d| *d >= today).min();
    let next_holiday = match next {
        Some(d) => format!(
            "{} · 还有 {} 天",
            d,
            d.signed_duration_since(today).num_days()
        ),
        None => "—".to_string(),
    };

    let bonus_months = s.bonus.months_x100 as f64 / 100.0;

    SettingsView {
        month_yuan: s.month_milli as f64 / 1000.0,
        pay_days_centi: s.pay_days_centi,
        daily_hours: s.daily_sec as f64 / 3600.0,
        ot_paid: matches!(s.ot_mode, OtMode::Paid),
        sched: SchedView {
            start: hm(s.sched.start_min),
            end: hm(s.sched.end_min),
            lunch1: hm(s.sched.lunch_start_min),
            lunch2: hm(s.sched.lunch_end_min),
            span_h: s.sched.span_sec() as f64 / 3600.0,
            lunch_h: s.sched.lunch_sec() as f64 / 3600.0,
            paid_h: s.sched.paid_sec() as f64 / 3600.0,
            cross_midnight: s.sched.cross_midnight,
            overnight: s.sched.overnight(),
        },
        week: WeekView {
            form: s.week.form().as_str().to_string(),
            days: s.week.days,
            alt_sat: s.week.alt_sat,
            flex: s.week.flex,
            month: format!("{year} 年 {month} 月"),
            lead,
            cells,
            holidays: hol,
            rest: rst,
            makeup: mk,
            next_holiday,
        },
        bonus: BonusView {
            mode: match s.bonus.mode {
                BonusMode::None => "none",
                BonusMode::Months => "months",
                BonusMode::Amount => "amount",
                BonusMode::Rate => "rate",
            }
            .to_string(),
            months: bonus_months,
            amount_yuan: s.bonus.amount_milli as f64 / 1000.0,
            rate_pct: s.bonus.rate_x100 as f64 / 100.0,
            total_yuan: s.bonus.total_milli(s.month_milli) as f64 / 1000.0,
            per_month_yuan: s.bonus.per_month_milli(s.month_milli) as f64 / 1000.0,
            annual_months: 12.0 + bonus_months,
        },
        gross: s.gross,
        tz_fixed: s.tz_fixed,

        theme: s.theme.as_str().to_string(),
        num_font: s.num_font.as_str().to_string(),
        copy_style: s.copy_style.as_str().to_string(),
        autostart: s.autostart,
        silent_start: s.silent_start,
        snap_px: s.snap_px,
        monitor_primary: s.monitor_primary,
        idle_detect: s.idle_detect,
        idle_min: s.idle_min,
        close_action: match s.close_action {
            CloseAction::Tray => "tray",
            CloseAction::Quit => "quit",
        }
        .to_string(),
        hotkey: HotkeyView {
            enabled: s.hotkey.enabled,
            show: s.hotkey.show.clone(),
            passthru: s.hotkey.passthru.clone(),
            clock: s.hotkey.clock.clone(),
            extra: s.hotkey.extra.clone(),
        },
        notify: NotifyView {
            end: match s.notify.end {
                NotifyEnd::Off => "off",
                NotifyEnd::Amount => "amount",
                NotifyEnd::AmountWork => "amountwork",
                NotifyEnd::AmountEq => "amounteq",
            }
            .to_string(),
            lunch: s.notify.lunch,
            lunch_before: s.notify.lunch_before_min,
            sit: s.notify.sit,
            sit_min: s.notify.sit_min,
            dnd: s.notify.dnd,
            dnd_from: hm(s.notify.dnd_from_min),
            dnd_to: hm(s.notify.dnd_to_min),
        },
        idle_now_sec: crate::platform::idle_seconds(),

        accent: s.accent.clone(),
        opacity: s.opacity,
        size: s.size.as_str().to_string(),
        density: s.density,

        eq_name: s.eq_name.clone(),
        eq_price_yuan: s.eq_price_centi as f64 / 100.0,
        eq_goal_cups: s.eq_goal_cups,
        eq_day_cups: s.eq_day_cups,

        leave_kind: s.leave_kind.clone(),
        leave_left_days: s.leave_left_centi as f64 / 100.0,
        today_leave: s.is_leave_day(today),

        click_through: s.click_through,
        // 杯数只有一处算法（在 snapshot_at 里），这里直接把那个数拿过来，
        // 不在设置窗口另写一遍「金额 ÷ 单价」——两处各写一次迟早会差半杯。
        today_cups_x10: crate::snapshot_at(crate::clock(&s), &s).eq_cups_x10,

        day_yuan: day_milli as f64 / 1000.0,
        hourly_yuan: hourly(day_milli, s.daily_sec) as f64 / 1000.0,
        per_sec_yuan: p.per_sec_micro() as f64 / 1_000_000.0,

        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: dir.display().to_string(),
        paused: paypulse_clock::is_paused(&s.spans()),
    }
}

// ---------------------------------------------------------------- 补丁

/// 前端改哪一项就只传哪一项。
///
/// 为什么不收整份 `Settings`：那样前端一旦漏传某个字段，它就会被当成默认值写回去
/// （`pauses`、`win`、`leave_days` 这些界面根本不展示的字段会被清空）。
/// 用 `Option` 逐项合并，没传的字段保持原样。
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Patch {
    /// 月薪，元
    month_yuan: Option<f64>,
    /// 计薪天数 × 100
    pay_days_centi: Option<u32>,
    /// 每日计薪工时（小时）
    daily_hours: Option<f64>,
    /// 加班是否计入月薪
    ot_paid: Option<bool>,
    /// 作息（只传改动的时刻）
    sched: Option<SchedPatch>,
    accent: Option<String>,
    opacity: Option<u8>,
    density: Option<Density>,
    eq_name: Option<String>,
    eq_price_yuan: Option<f64>,
    eq_goal_cups: Option<u32>,
    eq_day_cups: Option<u32>,
    leave_kind: Option<String>,
    leave_left_days: Option<f64>,
    size: Option<String>,
    /// 鼠标穿透（和托盘那项是同一个开关）
    click_through: Option<bool>,
    /// 「今天请假」——它不是设置项，是托盘那个勾选的同一个入口，
    /// 放在这里是因为设置窗口也要能点。
    today_leave: Option<bool>,

    // ---- 排班 ----
    week_days: Option<[bool; 7]>,
    /// -1 = 关掉大小周；0/1 = 只让那一奇偶性的周六上班
    alt_sat: Option<i32>,
    /// five / bigsmall / six
    work_form: Option<String>,
    /// 弹性上下班（按首次活动计算起点）
    flex_start: Option<bool>,
    holiday: Option<DayOp>,
    makeup: Option<DayOp>,
    cross_midnight: Option<bool>,

    // ---- 年终奖 / 口径 ----
    bonus_mode: Option<String>,
    /// 年终奖 = 几个月工资（1.0 = 13 薪）
    bonus_months: Option<f64>,
    bonus_amount_yuan: Option<f64>,
    bonus_rate_pct: Option<f64>,
    gross: Option<bool>,
    tz_fixed: Option<bool>,

    // ---- 外观 ----
    theme: Option<String>,
    num_font: Option<String>,
    /// 文案风格，和「通知」分开写是因为它在界面上属于「通知与文案」那一组
    copy_style: Option<String>,

    // ---- 行为 ----
    autostart: Option<bool>,
    silent_start: Option<bool>,
    snap_px: Option<u32>,
    /// 启动摆主屏
    monitor_primary: Option<bool>,
    idle_detect: Option<bool>,
    idle_min: Option<u32>,
    close_action: Option<String>,
    hk_enabled: Option<bool>,
    hk_show: Option<String>,
    hk_passthru: Option<String>,
    hk_clock: Option<String>,
    hk_extra: Option<String>,

    // ---- 通知 ----
    notify_end: Option<String>,
    notify_lunch: Option<bool>,
    notify_lunch_before: Option<u32>,
    notify_sit: Option<bool>,
    notify_sit_min: Option<u32>,
    notify_dnd: Option<bool>,
    notify_dnd_from: Option<String>,
    notify_dnd_to: Option<String>,

    /// 清除数据：`today` / `month` / `all`。
    ///
    /// 前端负责「输入 DELETE 才放行」那一关，后端不做二次确认——
    /// 但后端也不接受空字符串，避免一个手滑的 `clear: ""` 把数据清光。
    clear: Option<String>,
}

/// 「加一天 / 删一天」。节假日和调休补班共用。
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DayOp {
    /// `YYYY-MM-DD`
    add: Option<String>,
    del: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SchedPatch {
    start: Option<String>,
    end: Option<String>,
    lunch1: Option<String>,
    lunch2: Option<String>,
}

fn round_u32(v: f64, lo: f64, hi: f64) -> u32 {
    if !v.is_finite() {
        return lo as u32;
    }
    v.clamp(lo, hi).round() as u32
}

fn round_i64(v: f64, lo: f64, hi: f64) -> i64 {
    if !v.is_finite() {
        return lo as i64;
    }
    v.clamp(lo, hi).round() as i64
}

// ---------------------------------------------------------------- 命令

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    view(&app)
}

/// 把一次补丁合并进配置。
///
/// 抽成纯函数是为了能测：这里每一行都可能悄悄错一格（月份写成 1000 倍、
/// 作息推导盖掉手工工时、请假日期加到昨天），而它们的表现都是「数字不对」，
/// 从界面上看不出来。`today` 从参数进来，测试里就能摆到任意一天。
fn merge(s: &mut Settings, patch: &Patch, today: NaiveDate) {
    if let Some(v) = patch.month_yuan {
        s.month_milli = round_i64(v * 1000.0, 1.0, crate::settings::MONTH_MAX as f64);
    }
    if let Some(v) = patch.pay_days_centi {
        s.pay_days_centi = v;
    }

    // 作息先合并，再把推导出的计薪工时写进 daily_sec；
    // 顺序很重要——反过来的话手工填的工时会被作息推导覆盖掉。
    if let Some(p) = &patch.sched {
        let mut cfg = s.sched;
        if let Some(v) = p.start.as_deref().and_then(parse_hm) {
            cfg.start_min = v;
        }
        if let Some(v) = p.end.as_deref().and_then(parse_hm) {
            cfg.end_min = v;
        }
        if let Some(v) = p.lunch1.as_deref().and_then(parse_hm) {
            cfg.lunch_start_min = v;
        }
        if let Some(v) = p.lunch2.as_deref().and_then(parse_hm) {
            cfg.lunch_end_min = v;
        }
        cfg.sanitize();
        s.sched = cfg;
        s.daily_sec = cfg.paid_sec();
    }
    // 手工覆盖：原型 04 章那句「由作息自动推导，此处可手工覆盖」。
    // 同一次补丁里既改作息又改工时（不会发生，但别留一个说不清的分支）——
    // 以工时为准，因为那总是用户最后一下动作。
    if let Some(v) = patch.daily_hours {
        s.daily_sec = round_u32(v * 3600.0, 60.0, 86_400.0);
    }

    if let Some(v) = patch.ot_paid {
        s.ot_mode = if v { OtMode::Paid } else { OtMode::Shadow };
    }
    if let Some(v) = &patch.accent {
        s.accent = v.clone();
    }
    if let Some(v) = patch.opacity {
        s.opacity = v;
    }
    if let Some(v) = patch.density {
        s.density = v;
    }
    if let Some(v) = &patch.eq_name {
        s.eq_name = v.clone();
    }
    if let Some(v) = patch.eq_price_yuan {
        s.eq_price_centi = round_u32(v * 100.0, 1.0, 100_000_000.0);
    }
    if let Some(v) = patch.eq_goal_cups {
        s.eq_goal_cups = v;
    }
    if let Some(v) = patch.eq_day_cups {
        s.eq_day_cups = v;
    }
    if let Some(v) = &patch.leave_kind {
        s.leave_kind = v.clone();
    }
    if let Some(v) = patch.leave_left_days {
        s.leave_left_centi = round_u32(v * 100.0, 0.0, 36_500.0);
    }
    if let Some(v) = &patch.size {
        s.size = match v.as_str() {
            "compact" => Size::Compact,
            "expanded" => Size::Expanded,
            _ => Size::Normal,
        };
    }
    if let Some(on) = patch.click_through {
        s.click_through = on;
    }
    // ---- 排班 ----
    if let Some(days) = patch.week_days {
        s.week.days = days;
    }
    if let Some(v) = patch.alt_sat {
        // -1 是「关掉大小周」。用一个负数而不是 null：
        // 前端 JSON 里 `null` 和「没传」长得一样，分不出来。
        s.week.alt_sat = if v < 0 { None } else { Some((v as u8).min(1)) };
    }
    if let Some(v) = patch.flex_start {
        s.week.flex = v;
        // 关掉弹性就把锚点清掉：留着它，下次再开启会把今天钉在
        // 「上一次开启时那个活动时刻」上——那个点早就不对了，金额会当场跳。
        if !v {
            s.day_anchor = None;
        }
    }
    if let Some(f) = &patch.work_form {
        // 形态只是三个预设，点下去就把七个格子改写掉。
        // 「下一个周六上不上」由今天所在周的奇偶决定——不这样的话
        // 用户点完大小周会看到「本周六不上班」，与直觉相反。
        match f.as_str() {
            "five" => {
                s.week.days = [true, true, true, true, true, false, false];
                s.week.alt_sat = None;
            }
            "six" => {
                s.week.days = [true, true, true, true, true, true, false];
                s.week.alt_sat = None;
            }
            "bigsmall" => {
                s.week.days = [true, true, true, true, true, true, false];
                s.week.alt_sat = Some(paypulse_clock::week_parity(today));
            }
            _ => {}
        }
    }
    // 加一天时要顺手把它从另一张表里删掉：
    // 同一天既「放假」又「补班」是自相矛盾的配置，不该让用户自己收拾。
    if let Some(op) = &patch.holiday {
        if let Some(d) = &op.add {
            s.week.holidays.push(d.clone());
            s.week.makeup.retain(|x| x != d);
        }
        if let Some(d) = &op.del {
            s.week.holidays.retain(|x| x != d);
        }
    }
    if let Some(op) = &patch.makeup {
        if let Some(d) = &op.add {
            s.week.makeup.push(d.clone());
            s.week.holidays.retain(|x| x != d);
        }
        if let Some(d) = &op.del {
            s.week.makeup.retain(|x| x != d);
        }
    }
    if let Some(v) = patch.cross_midnight {
        s.sched.cross_midnight = v;
    }

    // ---- 年终奖 / 口径 ----
    if let Some(m) = &patch.bonus_mode {
        s.bonus.mode = match m.as_str() {
            "months" => BonusMode::Months,
            "amount" => BonusMode::Amount,
            "rate" => BonusMode::Rate,
            _ => BonusMode::None,
        };
    }
    if let Some(v) = patch.bonus_months {
        s.bonus.months_x100 = round_u32(v * 100.0, 0.0, 1_200.0);
    }
    if let Some(v) = patch.bonus_amount_yuan {
        s.bonus.amount_milli = round_i64(v * 1000.0, 0.0, crate::settings::MONTH_MAX as f64);
    }
    if let Some(v) = patch.bonus_rate_pct {
        s.bonus.rate_x100 = round_u32(v * 100.0, 0.0, 50_000.0);
    }
    if let Some(v) = patch.gross {
        s.gross = v;
    }
    if let Some(v) = patch.tz_fixed {
        s.tz_fixed = v;
    }

    // ---- 外观 ----
    if let Some(v) = &patch.theme {
        s.theme = match v.as_str() {
            "black" => Theme::Black,
            "light" => Theme::Light,
            "system" => Theme::System,
            _ => Theme::Dark,
        };
    }
    if let Some(v) = &patch.num_font {
        s.num_font = match v.as_str() {
            "jb" => crate::settings::NumFont::JetBrains,
            "sys" => crate::settings::NumFont::System,
            _ => crate::settings::NumFont::SfMono,
        };
    }
    if let Some(v) = &patch.copy_style {
        s.copy_style = match v.as_str() {
            "plain" => crate::settings::CopyStyle::Plain,
            "silent" => crate::settings::CopyStyle::Silent,
            _ => crate::settings::CopyStyle::Emotional,
        };
    }

    // ---- 行为 ----
    if let Some(v) = patch.autostart {
        s.autostart = v;
    }
    if let Some(v) = patch.silent_start {
        s.silent_start = v;
    }
    if let Some(v) = patch.snap_px {
        s.snap_px = v;
    }
    if let Some(v) = patch.monitor_primary {
        s.monitor_primary = v;
    }
    if let Some(v) = patch.idle_detect {
        s.idle_detect = v;
    }
    if let Some(v) = patch.idle_min {
        s.idle_min = v;
    }
    if let Some(v) = &patch.close_action {
        s.close_action = if v == "quit" {
            CloseAction::Quit
        } else {
            CloseAction::Tray
        };
    }
    if let Some(v) = patch.hk_enabled {
        s.hotkey.enabled = v;
    }
    if let Some(v) = &patch.hk_show {
        s.hotkey.show = v.trim().to_string();
    }
    if let Some(v) = &patch.hk_passthru {
        s.hotkey.passthru = v.trim().to_string();
    }
    if let Some(v) = &patch.hk_clock {
        s.hotkey.clock = v.trim().to_string();
    }
    if let Some(v) = &patch.hk_extra {
        s.hotkey.extra = v.trim().to_string();
    }

    // ---- 通知 ----
    if let Some(v) = &patch.notify_end {
        s.notify.end = match v.as_str() {
            "off" => NotifyEnd::Off,
            "amountwork" => NotifyEnd::AmountWork,
            "amounteq" => NotifyEnd::AmountEq,
            _ => NotifyEnd::Amount,
        };
    }
    if let Some(v) = patch.notify_lunch {
        s.notify.lunch = v;
    }
    if let Some(v) = patch.notify_lunch_before {
        s.notify.lunch_before_min = v;
    }
    if let Some(v) = patch.notify_sit {
        s.notify.sit = v;
    }
    if let Some(v) = patch.notify_sit_min {
        s.notify.sit_min = v;
    }
    if let Some(v) = patch.notify_dnd {
        s.notify.dnd = v;
    }
    if let Some(v) = patch.notify_dnd_from.as_deref().and_then(parse_hm) {
        s.notify.dnd_from_min = v;
    }
    if let Some(v) = patch.notify_dnd_to.as_deref().and_then(parse_hm) {
        s.notify.dnd_to_min = v;
    }

    // ---- 清除数据 ----
    //
    // 「全部」保留窗口位置和外观：用户想清的是**账**，不是「把悬浮窗扔回
    // 屏幕右上角」。位置和主题不在这笔账里。
    match patch.clear.as_deref() {
        Some("today") => {
            s.pauses.clear();
            s.lunch_ots.clear();
            s.ot_start = None;
            // 账本也要清：只清计时不清进账，用户看到「本月已到账」还是有那笔
            let iso = today.format("%Y-%m-%d").to_string();
            s.extras.retain(|e| e.date != iso);
        }
        Some("month") => {
            let (y, m) = (today.year_ce().1, today.month());
            let ym = today.format("%Y-%m").to_string();
            s.leave_days.retain(|d| match NaiveDate::parse_from_str(d, "%Y-%m-%d") {
                Ok(d) => !(d.year() == y as i32 && d.month() == m),
                Err(_) => false,
            });
            s.extras.retain(|e| e.date.get(..7) != Some(ym.as_str()));
            s.pauses.clear();
            s.lunch_ots.clear();
            s.ot_start = None;
        }
        Some("all") => {
            let win = s.win;
            let size = s.size;
            let theme = s.theme;
            let accent = s.accent.clone();
            let opacity = s.opacity;
            let num_font = s.num_font;
            let copy_style = s.copy_style;
            let tz_fixed = s.tz_fixed;
            let monitor_primary = s.monitor_primary;
            *s = Settings::default();
            s.win = win;
            s.size = size;
            s.theme = theme;
            s.accent = accent;
            s.opacity = opacity;
            s.num_font = num_font;
            // 文案风格和字体一样是「长相」，不是账：清数据不该把说话方式也清掉
            s.copy_style = copy_style;
            s.tz_fixed = tz_fixed;
            // 「摆在哪块屏」也是偏好，不是账
            s.monitor_primary = monitor_primary;
        }
        _ => {}
    }

    if let Some(on) = patch.today_leave {
        let key = today.to_string();
        let has = s.leave_days.iter().any(|d| *d == key);
        if on && !has {
            s.leave_days.push(key);
        } else if !on && has {
            s.leave_days.retain(|d| *d != key);
        }
    }
}

/// 合并一次改动，返回合并后的完整视图（含重新算过的折算值）。
#[tauri::command]
pub fn apply_settings(app: AppHandle, patch: Patch) -> SettingsView {
    let st = app.state::<AppState>();
    let before_size = st.settings().size;
    // 「今天」也按配置的时区读：切换时区那一刻不该让请假记录跳到另一天。
    // 用改动前的配置是刻意的——同一次补丁里既改时区又改请假时，
    // 「今天」应该还是原来那个今天。
    let today = crate::clock(&st.settings()).date_naive();

    st.mutate(|s| merge(s, &patch, today));

    // 尺寸改了要立刻改悬浮窗宽度：等下一帧的话，中间那帧窗口比内容宽，
    // 多出来的透明区域会吃掉桌面上的一次点击。
    let after = st.settings();
    if after.size != before_size {
        if let Some(win) = app.get_webview_window(crate::HUD_LABEL) {
            crate::apply_width(&win, after.size);
        }
    }

    // 穿透是**窗口属性**，不是画出来的：改完必须真的落到窗口上。
    // 漏了这一步，用户在设置里打开穿透会以为没生效，然后在悬浮窗上照样点得动。
    if patch.click_through.is_some() {
        if let Some(win) = app.get_webview_window(crate::HUD_LABEL) {
            let _ = win.set_ignore_cursor_events(after.click_through);
        }
    }
    // 开机自启是**系统状态**，不是我们的一个字段：改完必须真的写进注册表。
    // 只写了配置不写注册表，用户会看到开关是开的、开机却没起来——
    // 这种「看起来生效了」的谎最难查。
    if patch.autostart.is_some() {
        match std::env::current_exe() {
            Ok(exe) => {
                if let Err(e) = crate::platform::set_autostart(after.autostart, &exe) {
                    eprintln!("设置开机自启失败：{e}");
                }
            }
            Err(e) => eprintln!("拿不到程序路径，开机自启没写进去：{e}"),
        }
    }
    // 快捷键同理：改完要重新注册，否则旧的那个还在抢按键。
    //
    // `hk_passthru` 原来漏在这儿了：改了「鼠标穿透」的键位，配置写了、界面也变了，
    // 但注册的还是旧键——用户按新键没反应、按旧键却还灵，最容易判成「快捷键坏了」。
    if patch.hk_enabled.is_some()
        || patch.hk_show.is_some()
        || patch.hk_passthru.is_some()
        || patch.hk_clock.is_some()
        || patch.hk_extra.is_some()
    {
        crate::apply_hotkeys(&app);
    }

    // 托盘勾选和这里读的是同一份配置，任何一次改动都刷一遍，
    // 免得出现「设置里开着、托盘没勾」的分裂。
    crate::tray::sync_checks(&app);

    // 首启：第一次写盘之后悬浮窗才露面。
    // 之前它是藏着的——让人先看到「示例 8000 块」在那儿跳，再看到他自己的数，
    // 那半秒足够让人认定这软件在瞎算。`take_fresh` 保证只露这一次。
    if st.take_fresh() {
        if let Some(win) = app.get_webview_window(crate::HUD_LABEL) {
            let _ = win.show();
            crate::clamp_into_screen(&win);
        }
        crate::tray::sync_checks(&app);
    }

    view(&app)
}

/// 打开（或唤到前台）设置窗口。
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }

    // 正常情况窗口已在启动时预热（prewarm），这里只该拿到现成的。
    // 万一没建好，退回去现建一个——内容可能空白，但至少窗口结构是对的。
    crate::log::w("设置窗口：没预热，现建（内容可能空白）");
    let _ = build(app)?;
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

/// 启动时把设置窗口建好藏着：和通知窗、右键菜单窗、今日明细窗一样，
/// 懒创建的透明窗口第一次 `show()` 是空白的（WebView2 还没渲染首帧），
/// 先建好让页面加载一次，用的时候只 `show()`。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Ok(_w) => crate::log::w("设置窗口已预热（藏起）"),
        Err(e) => crate::log::w(&format!("设置窗口预热失败：{e}")),
    }
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let w = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("设置 · 牛马薪动了！")
    .inner_size(980.0, 720.0)
    // 去掉系统标题栏：Win11 的原生标题栏是浅色的，盖在这张深色窗口上像贴了张纸。
    // 代价有两个，都自己补：阴影画在 .app 上，尺寸锁死——
    // 没有标题栏的窗口没有可抓的边框，「能拉」只是一个拉不动的谎。
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .center()
    // 建的时候就藏着：配合 prewarm 让页面先加载，打开时只 show。
    .visible(false)
    .focused(false)
    .build()?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_strings_parse_both_ways() {
        assert_eq!(parse_hm("09:00"), Some(540));
        assert_eq!(parse_hm("18:30"), Some(1110));
        assert_eq!(parse_hm(" 7:05 "), Some(425));
        // 凌晨 0 点是个合法值，别被「0 是假值」的思路当成空
        assert_eq!(parse_hm("00:00"), Some(0));

        // 坏输入一律 None，不 panic
        assert_eq!(parse_hm(""), None);
        assert_eq!(parse_hm("9点"), None);
        assert_eq!(parse_hm("25:00"), None);
        assert_eq!(parse_hm("09:61"), None);

        assert_eq!(hm(540), "09:00");
        assert_eq!(hm(1110), "18:30");
        assert_eq!(hm(0), "00:00");
    }

    /// 时薪必须是四舍五入，不是截断——49.038 截成 49.03 会和悬浮窗对不上。
    #[test]
    fn hourly_rounds_half_up() {
        // 367.82 元 / 7.5h = 49.0426…，两位小数 → 49.04
        assert_eq!(hourly(367_816, 27_000), 49_042);
        // 8000 / 21.75 = 367.81609…，取整到毫 → 367_816
        assert_eq!(hourly(367_816, 27_000) * 27_000 / 3_600, 367_815);
        // 除数是 0 时给 0 而不是 panic
        assert_eq!(hourly(367_816, 0), 0);
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn patched(p: Patch) -> Settings {
        let mut s = Settings::default();
        merge(&mut s, &p, day(2026, 11, 10));
        s.validate();
        s
    }

    /// 元 → 毫：8000.00 元必须是 8_000_000 毫（差一个零就是「月薪八百万」）。
    #[test]
    fn yuan_is_converted_to_milli_once() {
        let s = patched(Patch { month_yuan: Some(8000.0), ..Default::default() });
        assert_eq!(s.month_milli, 8_000_000);
        // 两位小数要留住：8000.50 元
        let s = patched(Patch { month_yuan: Some(8000.5), ..Default::default() });
        assert_eq!(s.month_milli, 8_000_500);
    }

    /// 改作息要顺手把计薪工时推导出来——原型那行「已同步到薪资与口径」。
    #[test]
    fn schedule_edit_rederives_paid_hours() {
        let s = patched(Patch {
            sched: Some(SchedPatch {
                start: Some("09:30".into()),
                end: Some("19:00".into()),
                lunch1: Some("12:00".into()),
                lunch2: Some("13:00".into()),
            }),
            ..Default::default()
        });
        assert_eq!(s.sched.start_min, 570);
        assert_eq!(s.sched.end_min, 1140);
        // 9.5h − 1h = 8.5h
        assert_eq!(s.daily_sec, 30_600);
    }

    /// 只改一个时刻时，另外三个不能被顺手重置。
    #[test]
    fn partial_schedule_patch_keeps_the_rest() {
        let s = patched(Patch {
            sched: Some(SchedPatch {
                end: Some("19:00".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(s.sched.start_min, 540, "上班时间不该被改");
        assert_eq!(s.sched.lunch_start_min, 720);
        assert_eq!(s.sched.lunch_end_min, 810);
        assert_eq!(s.sched.end_min, 1140);
    }

    /// 手工填的工时优先于作息推导（原型：可手工覆盖）。
    #[test]
    fn manual_hours_win_over_derivation() {
        let s = patched(Patch {
            sched: Some(SchedPatch {
                start: Some("10:00".into()),
                ..Default::default()
            }),
            daily_hours: Some(6.0),
            ..Default::default()
        });
        assert_eq!(s.daily_sec, 21_600, "手工的 6h 要盖掉推导出的 7.5h");
    }

    /// 坏时刻不该把整份配置带下去：认不出来就跳过这一项。
    #[test]
    fn garbage_time_is_ignored() {
        let s = patched(Patch {
            sched: Some(SchedPatch {
                start: Some("".into()),
                end: Some("25:99".into()),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(s.sched, crate::settings::SchedCfg::default(), "原样保留");
    }

    /// 「今天请假」是开关语义：点两次回到原样，不会留下两条记录。
    #[test]
    fn today_leave_toggles_without_duplicating() {
        let on = Patch { today_leave: Some(true), ..Default::default() };
        let mut s = Settings::default();
        merge(&mut s, &on, day(2026, 11, 10));
        merge(&mut s, &on, day(2026, 11, 10));
        assert_eq!(s.leave_days.len(), 1, "同一天不能记两条");

        let off = Patch { today_leave: Some(false), ..Default::default() };
        merge(&mut s, &off, day(2026, 11, 10));
        assert!(s.leave_days.is_empty());
    }

    /// 请假记的是**哪一天**，不是「现在请假」——换一天再点不能把昨天也标上。
    #[test]
    fn leave_is_recorded_for_the_given_day_only() {
        let mut s = Settings::default();
        merge(&mut s, &Patch { today_leave: Some(true), ..Default::default() }, day(2026, 11, 10));
        merge(&mut s, &Patch { today_leave: Some(true), ..Default::default() }, day(2026, 11, 11));
        assert_eq!(s.leave_days, vec!["2026-11-10", "2026-11-11"]);

        // 取消「今天」只取消 11-11
        merge(&mut s, &Patch { today_leave: Some(false), ..Default::default() }, day(2026, 11, 11));
        assert_eq!(s.leave_days, vec!["2026-11-10"]);
    }

    /// 补丁里没提到的字段必须原样留着。
    /// 这是「逐项 Option」而非「整份 Settings」的全部理由：
    /// 前端根本不展示 pauses / win，整份写回去就会把它们清空。
    #[test]
    fn patch_never_touches_what_it_does_not_mention() {
        let mut s = Settings::default();
        s.win = Some(crate::settings::WinPos { x: 100, y: 20 });
        s.pauses.push(crate::settings::PauseRec { start: 10, end: Some(20) });
        s.leave_days = vec!["2026-11-03".to_string()];

        merge(&mut s, &Patch { accent: Some("#FF9F45".into()), ..Default::default() }, day(2026, 11, 10));

        assert_eq!(s.accent, "#FF9F45");
        assert_eq!(s.win, Some(crate::settings::WinPos { x: 100, y: 20 }));
        assert_eq!(s.pauses.len(), 1);
        assert_eq!(s.leave_days, vec!["2026-11-03"]);
    }

    /// 不透明度、强调色这种「直接进 CSS」的值也要过校验。
    #[test]
    fn display_values_are_sanitized() {
        let s = patched(Patch { opacity: Some(5), accent: Some("red;}".into()), ..Default::default() });
        assert_eq!(s.opacity, 50, "太透会看不见");
        assert_eq!(s.accent, "#35E07F", "非 #RRGGBB 一律退回品牌绿");

        let s = patched(Patch { opacity: Some(140), ..Default::default() });
        assert_eq!(s.opacity, 100);
    }

    #[test]
    fn absurd_input_is_clamped_not_rejected() {
        assert_eq!(round_u32(f64::NAN, 1.0, 100.0), 1);
        assert_eq!(round_u32(-5.0, 1.0, 100.0), 1);
        assert_eq!(round_u32(1e12, 1.0, 100.0), 100);
        assert_eq!(round_i64(f64::INFINITY, 1.0, 10.0), 1);
    }

    // ---- 排班 ----

    /// 三个预设各改一遍七个格子。大小周还要把「本周六上不上」按今天所在周定下来，
    /// 否则用户点完会看到「本周六不上班」，与「我选的是大小周」直接冲突。
    #[test]
    fn work_form_rewrites_the_week() {
        let five = patched(Patch { work_form: Some("five".into()), ..Default::default() });
        assert_eq!(five.week.days, [true, true, true, true, true, false, false]);
        assert_eq!(five.week.alt_sat, None);

        let six = patched(Patch { work_form: Some("six".into()), ..Default::default() });
        assert_eq!(six.week.days, [true, true, true, true, true, true, false]);

        let today = day(2026, 11, 10);
        let big = patched(Patch { work_form: Some("bigsmall".into()), ..Default::default() });
        assert_eq!(big.week.days[5], true, "大小周的周六得是「上」");
        assert_eq!(big.week.alt_sat, Some(paypulse_clock::week_parity(today)));
        assert_eq!(big.week.form(), crate::settings::WorkForm::BigSmall);
    }

    /// 手改七天格子之后，形态标签自己变成「自定义」——
    /// 这一项刻意不存字段，存了就会和 days 打架。
    #[test]
    fn hand_edited_days_become_custom() {
        let s = patched(Patch {
            week_days: Some([true, false, true, false, true, false, false]),
            ..Default::default()
        });
        assert_eq!(s.week.form(), crate::settings::WorkForm::Custom);
    }

    /// 同一天不能既是「放假」又是「补班」：加一天时把它从另一张表里删掉。
    #[test]
    fn a_day_lives_in_exactly_one_table() {
        let mut s = Settings::default();
        merge(&mut s, &Patch { holiday: Some(DayOp { add: Some("2026-12-25".into()), ..Default::default() }), ..Default::default() }, day(2026, 11, 10));
        assert_eq!(s.week.holidays, vec!["2026-12-25"]);

        merge(&mut s, &Patch { makeup: Some(DayOp { add: Some("2026-12-25".into()), ..Default::default() }), ..Default::default() }, day(2026, 11, 10));
        assert!(s.week.holidays.is_empty(), "标成补班后，放假表里不该还有它");
        assert_eq!(s.week.makeup, vec!["2026-12-25"]);

        merge(&mut s, &Patch { makeup: Some(DayOp { del: Some("2026-12-25".into()), ..Default::default() }), ..Default::default() }, day(2026, 11, 10));
        assert!(s.week.makeup.is_empty());
        assert!(s.week.holidays.is_empty(), "取消标记不该把它挪到另一张表");
    }

    // ---- 年终奖 / 口径 ----

    #[test]
    fn bonus_months_are_stored_as_centi() {
        // 只改月数不改口径的话，mode 还是「不计入」——那种组合算出来必须是 0，
        // 所以这里两样一起改，和界面上点完 chip 再填数字的顺序一致。
        let s = patched(Patch {
            bonus_mode: Some("months".into()),
            bonus_months: Some(3.0),
            ..Default::default()
        });
        assert_eq!(s.bonus.months_x100, 300, "3 个月 = 300，就是 15 薪");
        // 摊到每月 = 月薪 × 3 / 12 = 2000 元
        assert_eq!(s.bonus.per_month_milli(s.month_milli), 2_000_000);
        assert_eq!(s.bonus.total_milli(s.month_milli), 24_000_000);
    }

    /// 只改数字、没选口径时，一分钱都不该多出来。
    #[test]
    fn bonus_number_without_a_mode_changes_nothing() {
        let s = patched(Patch { bonus_months: Some(3.0), ..Default::default() });
        assert_eq!(s.bonus.mode, crate::settings::BonusMode::None);
        assert_eq!(s.bonus.per_month_milli(s.month_milli), 0);
    }

    /// 三种口径摊出来的月增量都该等于「总额 ÷ 12」，不该各算一套。
    #[test]
    fn every_bonus_mode_spreads_the_same_way() {
        let mut s = Settings::default();
        s.bonus.mode = crate::settings::BonusMode::Amount;
        s.bonus.amount_milli = 24_000_000; // 24000 元
        assert_eq!(s.bonus.per_month_milli(s.month_milli), 2_000_000);

        s.bonus.mode = crate::settings::BonusMode::Rate;
        s.bonus.rate_x100 = 2_500; // 年薪的 25%
        // 年薪 = 8000 × 12 = 96000，25% = 24000
        assert_eq!(s.bonus.total_milli(s.month_milli), 24_000_000);
        assert_eq!(s.bonus.per_month_milli(s.month_milli), 2_000_000);
    }

    #[test]
    fn gross_only_renames_nothing_else() {
        let a = patched(Patch { gross: Some(true), ..Default::default() });
        let b = patched(Patch { gross: Some(false), ..Default::default() });
        assert_eq!(a.month_milli, b.month_milli);
        assert_eq!(a.daily_sec, b.daily_sec);
    }

    /// 文案风格只换语气：三档之下，钱和工时一个字节都不许动。
    #[test]
    fn copy_style_changes_nothing_but_words() {
        let a = patched(Patch { copy_style: Some("emotional".into()), ..Default::default() });
        let b = patched(Patch { copy_style: Some("silent".into()), ..Default::default() });
        assert_eq!(b.copy_style, crate::settings::CopyStyle::Silent);
        assert_eq!(a.month_milli, b.month_milli);
        assert_eq!(a.daily_sec, b.daily_sec);
        assert_eq!(a.bonus, b.bonus);
    }

    // ---- 清除数据 ----

    /// 「清空全部」清的是账，不是外观：主题、强调色、不透明度、字体、
    /// 时区、窗口位置都留着——用户想删的是数据，不是把悬浮窗扔回右上角。
    #[test]
    fn clearing_everything_keeps_the_look() {
        let mut s = Settings::default();
        s.theme = crate::settings::Theme::Black;
        s.num_font = crate::settings::NumFont::JetBrains;
        s.copy_style = crate::settings::CopyStyle::Silent;
        s.accent = "#FF9F45".into();
        s.opacity = 80;
        s.tz_fixed = true;
        s.win = Some(crate::settings::WinPos { x: 100, y: 20 });
        s.pauses.push(crate::settings::PauseRec { start: 10, end: Some(20) });
        s.leave_days = vec!["2026-11-03".to_string()];
        s.month_milli = 20_000_000;

        merge(&mut s, &Patch { clear: Some("all".into()), ..Default::default() }, day(2026, 11, 10));

        assert_eq!(s.theme, crate::settings::Theme::Black);
        assert_eq!(s.num_font, crate::settings::NumFont::JetBrains);
        assert_eq!(s.copy_style, crate::settings::CopyStyle::Silent);
        assert_eq!(s.accent, "#FF9F45");
        assert_eq!(s.opacity, 80);
        assert!(s.tz_fixed, "时区是偏好，不是账");
        assert_eq!(s.win, Some(crate::settings::WinPos { x: 100, y: 20 }));

        assert!(s.pauses.is_empty());
        assert!(s.leave_days.is_empty());
        assert_eq!(s.month_milli, 8_000_000, "月薪回到默认值");
    }

    /// 「清除本月」只清本月的请假：上个月的记录和窗口位置都不动。
    #[test]
    fn clearing_the_month_keeps_other_months() {
        let mut s = Settings::default();
        s.leave_days = vec!["2026-10-31".into(), "2026-11-03".into(), "2026-12-01".into()];
        s.pauses.push(crate::settings::PauseRec { start: 10, end: Some(20) });

        merge(&mut s, &Patch { clear: Some("month".into()), ..Default::default() }, day(2026, 11, 10));
        assert_eq!(s.leave_days, vec!["2026-10-31", "2026-12-01"]);
        assert!(s.pauses.is_empty());
    }

    /// 清账要连「记一笔」一起清，只清计时的话用户会看到那笔进账还在。
    #[test]
    fn clearing_today_and_month_takes_the_extra_entries_with_it() {
        let mk = |d: &str| crate::settings::ExtraEntry {
            date: d.into(),
            amount_milli: 1_000_000,
        };

        let mut s = Settings::default();
        s.extras = vec![mk("2026-10-31"), mk("2026-11-10"), mk("2026-11-03")];
        merge(&mut s, &Patch { clear: Some("today".into()), ..Default::default() }, day(2026, 11, 10));
        assert_eq!(
            s.extras.iter().map(|e| e.date.as_str()).collect::<Vec<_>>(),
            vec!["2026-10-31", "2026-11-03"],
            "只清今天那一笔"
        );

        let mut s = Settings::default();
        s.extras = vec![mk("2026-10-31"), mk("2026-11-10"), mk("2026-12-01")];
        merge(&mut s, &Patch { clear: Some("month".into()), ..Default::default() }, day(2026, 11, 10));
        assert_eq!(
            s.extras.iter().map(|e| e.date.as_str()).collect::<Vec<_>>(),
            vec!["2026-10-31", "2026-12-01"],
            "清本月，跨月的两笔留着"
        );

        let mut s = Settings::default();
        s.extras = vec![mk("2026-11-10")];
        merge(&mut s, &Patch { clear: Some("all".into()), ..Default::default() }, day(2026, 11, 10));
        assert!(s.extras.is_empty(), "全清");
    }

    /// 空字符串不该被当成一次「清除」——手滑传个 `clear: ""` 不能清数据。
    #[test]
    fn empty_clear_does_nothing() {
        let mut s = Settings::default();
        s.leave_days = vec!["2026-11-03".to_string()];
        merge(&mut s, &Patch { clear: Some("".into()), ..Default::default() }, day(2026, 11, 10));
        assert_eq!(s.leave_days, vec!["2026-11-03"]);
    }

    // ---- 行为 ----

    /// 吸附阈值 0 是「关掉」，不是「阈值零像素」——这两件事在实现上是同一个数，
    /// 但在界面上必须能被区分，所以 0 要能原样存下来，不能被夹成 1。
    #[test]
    fn snap_zero_means_off() {
        let s = patched(Patch { snap_px: Some(0), ..Default::default() });
        assert_eq!(s.snap_px, 0);
    }

    #[test]
    fn hotkeys_are_stored_trimmed() {
        let s = patched(Patch { hk_show: Some("  Ctrl+Alt+P ".into()), ..Default::default() });
        assert_eq!(s.hotkey.show, "Ctrl+Alt+P");
    }
}
