//! 「这段没开机的时间算不算工？」——规格 02-D 的「长时间未启动」。
//!
//! ## 这条规格在做什么（以及它为什么不能省）
//!
//! 这个程序的金额是**时间的纯函数**：`已赚 = f(现在)`，不看「跑了多少次 tick」。
//! 好处是合盖八小时醒来数字也对得上；副作用是——**程序关着的那段时间，
//! 只要落在作息区间里，这段时间也照样被算成工作**。
//!
//! 对「中午关机吃个饭」这是对的；对「出差一周没开机」就是凭空多出一个星期的钱。
//! 规格原文是「启动时按『上次退出时间』补算，弹一次确认：是否按 xx 补记，
//! 一键采纳或跳过」。所以这扇窗问的其实是那个副作用：
//!
//! > 从你上次退出到现在，中间有 2 小时 10 分在班次内。这段算不算？
//!
//! * 「算进去」= 什么都不做（现状就是算着的），只记下今天已经问过；
//! * 「不算」= 往 `pauses` 里补一段覆盖今天的区间，金额当场回落。
//!
//! ## 为什么只算今天
//!
//! 引擎里只有「今天」这一本账（`snapshot_at` 只回答今天的数）。
//! 三天前的缺口无论怎么处理，都不会影响界面上任何一个数字——
//! 把它算进一个不存在的地方，只是给自己造一个假的复杂度。
//!
//! ## 为什么一天只问一次
//!
//! 每天早上十点开机的人，每次启动都会触发一次「补记」询问。
//! 一天答一次是提醒，一天答三次就是纠缠。所以记 `fill_day`。

use chrono::{DateTime, FixedOffset, TimeZone};

use crate::settings::Settings;

/// 少于这个时长不问。
///
/// 用的是「班次内的时长」而不是「离上次退出多久」：关机一晚 15 小时，
/// 但落在班次内的是 0 分钟——那种情况不该弹窗。反过来，中午关机两小时、
/// 一点半回来开机，落在班次内的有 1.5 小时，这才是真该问的。
const ASK_OVER_SEC: i64 = 30 * 60;

/// 一次补记询问的内容。全部是算好的字符串，前端不做算术。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FillView {
    /// 「11-09 18:32」——上次还活着的时刻
    pub from_label: String,
    /// 「3 天 15 小时」——中间空了多久
    pub gap_label: String,
    /// 「2 小时 10 分」——其中落在班次内的部分
    pub covered_label: String,
    /// 「¥106.42」——这部分值多少钱
    pub amount_label: String,
    /// 上次退出时刻（Unix 秒）。点「不算」时要拿它补一段暂停，
    /// 页面不显示它，只把它原样传回来——比让前端自己算一个时间戳可靠。
    pub from_ts: i64,
}

/// 算一次。返回 `Some` = 值得停下来问。
///
/// 抽成纯函数是为了能测：这里的边界全是「几十分钟」和「哪天」，
/// 而错了的表现是「该问的没问」或「天天来问」——两种都不会在日志里留下痕迹。
pub fn plan(s: &Settings, now: DateTime<FixedOffset>, last: i64) -> Option<FillView> {
    let now_ts = now.timestamp();
    if last <= 0 || now_ts <= last {
        return None;
    }

    let today = now.date_naive();
    let today_iso = today.format("%Y-%m-%d").to_string();
    // 今天已经答过了：一天一次就够。
    if s.fill_day.as_deref() == Some(today_iso.as_str()) {
        return None;
    }
    // 休息日 / 请假：今天本来就不计薪，也就没有「算不算」这个问题。
    if !s.calendar().is_workday(today) || s.is_leave_day(today) {
        return None;
    }
    // 夜班的班次跨零点，交集要按「起点 → 终点」绕一圈算。这一版不支持，
    // 直接不问——比起算错一个数让用户按着错数做决定，不问更好。
    if s.sched.overnight() {
        return None;
    }

    let lo = crate::day_start(now);
    // 只关心今天那一截。上次退出是三天前也好、昨晚也好，都从 00:00 起算。
    let from = last.max(lo);
    let covered = covered_sec(s, from, now_ts, lo);
    if covered < ASK_OVER_SEC {
        return None;
    }

    let day_milli = crate::params(s).day_milli();
    let amount_milli = day_milli * covered as i64 / s.daily_sec.max(1) as i64;

    Some(FillView {
        from_label: stamp(now, last),
        gap_label: span_text(now_ts - last),
        covered_label: clock_text(covered),
        amount_label: crate::yuan(amount_milli, 2),
        from_ts: last,
    })
}

/// `from..now` 之间落在「今天这一班的计薪窗口」里的秒数。
///
/// 计薪窗口 = 班次区间减去午休。三个区间都是当天的分钟数，
/// 所以这里只做一次坐标平移（都改成「当天第几分钟」）再算交集。
fn covered_sec(s: &Settings, from: i64, now_ts: i64, lo: i64) -> i64 {
    let min = |t: i64| ((t - lo) / 60).clamp(0, 1440);
    let (a, b) = (min(from), min(now_ts));
    if b <= a {
        return 0;
    }
    let shift = overlap(a, b, s.sched.start_min as i64, s.sched.end_min as i64);
    let lunch = overlap(
        a,
        b,
        s.sched.lunch_start_min as i64,
        s.sched.lunch_end_min as i64,
    );
    // 午休在班次外时 `overlap` 是 0，减了也不会把班次时长减成负数
    // （`max(0)` 兜一道，手改配置把午休写到 23:00 也不会算出负数）。
    (shift - lunch).max(0) * 60
}

fn overlap(a: i64, b: i64, x: i64, y: i64) -> i64 {
    let hi = b.min(y);
    let lo = a.max(x);
    (hi - lo).max(0)
}

/// 「3 天 15 小时」/「2 小时 10 分」/「45 分」。
fn span_text(sec: i64) -> String {
    let m = (sec.max(0)) / 60;
    let (d, h, mi) = (m / 1440, (m % 1440) / 60, m % 60);
    if d > 0 {
        if h > 0 {
            format!("{d} 天 {h} 小时")
        } else {
            format!("{d} 天")
        }
    } else if h > 0 {
        if mi > 0 {
            format!("{h} 小时 {mi} 分")
        } else {
            format!("{h} 小时")
        }
    } else {
        format!("{mi} 分")
    }
}

/// 「2 小时 10 分」，和 `span_text` 同一个口径但只到小时。
fn clock_text(sec: i64) -> String {
    clock_hm(sec / 60)
}

fn clock_hm(min: i64) -> String {
    let (h, mi) = (min / 60, min % 60);
    if h == 0 {
        format!("{mi} 分")
    } else if mi == 0 {
        format!("{h} 小时")
    } else {
        format!("{h} 小时 {mi} 分")
    }
}

/// 时间戳 → `11-09 18:32`（不带年份：这是「上次」不是「历史」）。
fn stamp(now: DateTime<FixedOffset>, ts: i64) -> String {
    let dt = now
        .offset()
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or(now);
    format!("{} {}", dt.format("%m-%d"), dt.format("%H:%M"))
}

/// 用户在补记窗里点了哪个。
///
/// * `count`：算进去 —— 什么都不用做，现状就是算着的；
/// * `skip`：不算 —— 补一段暂停，把今天从「上次退出」到现在的这段扣掉。
#[tauri::command]
pub fn fill_answer(app: tauri::AppHandle, mode: String, from: i64) {
    mark_answered(&app, mode == "skip", from);
    crate::ask_win::close(&app);
}

/// 记下「今天问过了」。两条路（答了 / 关掉了）都得走这里。
///
/// `skip` + `from` 才会落一段暂停。`from <= 0` 表示调用方没带时刻（用户点了 ×），
/// 那种情况下只记账、不动金额——**点 × 的意思正是「别动我的数」**。
pub fn mark_answered(app: &tauri::AppHandle, skip: bool, from: i64) {
    use tauri::Manager;
    let now = crate::clock(&app.state::<crate::AppState>().settings());
    let now_ts = now.timestamp();
    let today = now.date_naive().format("%Y-%m-%d").to_string();
    let lo = crate::day_start(now);
    let start = from.max(lo).min(now_ts);

    app.state::<crate::AppState>().mutate(|s| {
        if skip && from > 0 && start < now_ts {
            // 只补今天那一截：更早的日子不在任何一本账里。
            s.pauses.push(crate::settings::PauseRec {
                start,
                end: Some(now_ts),
            });
        }
        s.fill_day = Some(today);
        s.last_alive = Some(now_ts);
    });

    crate::log::w(&format!(
        "补记询问已答：{}（{}）",
        if skip { "不算，记为暂停" } else { "算进去" },
        if skip && from > 0 {
            format!("扣掉 {} 秒", (now_ts - start).max(0))
        } else {
            "没动金额".to_string()
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 2026-11-11 是周三（工作日），11-14 是周六。
    fn at(day: u32, h: u32, m: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, day, h, m, 0)
            .unwrap()
    }

    fn base() -> Settings {
        Settings::default() // 09:00–18:00，午休 12:00–13:30
    }

    /// 昨晚下班后关机，今天十点半开机：班次内空了 1.5 小时，该问。
    #[test]
    fn overnight_gap_inside_the_shift_is_asked() {
        let s = base();
        let last = at(10, 23, 30).timestamp();
        let v = plan(&s, at(11, 10, 30), last).expect("该问");
        assert_eq!(v.covered_label, "1 小时 30 分");
        // (8000/21.75) × 1.5h ≈ ¥551.72/日 × 1.5/8 … 这里只验非空与格式
        assert!(v.amount_label.starts_with('¥'), "{}", v.amount_label);
        assert_eq!(v.gap_label, "11 小时");
    }

    /// 刚重启一下（十几分钟）：不问。这条是「别把正常重启变成打扰」。
    #[test]
    fn short_restart_is_not_asked() {
        let s = base();
        let last = at(11, 10, 5).timestamp();
        assert!(plan(&s, at(11, 10, 20), last).is_none());
    }

    /// 周六本来就不计薪，问「算不算工」没有意义。
    #[test]
    fn rest_day_is_not_asked() {
        let s = base();
        let last = at(13, 20, 0).timestamp();
        assert!(plan(&s, at(14, 11, 0), last).is_none());
    }

    /// 请假当天同理。
    #[test]
    fn leave_day_is_not_asked() {
        let mut s = base();
        s.leave_days = vec!["2026-11-11".to_string()];
        let last = at(10, 23, 0).timestamp();
        assert!(plan(&s, at(11, 11, 0), last).is_none());
    }

    /// 今天已经答过一次：不再问。否则一天重启三次就问三次。
    #[test]
    fn answers_once_per_day() {
        let mut s = base();
        s.fill_day = Some("2026-11-11".to_string());
        let last = at(10, 23, 0).timestamp();
        assert!(plan(&s, at(11, 11, 0), last).is_none());
        // 换一天就该问了
        assert!(plan(&s, at(12, 11, 0), last).is_some());
    }

    /// 关机一整周再开机：只算今天那一截，不给「七天工资」。
    #[test]
    fn a_week_off_counts_only_today() {
        let s = base();
        let last = at(4, 18, 30).timestamp();
        let v = plan(&s, at(11, 10, 0), last).expect("该问");
        assert_eq!(v.covered_label, "1 小时");
        assert_eq!(v.gap_label, "6 天 15 小时");
    }

    /// 开机时还没到上班点：班次内 0 分钟，不问。
    ///
    /// 这条是最容易漏的：晚上关机、早上七点开机，`now - last` 有十几个小时，
    /// 但一分钱都不该补。
    #[test]
    fn before_the_shift_starts_there_is_nothing_to_ask() {
        let s = base();
        let last = at(10, 18, 30).timestamp();
        assert!(plan(&s, at(11, 8, 0), last).is_none());
    }

    /// 午休那一小时不算班次内：11:30 关机到 13:00 开机只有 30 分钟在班次内。
    #[test]
    fn lunch_is_excluded_from_the_covered_time() {
        let s = base();
        let last = at(11, 11, 30).timestamp();
        // 11:30–12:00 = 30 分钟（班次内）→ 刚好到阈值
        let v = plan(&s, at(11, 13, 0), last).expect("30 分钟该问");
        assert_eq!(v.covered_label, "30 分");
    }
}
