//! 墙上时钟 → 已计薪秒数。
//!
//! 这是整个产品里**最容易算错**的一段，而且错了没人看得出来：
//! 午休少扣一分钟，一天就多给一分钟的钱，一年下来对不上账。
//! 所以它被单独拆出来，做成不依赖系统时钟的纯函数，可以拿固定时刻喂断言。
//!
//! 它刻意不属于引擎（`paypulse-core`）：引擎只认「已计薪秒」，
//! 不认「几点几分」。时钟口径属于外壳层，改了不影响金额公式。
//!
//! ## 两件曾经是「常量」的事，现在都是输入
//!
//! - **哪几天算上班**：以前写死「周一到周五」，于是单休、大小周的人打开它，
//!   周六那天的钱永远是 0。现在是 `Calendar::days` 七位掩码 + 大小周的奇偶周六
//!   + 法定节假日 / 调休补班两张日期表。
//! - **上下班几点**：以前 `Schedule` 里只有上班时刻，下班时刻没进来，
//!   所以算不出「距下班」。现在端点、午休、跨零点都在 `Shift` 里。
//!
//! ## 金额仍然是纯函数
//!
//! 这里没有任何累加：`paid_seconds_at` 只回答「从今天开工到现在，该计薪多少秒」。
//! 合盖 8 小时再唤醒，它给的是真实经过的时间，而不是「多跑了多少次 tick」。

use chrono::{Datelike, NaiveDate, TimeZone, Timelike, Weekday};

/// 一天里的作息，单位是「当天第几分钟」（0–1439）。
///
/// 为什么不用 `"09:00"` 这种字符串：它有三个表示法（`9:00` / `09:00` / `0900`），
/// 每种都得写一遍解析和兜底，而分钟数是唯一的。界面上那两个 `<input type="time">`
/// 负责在边界上做一次转换，中间层只认整数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    /// 上班时刻
    pub start_min: u32,
    /// 下班时刻。允许小于 `start_min`——那就是跨零点夜班。
    pub end_min: u32,
    /// 午休开始
    pub lunch_start_min: u32,
    /// 午休结束。允许小于 `lunch_start_min`——夜班的午休在凌晨。
    pub lunch_end_min: u32,
}

impl Default for Shift {
    fn default() -> Self {
        Self {
            start_min: 9 * 60,
            end_min: 18 * 60,
            lunch_start_min: 12 * 60,
            lunch_end_min: 13 * 60 + 30,
        }
    }
}

impl Shift {
    /// 下班时刻早于等于上班时刻 → 跨零点。
    #[inline]
    pub fn overnight(&self) -> bool {
        self.end_min <= self.start_min
    }

    /// 从班次起点数到某个时刻的分钟偏移，恒在 `0..1440`。
    ///
    /// 这是夜班的全部秘密：02:00 对 21:00 上班的人来说不是「两小时后」，
    /// 而是「五小时后」。取模把「跨了一天」这件事摊平了。
    #[inline]
    fn rel_from_start(&self, min: u32) -> i64 {
        let d = min as i64 - self.start_min as i64;
        d.rem_euclid(1440)
    }

    /// 班次总长（分钟，含午休）。
    #[inline]
    pub fn span_min(&self) -> u32 {
        if self.overnight() {
            self.end_min + 1440 - self.start_min
        } else {
            self.end_min.saturating_sub(self.start_min)
        }
    }

    /// 午休时长（分钟）。起止相对班次起点算，夜班也成立。
    #[inline]
    pub fn lunch_min(&self) -> u32 {
        let a = self.rel_from_start(self.lunch_start_min);
        let b = self.rel_from_start(self.lunch_end_min);
        b.saturating_sub(a) as u32
    }

    /// 每日计薪分钟（班次 − 午休）。
    #[inline]
    pub fn paid_min(&self) -> u32 {
        self.span_min().saturating_sub(self.lunch_min())
    }

    /// 从班次起点到「现在」经过了多少秒。`None` = 不在班上。
    ///
    /// 正班只有一种 `None`：还没到上班时间（09:00 上班、现在 08:00）。
    ///
    /// 夜班有三种情况，靠「在班窗口 = 起点 → 终点」切开：
    /// - 21:00 之后 → 今天这一班，从头数
    /// - 00:00 到 06:00 → **昨天**那一班的尾巴，接着数
    /// - 06:00 到 21:00 → 既不在班上也不是加班，`None`（归零）
    ///
    /// 最后这条和正班不一样：正班过了下班点会一直涨（那是加班），
    /// 夜班的 06:00–21:00 是「下班回家睡觉」的十几个小时，
    /// 让它继续涨成「已加班 9 小时」是纯粹的谎话。
    fn elapsed_sec(&self, now_min: u32, now_sec: u32) -> Option<i64> {
        if self.overnight() {
            let mins = if now_min >= self.start_min {
                now_min - self.start_min
            } else if now_min <= self.end_min {
                now_min + 1440 - self.start_min
            } else {
                return None;
            };
            return Some(mins as i64 * 60 + now_sec as i64);
        }
        let raw = now_min as i64 - self.start_min as i64;
        if raw < 0 {
            return None;
        }
        Some(raw * 60 + now_sec as i64)
    }

    /// 这一班是不是已经走到终点了。
    ///
    /// 「下班通知」必须挂在这上面，**不能挂在「日工时攒满」上**：
    /// 用户把下班时间改成十分钟后、日工时还是七个半小时的时候，
    /// 那一班永远攒不满，`done` 永不出现，通知就永远不弹——
    /// 而用户眼里「我到点了」和「我攒满了」是两件事，他只认得前者。
    ///
    /// `elapsed_sec` 返回 `None` 有两种含义，得分开：
    /// 正班是「还没上班」（不是终点），夜班是「已经下班回家」（就是终点）。
    pub fn past_end(&self, now_min: u32) -> bool {
        match self.elapsed_sec(now_min, 0) {
            Some(e) => e >= self.span_min() as i64 * 60,
            None => self.overnight(),
        }
    }

    /// 距午休开始还有多少秒。已经过了、或者不在班上，一律 0。
    ///
    /// 「午休前 5 分钟提醒」要靠它。0 和「还有 0 秒」必须区分开：
    /// 界面上的 0 是不提醒，而 12:00:00 那一刻也不该再提醒一次。
    pub fn until_lunch_sec(&self, now_min: u32, now_sec: u32) -> u32 {
        let Some(elapsed) = self.elapsed_sec(now_min, now_sec) else {
            return 0;
        };
        let a = self.rel_from_start(self.lunch_start_min) * 60;
        if elapsed >= a {
            0
        } else {
            (a - elapsed) as u32
        }
    }

    /// 已走过的午休秒数（只扣走过的那一段）。
    ///
    /// 人还没到午休就一分不扣，午休走到一半就只扣一半。
    /// 这一条错了，全天金额就全错。
    fn lunch_overlap_sec(&self, elapsed: i64) -> i64 {
        let a = self.rel_from_start(self.lunch_start_min) * 60;
        let b = self.rel_from_start(self.lunch_end_min) * 60;
        if b <= a {
            return 0; // 午休长度非法（配置层已经夹过，这里是第二道）
        }
        (elapsed.min(b) - a).max(0)
    }
}

/// 出勤日历：班次 + 哪几天上班 + 法定节假日 / 调休补班。
///
/// 为什么不塞进 `Settings`：时钟 crate 是纯计算，不该认识配置文件长什么样。
/// 外壳那边用一个 `clock()` 做转换，这是两者唯一的接缝。
///
/// 为什么节假日要传日期表而不是「今天是不是节假日」一个布尔：
/// 「本月还剩几个工作日」「本月已赚」都要按月遍历，一个布尔撑不起来。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    /// 班次时刻
    pub shift: Shift,
    /// 周一到周日，`true` = 上班。单休把周六也置上，三班倒随便怎么排。
    pub days: [bool; 7],
    /// 大小周：`Some(parity)` 表示只有「周序号奇偶性等于它」的那周周六上班。
    ///
    /// 为什么用奇偶而不是「本周上不上」：后者第二天就失效。
    /// 周序号取 `num_days_from_monday / 7`，是连续的、跨年不断的。
    pub alt_sat: Option<u8>,
    /// 法定节假日（放假，不计薪）。手动维护，不硬编码——见 README。
    pub holidays: Vec<NaiveDate>,
    /// 调休补班日（本来是周末，但要上班）。
    pub makeup: Vec<NaiveDate>,
}

/// 周序号奇偶性：从 1970-01-05（周一）起，每 7 天算一周。
///
/// 注意不能用 `Weekday::num_days_from_monday()`——它返回的是 0..6 的星期几，
/// 除以 7 恒得 0，大小周会悄悄退化成「每个周六都上班」。
/// 用 `num_days_from_ce()` 才是连续的绝对天数。
#[inline]
pub fn week_parity(d: NaiveDate) -> u8 {
    const MONDAY_EPOCH_CE: i64 = 719_167; // 1970-01-05
    (d.num_days_from_ce() as i64 - MONDAY_EPOCH_CE)
        .div_euclid(7)
        .rem_euclid(2) as u8
}

impl Calendar {
    /// 双休：周一到周五上班。
    pub fn default_week() -> [bool; 7] {
        [true, true, true, true, true, false, false]
    }

    /// 这一天要不要上班。
    ///
    /// 判定顺序是有讲究的：**补班 > 放假 > 周模板**。
    /// 国庆调休那种「周日要上班」的日子，必须压过「周日休息」这条默认规则，
    /// 否则十月会少算一天钱，而且没人看得出是哪里少的。
    pub fn is_workday(&self, d: NaiveDate) -> bool {
        if self.makeup.contains(&d) {
            return true;
        }
        if self.holidays.contains(&d) {
            return false;
        }
        if let (Weekday::Sat, Some(par)) = (d.weekday(), self.alt_sat) {
            return week_parity(d) == par;
        }
        self.days[d.weekday().num_days_from_monday() as usize]
    }

    /// 这个时刻属于**哪一天**的班。
    ///
    /// 夜班 21:00 上班、凌晨 2 点还在班上时，这笔钱算昨天的。
    /// 不这么归属，跨零点那一刻「本月已赚」会先跳掉一整天才补回来。
    pub fn work_date(&self, now: NaiveDate, now_min: u32) -> NaiveDate {
        // 只有落在「昨天那一班」的窗口里才回退一天。
        // 判据不能写成 `now_min < start_min`：那样 07:00–21:00 这段
        // 也会被算成昨天的班，于是一整个白天都显示「今天休息」。
        if self.shift.overnight() && now_min <= self.shift.end_min {
            now.pred_opt().unwrap_or(now)
        } else {
            now
        }
    }

    /// 某个月一共有多少个工作日。
    pub fn workdays_in_month(&self, year: i32, month: u32) -> u32 {
        let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
            return 0;
        };
        // 下个月 1 号是终点。12 月要进位到下一年，别写成 month + 1。
        let (ny, nm) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let Some(end) = NaiveDate::from_ymd_opt(ny, nm, 1) else {
            return 0;
        };
        self.count_workdays(first, end)
    }

    /// 从本月 1 号到「昨天」为止，过了几个工作日。
    ///
    /// 用途是「本月已赚」的整数部分：已经完整过完的工作日 × 日薪。
    /// 今天那一份要按实际进度算，不能算整数天，所以不含今天。
    pub fn workdays_before(&self, today: NaiveDate) -> u32 {
        let Some(first) = NaiveDate::from_ymd_opt(today.year(), today.month(), 1) else {
            return 0;
        };
        self.count_workdays(first, today)
    }

    fn count_workdays(&self, from: NaiveDate, to: NaiveDate) -> u32 {
        let mut d = from;
        let mut n = 0;
        while d < to {
            if self.is_workday(d) {
                n += 1;
            }
            d = d.succ_opt().expect("日历不会在月末断掉");
        }
        n
    }
}

/// 把任意时刻换算成「已计薪秒」。
///
/// 返回 `(秒, 是否休息日)`。休息日一律给 0——**不给数字，比给一个错的数字好**：
/// 如果照工作日算，周六打开它会一路涨到日薪满额，用户会以为软件在偷钱。
pub fn paid_seconds_at<Tz: TimeZone>(now: chrono::DateTime<Tz>, cal: &Calendar) -> (u32, bool)
where
    Tz::Offset: Copy,
{
    let mins = now.hour() * 60 + now.minute();
    let work_date = cal.work_date(now.date_naive(), mins);
    if !cal.is_workday(work_date) {
        return (0, true);
    }

    let Some(elapsed) = cal.shift.elapsed_sec(mins, now.second()) else {
        return (0, false);
    };
    let lunch = cal.shift.lunch_overlap_sec(elapsed);
    ((elapsed - lunch).clamp(0, u32::MAX as i64) as u32, false)
}

/// 一天里的第几段。它只回答「界面该显示哪个状态」，不回答金额。
///
/// 为什么单独存在：金额公式只认「已计薪秒」，而 `已计薪秒 = 3h` 这一件事
/// 可能发生在午休开始时、午休结束时、上午 12:00 整——三个时刻的界面完全不同
/// （午休中要显示「午休剩余」并长出一个按钮，在岗不该有）。秒数无法区分它们，
/// 所以时钟层得再给出一个「时段」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// 上班前（含凌晨）
    BeforeWork,
    /// 在岗（上午 / 下午）
    Working,
    /// 午休窗口内
    Lunch,
    /// 休息日
    Rest,
}

/// 现在处在一天里的哪一段。
///
/// 边界取左闭右开：12:00:00 已经算午休，13:30:00 已经算在岗。
/// 这条与 `paid_seconds_at` 的扣减区间必须一致，否则会出现
/// 「显示午休中、但金额已经开始涨」这种自相矛盾的帧。
pub fn phase_at<Tz: TimeZone>(now: chrono::DateTime<Tz>, cal: &Calendar) -> Phase
where
    Tz::Offset: Copy,
{
    let mins = now.hour() * 60 + now.minute();
    let work_date = cal.work_date(now.date_naive(), mins);
    if !cal.is_workday(work_date) {
        return Phase::Rest;
    }
    let Some(elapsed) = cal.shift.elapsed_sec(mins, now.second()) else {
        return Phase::BeforeWork;
    };
    let a = cal.shift.rel_from_start(cal.shift.lunch_start_min) * 60;
    let b = cal.shift.rel_from_start(cal.shift.lunch_end_min) * 60;
    if b > a && elapsed >= a && elapsed < b {
        return Phase::Lunch;
    }
    Phase::Working
}

/// 距离午休开始还有多少秒。休息日、没上班、已过午休都是 0。
pub fn lunch_in_at<Tz: TimeZone>(now: chrono::DateTime<Tz>, cal: &Calendar) -> u32
where
    Tz::Offset: Copy,
{
    let mins = now.hour() * 60 + now.minute();
    let work_date = cal.work_date(now.date_naive(), mins);
    if !cal.is_workday(work_date) {
        return 0;
    }
    cal.shift.until_lunch_sec(mins, now.second())
}

/// 距离午休结束还有多少秒。不在午休窗口内一律 0。
pub fn lunch_remaining_at<Tz: TimeZone>(now: chrono::DateTime<Tz>, cal: &Calendar) -> u32
where
    Tz::Offset: Copy,
{
    if phase_at(now, cal) != Phase::Lunch {
        return 0;
    }
    let mins = now.hour() * 60 + now.minute();
    let Some(elapsed) = cal.shift.elapsed_sec(mins, now.second()) else {
        return 0;
    };
    let b = cal.shift.rel_from_start(cal.shift.lunch_end_min) * 60;
    (b - elapsed).clamp(0, u32::MAX as i64) as u32
}

/// 一段「暂停计时」。
///
/// 模型与引擎的 `ShadowSpan` 同构：**扣减量是区间的纯函数**，不做累加。
/// 这一点很关键——如果改成「暂停时开一个计数器每秒 +1」，合盖 8 小时再唤醒
/// 就会多扣 8 小时，那正是这套设计一开始就要避免的 bug。
///
/// 时间用 Unix 秒（而不是 `DateTime`），因为它是**要落盘的**：
/// 用户关了程序、第二天打开，「昨天暂停了多少」得能算出来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseSpan {
    pub start: i64,
    /// `None` = 还在暂停中
    pub end: Option<i64>,
}

impl PauseSpan {
    pub fn ongoing(start: i64) -> Self {
        Self { start, end: None }
    }

    pub fn closed(start: i64, end: i64) -> Self {
        Self {
            start,
            end: Some(end),
        }
    }
}

/// 最后一段还没结束 → 现在正暂停着。
pub fn is_paused(spans: &[PauseSpan]) -> bool {
    spans.last().map_or(false, |s| s.end.is_none())
}

/// 今天因为暂停而被扣掉的秒数。
///
/// 只统计**落在今天之内**的部分：一段从昨天 23:00 连到今早 09:30 的暂停，
/// 昨天那截算在昨天，今天只算 00:00 之后。否则跨天会让今天的工时凭空少一截。
pub fn paused_seconds_at<Tz: TimeZone>(now: chrono::DateTime<Tz>, spans: &[PauseSpan]) -> u32
where
    Tz::Offset: Copy,
{
    if spans.is_empty() {
        return 0;
    }

    let today = now.date_naive();
    let Some(midnight) = now
        .timezone()
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).expect("00:00:00 一定合法"))
        .earliest()
    else {
        // 夏令时把 00:00 跳过的那一天。一天差一小时，不值得为它写特例。
        return 0;
    };

    let day_lo = midnight.timestamp();
    let now_ts = now.timestamp();

    let mut total: i64 = 0;
    for s in spans {
        let a = s.start.max(day_lo);
        // 未结束的用 now 兜底；结束时间在未来的（改过系统时间、时钟回拨）也不外扩
        let b = s.end.unwrap_or(now_ts).min(now_ts);
        if b > a {
            total += b - a;
        }
    }

    total.clamp(0, u32::MAX as i64) as u32
}

/// 丢掉已经不可能再影响今天的区间。
///
/// 不清会越攒越多：暂停一次落一条，一年下来几千条，每次 tick 都遍历一遍。
/// 保留条件是「结束时间还不早于今天零点」——跨零点的区间要留着，
/// 它的今天部分还得算。
pub fn prune_pauses<Tz: TimeZone>(now: chrono::DateTime<Tz>, spans: &mut Vec<PauseSpan>)
where
    Tz::Offset: Copy,
{
    let today = now.date_naive();
    let Some(midnight) = now
        .timezone()
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).expect("00:00:00 一定合法"))
        .earliest()
    else {
        return;
    };
    let day_lo = midnight.timestamp();
    spans.retain(|s| s.end.map_or(true, |e| e >= day_lo));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, mi, 0).single().unwrap()
    }

    // 2026-11-10 是周二
    const D: (i32, u32, u32) = (2026, 11, 10);

    fn cal() -> Calendar {
        Calendar {
            shift: Shift::default(),
            days: Calendar::default_week(),
            alt_sat: None,
            holidays: Vec::new(),
            makeup: Vec::new(),
        }
    }

    fn sec(h: u32, mi: u32) -> u32 {
        paid_seconds_at(at(D.0, D.1, D.2, h, mi), &cal()).0
    }

    #[test]
    fn before_work_is_zero() {
        assert_eq!(sec(8, 30), 0);
        assert_eq!(sec(9, 0), 0);
    }

    #[test]
    fn morning_accrues_plainly() {
        assert_eq!(sec(9, 1), 60);
        assert_eq!(sec(11, 59), 2 * 3600 + 59 * 60);
    }

    #[test]
    fn lunch_is_deducted_exactly_once() {
        // 午休 12:00–13:30，共 90 分钟。走到哪扣到哪。
        assert_eq!(sec(12, 0), 3 * 3600);
        assert_eq!(sec(12, 30), 3 * 3600);
        assert_eq!(sec(13, 0), 3 * 3600);
        assert_eq!(sec(13, 30), 3 * 3600);
        // 出午休后继续累计，且只扣 90 分钟
        assert_eq!(sec(14, 0), 3 * 3600 + 30 * 60);
    }

    #[test]
    fn full_workday_is_exactly_seven_and_a_half_hours() {
        // 9:00 → 18:00 共 9 小时，扣 90 分钟午休 = 7.5 小时 = 27000 秒
        assert_eq!(sec(18, 0), 27_000);
    }

    #[test]
    fn overtime_keeps_counting_past_the_workday() {
        assert_eq!(sec(19, 0), 27_000 + 3600);
    }

    #[test]
    fn weekend_gives_no_number_at_all() {
        // 2026-11-14 是周六
        let (s, rest) = paid_seconds_at(at(2026, 11, 14, 10, 0), &cal());
        assert!(rest, "周末必须标记为休息，否则会虚增 367.82");
        assert_eq!(s, 0);

        // 2026-11-15 是周日
        let (s2, rest2) = paid_seconds_at(at(2026, 11, 15, 10, 0), &cal());
        assert!(rest2);
        assert_eq!(s2, 0);
    }

    #[test]
    fn cross_midnight_does_not_go_negative() {
        // 凌晨 1 点：早于上班时间，不能出现负数或巨大值
        let (s, rest) = paid_seconds_at(at(2026, 11, 11, 1, 0), &cal());
        assert!(!rest);
        assert_eq!(s, 0);
    }

    // ---- 排班：单休 / 大小周 / 节假日 / 调休 ----

    /// 单休：周六也上班，于是周六有数字，周日没有。
    #[test]
    fn six_day_week_pays_you_on_saturday() {
        let mut c = cal();
        c.days = [true, true, true, true, true, true, false];
        // 11-14 周六
        let (s, rest) = paid_seconds_at(at(2026, 11, 14, 12, 0), &c);
        assert!(!rest);
        assert_eq!(s, 3 * 3600, "周六上到中午就是 3 小时");
        // 11-15 周日仍然是休息日
        assert!(paid_seconds_at(at(2026, 11, 15, 12, 0), &c).1);
    }

    /// 大小周：相邻两周的周六，一周上班一周休息。
    #[test]
    fn alternating_saturday_flips_week_by_week() {
        let mut c = cal();
        c.days = [true, true, true, true, true, true, false];
        // 2026-11-14 的周序号算出来是 0 还是 1 不重要，重要的是**相邻两周相反**
        let par = week_parity(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap());
        c.alt_sat = Some(par);

        assert!(c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()), "本周六上班");
        assert!(!c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()), "下周六休息");
        assert!(c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 28).unwrap()), "再下周六上班");
        // 周日不受大小周影响
        assert!(!c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()));
    }

    /// 大小周只该改变周六，工作日一天都不能少。
    #[test]
    fn alternating_saturday_keeps_weekdays_intact() {
        let mut c = cal();
        c.days = [true, true, true, true, true, true, false];
        c.alt_sat = Some(0);
        for d in 1..=30 {
            let date = NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
            if date.weekday().num_days_from_monday() < 5 {
                assert!(c.is_workday(date), "{date} 是工作日");
            }
        }
    }

    /// 法定节假日不上班，但如果那天本来就休息，也不该被算两次。
    #[test]
    fn holiday_off_but_weekend_holiday_is_not_double_counted() {
        let mut c = cal();
        // 2026-11-11（周三）标成法定假日
        c.holidays = vec![NaiveDate::from_ymd_opt(2026, 11, 11).unwrap()];
        assert!(!c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 11).unwrap()));
        assert!(paid_seconds_at(at(2026, 11, 11, 12, 0), &c).1, "标了假日就是休息日");

        // 假日数量：11 月原本 21 个工作日，扣掉一天 = 20
        assert_eq!(c.workdays_in_month(2026, 11), 20);
    }

    /// 调休补班：周日要上班。这条压过「周日休息」。
    #[test]
    fn makeup_day_overrides_the_week_template() {
        let mut c = cal();
        c.makeup = vec![NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()]; // 周日
        assert!(c.is_workday(NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()));
        assert_eq!(c.workdays_in_month(2026, 11), 22, "21 + 补班 1 天");
    }

    /// 同一天既在放假日表又在补班表里：补班优先。
    /// 用户手滑同时填了两个，得有一个确定的答案，不能看遍历顺序。
    #[test]
    fn makeup_beats_holiday_when_both_listed() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let mut c = cal();
        c.holidays = vec![d];
        c.makeup = vec![d];
        assert!(c.is_workday(d));
    }

    // ---- 跨零点夜班 ----

    fn night() -> Calendar {
        let mut c = cal();
        // 21:00 → 06:00，午休 02:00–03:00
        c.shift = Shift {
            start_min: 21 * 60,
            end_min: 6 * 60,
            lunch_start_min: 2 * 60,
            lunch_end_min: 3 * 60,
        };
        c
    }

    /// 夜班 21:00–06:00，午休 02:00–03:00：一夜 9 小时 − 1 小时 = 8 小时。
    #[test]
    fn night_shift_counts_across_midnight() {
        let c = night();
        assert_eq!(c.shift.span_min(), 540);
        assert_eq!(c.shift.lunch_min(), 60);
        assert_eq!(c.shift.paid_min(), 480);

        // 上班时刻
        assert_eq!(paid_seconds_at(at(2026, 11, 10, 21, 0), &c).0, 0);
        // 午夜整：3 小时
        assert_eq!(paid_seconds_at(at(2026, 11, 11, 0, 0), &c).0, 3 * 3600);
        // 02:30：5.5 小时进来，午休已走半小时 → 5 小时
        assert_eq!(paid_seconds_at(at(2026, 11, 11, 2, 30), &c).0, 5 * 3600);
        // 06:00：整班 8 小时
        assert_eq!(paid_seconds_at(at(2026, 11, 11, 6, 0), &c).0, 8 * 3600);
    }

    /// 凌晨那一截算**昨天**的班：周二凌晨 2 点属于周一，周一不上班就没有数字。
    #[test]
    fn night_shift_belongs_to_the_day_it_started() {
        let c = night();
        // 周一 21:00 上班 → 周二 02:00 还在班上
        assert_eq!(paid_seconds_at(at(2026, 11, 10, 23, 0), &c).0, 2 * 3600);
        // 02:00 整刚好是午休起点（左闭），此刻还一分不扣 → 5 小时
        assert_eq!(paid_seconds_at(at(2026, 11, 11, 2, 0), &c).0, 5 * 3600);

        // 周日晚上没上班 → 周一凌晨不该有钱
        let (s, rest) = paid_seconds_at(at(2026, 11, 16, 2, 0), &night());
        assert!(rest, "周日凌晨属于周六那一班，周六不上班");
        assert_eq!(s, 0);
    }

    /// 夜班的「上班前」仍然要正确：下午 3 点离上班还早。
    #[test]
    fn night_shift_is_before_work_in_the_afternoon() {
        let c = night();
        assert_eq!(paid_seconds_at(at(2026, 11, 10, 15, 0), &c).0, 0);
        assert_eq!(phase_at(at(2026, 11, 10, 15, 0), &c), Phase::BeforeWork);
        // 但到了晚上 10 点就在岗了
        assert_eq!(phase_at(at(2026, 11, 10, 22, 0), &c), Phase::Working);
    }

    /// 夜班的午休倒计时不能因为跨了天就算成 0。
    #[test]
    fn night_shift_lunch_countdown_works() {
        let c = night();
        assert_eq!(phase_at(at(2026, 11, 11, 2, 0), &c), Phase::Lunch);
        assert_eq!(lunch_remaining_at(at(2026, 11, 11, 2, 0), &c), 3600);
        assert_eq!(lunch_remaining_at(at(2026, 11, 11, 2, 30), &c), 1800);
        assert_eq!(phase_at(at(2026, 11, 11, 3, 0), &c), Phase::Working);
    }

    // ---- 暂停计时 ----

    fn ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        at(y, m, d, h, mi).timestamp()
    }

    #[test]
    fn no_span_means_no_deduction() {
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 15, 0), &[]), 0);
    }

    #[test]
    fn ongoing_pause_follows_the_clock_not_a_counter() {
        // 关键性质：暂停时长是「now − start」，不是每秒 +1 的计数器。
        // 所以暂停期间合盖 8 小时再唤醒，扣的仍然是真实经过的那 8 小时。
        let spans = [PauseSpan::ongoing(ts(D.0, D.1, D.2, 10, 0))];
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 10, 30), &spans), 1_800);
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 12, 0), &spans), 7_200);
        // 钟往回走也不能出现负数或倒扣
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 9, 0), &spans), 0);
    }

    #[test]
    fn closed_pause_has_a_fixed_length() {
        let span = PauseSpan::closed(ts(D.0, D.1, D.2, 10, 0), ts(D.0, D.1, D.2, 10, 20));
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 10, 20), &[span]), 1_200);
        // 结束之后时间继续走，扣减量不该再变
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 17, 0), &[span]), 1_200);
    }

    #[test]
    fn spans_add_up() {
        let spans = [
            PauseSpan::closed(ts(D.0, D.1, D.2, 10, 0), ts(D.0, D.1, D.2, 10, 10)),
            PauseSpan::closed(ts(D.0, D.1, D.2, 14, 0), ts(D.0, D.1, D.2, 14, 5)),
        ];
        assert_eq!(paused_seconds_at(at(D.0, D.1, D.2, 18, 0), &spans), 600 + 300);
    }

    #[test]
    fn span_from_yesterday_only_counts_today_part() {
        // 昨晚 23:00 暂停，今早 00:30 才恢复。今天只该扣 30 分钟，
        // 不能把昨天那 1 小时也算进来，否则今天的工时凭空少一截。
        let span = PauseSpan::closed(ts(2026, 11, 9, 23, 0), ts(2026, 11, 10, 0, 30));
        assert_eq!(paused_seconds_at(at(2026, 11, 10, 9, 0), &[span]), 1_800);
    }

    #[test]
    fn is_paused_follows_the_last_span() {
        assert!(!is_paused(&[]));
        assert!(is_paused(&[PauseSpan::ongoing(ts(D.0, D.1, D.2, 10, 0))]));
        assert!(!is_paused(&[PauseSpan::closed(
            ts(D.0, D.1, D.2, 10, 0),
            ts(D.0, D.1, D.2, 10, 1)
        )]));
    }

    #[test]
    fn prune_drops_yesterday_but_keeps_cross_midnight() {
        let mut spans = vec![
            // 前天就结束的：对今天零贡献，删
            PauseSpan::closed(ts(2026, 11, 7, 9, 0), ts(2026, 11, 7, 9, 30)),
            // 跨零点的：今天那截还要算，留
            PauseSpan::closed(ts(2026, 11, 9, 23, 0), ts(2026, 11, 10, 0, 30)),
            // 还没结束的：留
            PauseSpan::ongoing(ts(D.0, D.1, D.2, 16, 0)),
        ];
        prune_pauses(at(D.0, D.1, D.2, 16, 10), &mut spans);
        assert_eq!(spans.len(), 2);
        assert!(spans
            .iter()
            .all(|s| s.end.is_none() || s.end.unwrap() >= ts(D.0, D.1, D.2, 0, 0)));
    }

    // ---- 时段 ----

    #[test]
    fn phases_follow_the_schedule() {
        let s = cal();
        assert_eq!(phase_at(at(D.0, D.1, D.2, 8, 59), &s), Phase::BeforeWork);
        assert_eq!(phase_at(at(D.0, D.1, D.2, 9, 0), &s), Phase::Working);
        assert_eq!(phase_at(at(D.0, D.1, D.2, 11, 59), &s), Phase::Working);
        // 左闭：12:00 整已经算午休
        assert_eq!(phase_at(at(D.0, D.1, D.2, 12, 0), &s), Phase::Lunch);
        assert_eq!(phase_at(at(D.0, D.1, D.2, 13, 29), &s), Phase::Lunch);
        // 右开：13:30 整已经算在岗
        assert_eq!(phase_at(at(D.0, D.1, D.2, 13, 30), &s), Phase::Working);
        assert_eq!(phase_at(at(D.0, D.1, D.2, 19, 0), &s), Phase::Working);
    }

    #[test]
    fn weekend_is_always_rest_phase() {
        let s = cal();
        // 2026-11-14 周六、11-15 周日
        assert_eq!(phase_at(at(2026, 11, 14, 10, 0), &s), Phase::Rest);
        assert_eq!(phase_at(at(2026, 11, 15, 10, 0), &s), Phase::Rest);
    }

    /// 时段与秒数必须一致：午休期间秒数不涨，出了午休才开始涨。
    /// 两条口径错位就会出现「显示午休中、金额却在跳」的帧。
    #[test]
    fn phase_and_seconds_agree_during_lunch() {
        let s = cal();
        let a = paid_seconds_at(at(D.0, D.1, D.2, 12, 0), &s).0;
        let b = paid_seconds_at(at(D.0, D.1, D.2, 13, 29), &s).0;
        assert_eq!(a, b, "午休区间内已计薪秒不该变化");
        assert_eq!(phase_at(at(D.0, D.1, D.2, 13, 29), &s), Phase::Lunch);
    }

    #[test]
    fn lunch_countdown_is_90_minutes_then_zero() {
        let s = cal();
        assert_eq!(lunch_remaining_at(at(D.0, D.1, D.2, 12, 0), &s), 5_400);
        assert_eq!(lunch_remaining_at(at(D.0, D.1, D.2, 13, 0), &s), 1_800);
        assert_eq!(lunch_remaining_at(at(D.0, D.1, D.2, 13, 30), &s), 0);
        // 不在午休窗口内不能返回一个正数，否则界面会显示假倒计时
        assert_eq!(lunch_remaining_at(at(D.0, D.1, D.2, 11, 0), &s), 0);
        assert_eq!(lunch_remaining_at(at(D.0, D.1, D.2, 15, 0), &s), 0);
    }

    // ---- 本月工作日 ----

    /// 2026 年 11 月：1 号是周日，工作日是 2–6、9–13、16–20、23–27、30，共 21 天。
    /// 这个数字和原型稿上的「本月还剩 20 个工作日」是同一套口径（稿子写的是示意值）。
    #[test]
    fn november_2026_has_21_workdays() {
        assert_eq!(cal().workdays_in_month(2026, 11), 21);
    }

    /// 11/10 是本月第 7 个工作日，之前已经有 6 个。
    #[test]
    fn six_workdays_passed_before_nov_10() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        assert_eq!(cal().workdays_before(d), 6);
    }

    /// 12 月要能跨年算出下个月 1 号，不能写成 month + 1 变成 13 月。
    #[test]
    fn december_does_not_overflow_into_month_13() {
        // 2026-12-01 是周二；12 月 31 天，1–4 号那一周只算 4 天
        assert_eq!(cal().workdays_in_month(2026, 12), 23);
    }

    /// 月初第一天的「已过工作日」必须是 0，不能把自己算进去。
    #[test]
    fn first_day_of_month_has_nothing_before_it() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(); // 周日
        assert_eq!(cal().workdays_before(d), 0);
        let d2 = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(); // 周一
        assert_eq!(cal().workdays_before(d2), 0, "2 号当天不算「已经过完」");
    }

    /// 单休的整月天数：2026 年 11 月有 4 个周六（7/14/21/28），21 + 4 = 25。
    #[test]
    fn six_day_week_month_count() {
        let mut c = cal();
        c.days = [true, true, true, true, true, true, false];
        assert_eq!(c.workdays_in_month(2026, 11), 25);
    }

    /// 下班通知的触发点：班次走到终点就算到点，跟「日工时攒没攒满」无关。
    ///
    /// 这条测试来自用户的原话：「下班时间设成 1 分钟后，今日已赚没在计算，
    /// 右下角也没滑出通知」。日工时 7.5 小时、班次只有 3 小时半的时候，
    /// `paid_sec >= daily_sec` 永远不成立，按 `done` 去挂通知就永远不弹。
    #[test]
    fn the_end_of_the_shift_is_not_the_same_as_filling_the_day() {
        let c = cal();                       // 09:00–18:00
        assert!(!c.shift.past_end(8 * 60), "还没上班，更谈不上到点");
        assert!(!c.shift.past_end(9 * 60), "刚上班");
        assert!(!c.shift.past_end(17 * 60 + 59), "差一分钟");
        assert!(c.shift.past_end(18 * 60), "整点就该到点");
        assert!(c.shift.past_end(18 * 60 + 1));
        assert!(c.shift.past_end(23 * 60), "晚上十一点也是过了下班点");
    }

    /// 夜班：00:30 还在班上，06:00 之后才算到点。
    ///
    /// 21:00 这个点（上一班已经散了、这一班还没开）单看布尔值是分不清
    /// 「还没上班」和「已经下班」的，两边都落在班次外面。调用方必须再用
    /// 「今天真的干过活没有」兜一道——`past_end` 只管「在不在班里」。
    #[test]
    fn a_night_shift_ends_after_midnight() {
        let mut c = cal();
        c.shift = Shift {
            start_min: 22 * 60,
            end_min: 6 * 60,
            lunch_start_min: 2 * 60,
            lunch_end_min: 3 * 60,
        };
        assert!(!c.shift.past_end(23 * 60), "刚上班");
        assert!(!c.shift.past_end(30), "00:30，正上着");
        assert!(!c.shift.past_end(5 * 60 + 59));
        assert!(c.shift.past_end(6 * 60), "06:00 到点");
        assert!(c.shift.past_end(12 * 60), "白天那段时间是「已经下班」");
        // 班次之外的两个空档，布尔值都是 true；靠调用方的「今天干过活」区分。
        assert!(c.shift.past_end(21 * 60));
    }
}
