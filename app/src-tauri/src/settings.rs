//! 配置持久化（外壳层）。
//!
//! 三条硬约束，都是踩过的坑：
//!
//! 1. **坏文件不能让程序起不来。** 用户手改一个逗号、或上次写入时断电，
//!    程序就再也打不开——这是最糟的失败模式，比重置配置糟得多。
//!    所以解析失败一律退回默认值，并把坏文件改名留证。
//! 2. **写盘要原子。** 直接 `File::create` 覆盖，掉电就得到半截 JSON，
//!    下次启动正好命中第 1 条。改成「写 tmp → sync → rename」。
//! 3. **计薪日是分母。** `pay_days_centi = 0` 会让引擎整数除零 panic，
//!    而这个值是可以从文件里手填的，所以读进来之后必须校验。
//!
//! 配置里**没有费率常量**——那些属于引擎的 `Params`，由调用方传进去。
//! 这里只负责「用户说了什么」，不负责「怎么算」。

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 月薪下限：1 元。再低没有意义，且 0 会让进度条恒为 0。
pub const MONTH_MIN: i64 = 1_000;
/// 月薪上限：10 亿元（毫）。
///
/// 这不是业务限制——税前月薪没有「最多多少」这回事。它只是两道**防溢出护栏**：
/// ① `hourly_milli()` 里 `total × 100 × 3600` 在 total ≈ 256 亿元时会逼近 i64 上限，
///    10 亿离它还有一个数量级，绝对安全；
/// ② `merge` 里 `元 × 1000` 是 f64 乘法，10 亿元（1e13 毫）毫级精度无损。
/// 现实里不存在税前月薪超过 10 亿的人，这个护栏碰不到，纯属挡住手滑多打 0。
pub const MONTH_MAX: i64 = 10_000_000_000_000;

/// `2026-11-10` 这个形状。只认形状和月日区间，不认真实历法——
/// 2 月 30 号照样放行，它不会算错任何东西。
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let m: u32 = s[5..7].parse().unwrap_or(0);
    let d: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// 用户输入的一串元 → 毫。最多四位小数。
///
/// 这个换算不放在外壳里：JS 的 `12.34 * 10000` 得到 123399.99999999999，
/// 一舍一入就少一毫。这里按字符串拆，加减乘全是整数。
///
/// 接受 `300`、`12.5`、`0.0001`、`1,200`、`¥88`；其余一律 `None`，
/// 由调用方去解释「为什么不行」。
pub fn parse_yuan(s: &str) -> Option<i64> {
    let s = s.trim().replace([',', '，'], "");
    let s = s.strip_prefix('¥').unwrap_or(&s).trim();
    if s.is_empty() {
        return None;
    }
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    // 毫之下还有丝，但账面上没有意义；多打的位数当成输入错误而不是四舍五入
    if frac.len() > 4 {
        return None;
    }
    if int_part.is_empty() && frac.is_empty() {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let int_v: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().ok()?
    };
    // 补到四位：`12.5` → "5000"，`12.05` → "0500"
    let mut f = frac.to_string();
    while f.len() < 4 {
        f.push('0');
    }
    let frac_v: i64 = f.parse().ok()?;
    int_v.checked_mul(10_000)?.checked_add(frac_v)
}

/// 窗口位置（逻辑像素，左上角）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinPos {
    pub x: i32,
    pub y: i32,
}

/// 一段暂停，落盘用。
///
/// 这只是 `paypulse_clock::PauseSpan` 的序列化镜像：时钟 crate 是纯计算，
/// 不该因为「要存盘」而多引一个 serde 依赖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseRec {
    pub start: i64,
    pub end: Option<i64>,
}

/// 一笔额外进账，落盘用。
///
/// 只记「哪天、多少」两件事。日期存 `YYYY-MM-DD` 而不是时间戳，是因为
/// 它按**本地日历**归属月份——同一个时间戳在东八区和 UTC 差一天，
/// 那笔钱就会跑到上个月去。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraEntry {
    /// `YYYY-MM-DD`
    pub date: String,
    /// 金额，单位毫。¥300 → 3_000_000
    pub amount_milli: i64,
}

/// 加班怎么算。
///
/// 原型稿的设置页里就是一个下拉：「1.5× 计薪」/「不计加班费（仅记白干价值）」。
/// 所以悬浮窗底部只留一个「我在加班」——它只声明「我现在在加班」，
/// 进主账还是只记白干，由这个设置决定。
/// 早先做的「我在加班 / 不计加班费」两个按钮是错的：那等于让人每加一次班选一次费率。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OtMode {
    /// 按 1.5× 计入月薪
    Paid,
    /// 不计加班费，只记白干价值（默认，也是现实里更常见的那种）
    Shadow,
}

impl Default for OtMode {
    fn default() -> Self {
        Self::Shadow
    }
}

/// 窗口尺寸档位。
///
/// 原型 01-C「像素规格标注」给了三个尺寸：紧凑 236×118、标准 302×242、展开 322×330；
/// 01-D「缩放」把交互定成**三档循环**：紧凑 → 标准 → 展开，双击标题区或滚轮触发。
///
/// 三档不是「缩放比例」而是**三套版式**：紧凑态砍掉指标行、页脚、提示，
/// 展开态多出一块等价物。所以它只能是枚举，不能是一个 width 数值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// 236 × 118：贴边用，只留金额、已工作、进度条
    Compact,
    /// 302 × 242：默认
    Normal,
    /// 322 × 330：多一块「奶茶等价物」
    Expanded,
}

impl Default for Size {
    fn default() -> Self {
        Self::Normal
    }
}

impl Size {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Expanded => "expanded",
        }
    }

    /// 这一档的目标宽度（逻辑像素）。
    ///
    /// 只有宽度是这里定的——高度永远由内容撑开，再回填给窗口。
    /// 定死高度就会在「白干条出现 / 动作行出现」时把内容裁掉。
    pub fn width(self) -> f64 {
        match self {
            Self::Compact => 236.0,
            Self::Normal => 302.0,
            Self::Expanded => 322.0,
        }
    }

    /// 循环到相邻档位。`dir > 0` 往大的走，`dir < 0` 往小的走。
    ///
    /// 用 `rem_euclid` 而不是取模：`%` 在负数上返回负值，
    /// 从紧凑档往回退会算出 -1 越界 panic。
    pub fn cycle(self, dir: i32) -> Self {
        let i = match self {
            Self::Compact => 0,
            Self::Normal => 1,
            Self::Expanded => 2,
        };
        match (i + dir).rem_euclid(3) {
            0 => Self::Compact,
            1 => Self::Normal,
            _ => Self::Expanded,
        }
    }
}

/// 一天的作息，单位是「当天第几分钟」（0–1439）。
///
/// 为什么不用 `"09:00"` 这种字符串：它有三个表示法（`9:00` / `09:00` / `0900`），
/// 每种都得写一遍解析和兜底，而分钟数是唯一的。界面上那两个 `<input type="time">`
/// 负责在边界上做一次转换，中间层只认整数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedCfg {
    /// 上班时刻
    pub start_min: u32,
    /// 下班时刻。它不参与「已计薪秒」的计算（那是 `daily_sec` 的事），
    /// 只用来推导 `daily_sec` 和显示「在岗 X h − 午休 Y h = 计薪 Z h」。
    pub end_min: u32,
    /// 午休开始
    pub lunch_start_min: u32,
    /// 午休结束
    pub lunch_end_min: u32,
    /// 允许跨零点夜班（下班时刻早于上班时刻）。
    ///
    /// 做成开关而不是「自动判定」：18:00 手滑打成 08:00 时，
    /// 自动判定会把一整天的时间轴悄悄翻个面，金额照算、没人发现。
    /// 开着这个开关才认「故意跨天」。
    pub cross_midnight: bool,
}

impl Default for SchedCfg {
    fn default() -> Self {
        Self {
            start_min: 9 * 60,
            end_min: 18 * 60,
            lunch_start_min: 12 * 60,
            lunch_end_min: 13 * 60 + 30,
            cross_midnight: false,
        }
    }
}

impl SchedCfg {
    /// 跨零点判定。开关没开就永远当正班——见 `cross_midnight` 的注释。
    pub fn overnight(&self) -> bool {
        self.cross_midnight && self.end_min <= self.start_min
    }

    /// 在岗时长（秒），含午休。
    ///
    /// 夜班的算式是 `end + 24h − start`，不是 `end − start`（那会是负数）。
    pub fn span_sec(&self) -> u32 {
        if self.overnight() {
            (self.end_min + 24 * 60 - self.start_min) * 60
        } else {
            self.end_min.saturating_sub(self.start_min) * 60
        }
    }

    /// 午休时长（秒）。起止都相对班次起点取模，夜班的凌晨午休也对。
    pub fn lunch_sec(&self) -> u32 {
        let rel = |m: u32| (m as i64 - self.start_min as i64).rem_euclid(24 * 60);
        let d = rel(self.lunch_end_min) - rel(self.lunch_start_min);
        (d.max(0) as u32) * 60
    }

    /// 由作息推导出的每日计薪秒数。
    ///
    /// 原型 04 章的 calc-strip 就是这个式子：「在岗 9.0 h − 午休 1.5 h = 计薪 7.5 h」。
    /// 推导结果会写回 `daily_sec`——但 `daily_sec` 是可手工覆盖的，
    /// 所以这里只负责「改作息时同步一次」，不是每帧都算。
    pub fn paid_sec(&self) -> u32 {
        self.span_sec().saturating_sub(self.lunch_sec()).max(1)
    }

    /// 夹回合法区间。作息是**边界**，坏数据会让整套时间判断自相矛盾：
    /// 午休跑到下班之后，`paid_seconds_at` 会扣掉一段还没到的午休，
    /// 金额当场变成负数（被 saturating 吃掉一半工时）。
    pub fn sanitize(&mut self) {
        const LAST: u32 = 24 * 60 - 1;
        self.start_min = self.start_min.min(LAST);
        self.end_min = self.end_min.min(24 * 60);
        self.lunch_start_min = self.lunch_start_min.min(LAST);
        self.lunch_end_min = self.lunch_end_min.min(24 * 60);

        // 没开「跨零点夜班」时只接受正班：end <= start 就夹成 one-hour shift。
        // 开了就放行，`overnight()` 会接管后面的换算。
        if !self.cross_midnight && self.end_min <= self.start_min {
            self.end_min = (self.start_min + 60).min(24 * 60);
        }
        // 午休必须整个落在上班区间里；夹不进去就退回默认的 12:00–13:30。
        let (s, e) = (self.start_min, self.end_min);
        if self.lunch_start_min < s
            || self.lunch_end_min > e
            || self.lunch_end_min <= self.lunch_start_min
        {
            let d = SchedCfg::default();
            // 默认午休塞不进当前正班时，就退化成「没有午休」
            if d.lunch_start_min >= s && d.lunch_end_min <= e {
                self.lunch_start_min = d.lunch_start_min;
                self.lunch_end_min = d.lunch_end_min;
            } else {
                self.lunch_start_min = s;
                self.lunch_end_min = s;
            }
        }
    }

    /// 转成时钟层认的班次。
    pub fn shift(&self) -> paypulse_clock::Shift {
        paypulse_clock::Shift {
            start_min: self.start_min,
            end_min: self.end_min,
            lunch_start_min: self.lunch_start_min,
            lunch_end_min: self.lunch_end_min,
        }
    }

    /// 整套作息沿时间轴平移 `delta` 分钟。
    ///
    /// 四个时刻一起移，`cross_midnight` 原样带过去：平移让下班时刻跑到上班
    /// 时刻前面（比如 21:00 上的班，锚点推到 23:00）时，跨零点那套换算照样成立。
    pub fn shifted(&self, delta: i32) -> SchedCfg {
        let m = |v: u32| -> u32 { (v as i32 + delta).rem_euclid(24 * 60) as u32 };
        SchedCfg {
            start_min: m(self.start_min),
            end_min: m(self.end_min),
            lunch_start_min: m(self.lunch_start_min),
            lunch_end_min: m(self.lunch_end_min),
            cross_midnight: self.cross_midnight,
        }
    }
}

/// 悬浮窗上的信息密度开关（原型 07 章「信息密度」那四个勾选框）。
///
/// 全都是「关掉不影响算钱」的纯展示项，所以放在一起，用一次写盘换四个开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Density {
    /// 每秒 / 时薪 / 日薪 那一行
    pub metrics: bool,
    /// 进度条与「已工作 / 百分比」
    pub bar: bool,
    /// 底部那句小字
    pub tip: bool,
    /// 白干条
    pub shadow: bool,
}

impl Default for Density {
    fn default() -> Self {
        Self {
            metrics: true,
            bar: true,
            tip: true,
            shadow: true,
        }
    }
}

/// 一段午休加班（12:00–13:30 之间，封顶到 13:30）。
///
/// 为什么午休加班要单独记区间、不能像下班后那样直接从「已计薪秒」推：
/// 午休本来就不计薪，那 90 分钟里已计薪秒是不动的，
/// 所以 `秒数 − 日工时` 永远得 0，推不出白干时长。只能记下开始时刻和封顶。
///
/// 为什么是「一段」而不是「一个」：原型稿里结束加班会把这一段**收口**
/// （记下结束时刻）而不是丢掉——「当天有白干记录就保留」。所以这里存一份列表，
/// 收了口的段长度固定，不再随时间增长；没收口的那段才用 now 去量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LunchOtRec {
    /// 开始时刻（Unix 秒）
    pub start: i64,
    /// 结束时刻。`None` = 还在记。
    pub end: Option<i64>,
    /// 本段最多能记多久。开始那一刻就按「距 13:30 还有多少秒」定死。
    pub cap_sec: u32,
}

impl LunchOtRec {
    /// 本段实际记了多久（秒）。
    pub fn len(&self, now: i64) -> u32 {
        let raw = (self.end.unwrap_or(now) - self.start).max(0);
        raw.min(self.cap_sec as i64) as u32
    }
}


/// 排班形态。**不落盘**，由「七天格子 + 大小周」反推出来——
/// 存一份 form 就会跟 days 打架（用户改了单天，form 还写着「双休」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkForm {
    Five,
    BigSmall,
    Six,
    /// 自己拼的（四班三倒、上四休三……）
    Custom,
}

impl WorkForm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Five => "five",
            Self::BigSmall => "bigsmall",
            Self::Six => "six",
            Self::Custom => "custom",
        }
    }
}

/// 一周怎么排班，加上两张日期表。
///
/// 为什么节假日存 `Vec<String>` 而不是 `Vec<NaiveDate>`：
/// 手改配置文件是这套东西的常规操作，`NaiveDate` 的 serde 形状是
/// `{"year":2026,...}` 这种，写起来没人愿意看，也容易写错。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeekCfg {
    /// 周一到周日，`true` = 上班
    pub days: [bool; 7],
    /// 大小周：`Some(0|1)` 表示只有奇偶性等于它的那一周周六上班
    pub alt_sat: Option<u8>,
    /// 法定节假日（放假不计薪），`YYYY-MM-DD`
    pub holidays: Vec<String>,
    /// 调休补班日（周末但要上班），`YYYY-MM-DD`
    pub makeup: Vec<String>,
    /// 弹性上下班：今天第一次动键鼠的时刻，就是这一天的上班点。
    ///
    /// 它不改「哪几天上班」（那是 `days` 的事），只改「这一天从几点开始」——
    /// 所以它不影响月历、不影响工作日数，只影响今天的计薪区间。
    /// 没动过键鼠之前一分钱都不记（见 `AppState` 里的 `day_anchor`）。
    pub flex: bool,
}

impl Default for WeekCfg {
    fn default() -> Self {
        Self {
            days: [true, true, true, true, true, false, false],
            alt_sat: None,
            holidays: Vec::new(),
            makeup: Vec::new(),
            flex: false,
        }
    }
}

impl WeekCfg {
    /// 反推形态。UI 上那四张卡片的高亮靠它，不靠存一个 form 字段。
    pub fn form(&self) -> WorkForm {
        if self.alt_sat.is_some() {
            return WorkForm::BigSmall;
        }
        if self.days == [true, true, true, true, true, false, false] {
            return WorkForm::Five;
        }
        if self.days == [true, true, true, true, true, true, false] {
            return WorkForm::Six;
        }
        WorkForm::Custom
    }

    pub fn holiday_dates(&self) -> impl Iterator<Item = NaiveDate> + '_ {
        self.holidays.iter().filter_map(|s| parse_day(s))
    }

    pub fn makeup_dates(&self) -> impl Iterator<Item = NaiveDate> + '_ {
        self.makeup.iter().filter_map(|s| parse_day(s))
    }
}

/// 年终奖怎么摊。原型 04 章那四张 chip。
///
/// **只有摊入 12 个月才会改变时薪**——一次性发的红包不该让每天都虚高。
/// 所以这里算的是「每个月多出来多少」，不是「一次性给多少」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BonusMode {
    /// 不计入
    None,
    /// 自定义 xx 薪：填 3 个月 = 15 薪
    Months,
    /// 按金额（红包）
    Amount,
    /// 按年薪 %
    Rate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bonus {
    #[serde(deserialize_with = "de_bonus")]
    pub mode: BonusMode,
    /// 年终奖 = 几个月工资 × 100。100 = 1 个月 = 13 薪
    pub months_x100: u32,
    /// 红包总额（毫）
    pub amount_milli: i64,
    /// 年薪百分比 × 100。1000 = 10%
    pub rate_x100: u32,
}

impl Default for Bonus {
    fn default() -> Self {
        Self {
            mode: BonusMode::None,
            months_x100: 100,
            amount_milli: 0,
            rate_x100: 0,
        }
    }
}

impl Bonus {
    /// 年终奖总额（毫）。
    ///
    /// 三种口径在这里就化成同一个数，后面「摊到每月」只剩一次除法：
    /// - months：年终奖 = 月薪 × 月数
    /// - amount：总额已知
    /// - rate：年薪 × 比例，年薪 = 月薪 × 12
    ///
    /// 先算总额再除 12，而不是「每月各算一遍」，是因为后者会各自截一次尾，
    /// 12 个月加起来和总额差几毫——账面上「年终奖 30000，摊下来 29999.88」很难解释。
    pub fn total_milli(&self, month_milli: i64) -> i64 {
        match self.mode {
            BonusMode::None => 0,
            BonusMode::Months => month_milli.saturating_mul(self.months_x100 as i64) / 100,
            BonusMode::Amount => self.amount_milli.max(0),
            BonusMode::Rate => month_milli.saturating_mul(12) * self.rate_x100 as i64 / 10_000,
        }
    }

    /// 每个月该多算多少钱（毫）。
    pub fn per_month_milli(&self, month_milli: i64) -> i64 {
        self.total_milli(month_milli) / 12
    }
}

/// 悬浮窗主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 深色玻璃（默认）
    Dark,
    /// 纯黑（OLED / 夜间）
    Black,
    /// 浅色
    Light,
    /// 跟随系统
    System,
}

impl Default for Theme {
    fn default() -> Self {
        Self::Dark
    }
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Black => "black",
            Self::Light => "light",
            Self::System => "system",
        }
    }
}

/// 数字字体。三档对应原型 03 章那三个 chip。
///
/// 落盘写的是 `as_str()` 那三个短名（`sf` / `jb` / `sys`）——和 `de_font` 认的、
/// 以及前端 `data-font` 用的是同一套。这就是 `Serialize` 要手写的原因：
/// 派生出来的是 `rename_all = "lowercase"` 那一套（`jetbrains` / `system`），
/// 读回来却只认短名，两套拼写一对不上，选过的人一重启就变回 SF Mono。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumFont {
    SfMono,
    JetBrains,
    System,
}

impl serde::Serialize for NumFont {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl Default for NumFont {
    fn default() -> Self {
        Self::SfMono
    }
}

impl NumFont {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SfMono => "sf",
            Self::JetBrains => "jb",
            Self::System => "sys",
        }
    }
}

/// 文案风格。三档对应原型 04 章「通知与文案」里那一行。
///
/// 它只决定「说哪句话」，一个字都不影响金额——所以它在 `Settings` 里是
/// 纯装饰字段，进快照之后前端换一张表就完事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CopyStyle {
    /// 情绪向（原型稿那套，默认）
    Emotional,
    /// 严谨：只陈述事实，不调侃
    Plain,
    /// 静默：不说话，只留数字
    Silent,
}

impl Default for CopyStyle {
    fn default() -> Self {
        Self::Emotional
    }
}

impl CopyStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Emotional => "emotional",
            Self::Plain => "plain",
            Self::Silent => "silent",
        }
    }
}

/// 点「×」时干什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseAction {
    Tray,
    Quit,
}

impl Default for CloseAction {
    fn default() -> Self {
        Self::Tray
    }
}

/// 四个全局快捷键。字符串形如 `Ctrl+Shift+P`，解析在 `platform` 里。
///
/// 顺序照原型稿：显隐 → 穿透 → 计时 → 额外收入。`#[serde(default)]` 让
/// 缺字段的旧配置文件照常读——加 `passthru` 之前存下来的 settings.json
/// 里没有这一项，没有这个标注会在反序列化时整份回落成默认值，
/// 表现是「升级后所有设置都没了」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyCfg {
    pub enabled: bool,
    pub show: String,
    pub passthru: String,
    pub clock: String,
    pub extra: String,
}

impl Default for HotkeyCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            show: "Ctrl+Shift+P".to_string(),
            passthru: "Ctrl+Shift+L".to_string(),
            clock: "Ctrl+Shift+Enter".to_string(),
            extra: "Ctrl+Shift+E".to_string(),
        }
    }
}

/// 下班通知报什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyEnd {
    Off,
    /// 只报金额
    Amount,
    /// 金额 + 工时
    AmountWork,
    /// 金额 + 等价物
    AmountEq,
}

impl Default for NotifyEnd {
    fn default() -> Self {
        Self::Amount
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyCfg {
    #[serde(deserialize_with = "de_notify_end")]
    pub end: NotifyEnd,
    /// 午休提醒
    pub lunch: bool,
    /// 提前几分钟提醒
    pub lunch_before_min: u32,
    /// 久坐提醒
    pub sit: bool,
    /// 间隔分钟
    pub sit_min: u32,
    /// 免打扰
    pub dnd: bool,
    pub dnd_from_min: u32,
    pub dnd_to_min: u32,
}

impl Default for NotifyCfg {
    fn default() -> Self {
        Self {
            end: NotifyEnd::Amount,
            lunch: true,
            lunch_before_min: 5,
            sit: false,
            sit_min: 60,
            dnd: false,
            dnd_from_min: 12 * 60,
            dnd_to_min: 13 * 60,
        }
    }
}

impl NotifyCfg {
    /// 现在是不是免打扰时段。
    ///
    /// 区间可以跨零点（22:00 → 08:00），所以不能写成 `from <= t && t < to`。
    pub fn in_dnd(&self, min_of_day: u32) -> bool {
        if !self.dnd {
            return false;
        }
        let (a, b) = (self.dnd_from_min, self.dnd_to_min);
        if a == b {
            return false;
        }
        if a < b {
            min_of_day >= a && min_of_day < b
        } else {
            min_of_day >= a || min_of_day < b
        }
    }
}

/// 全部用户可见配置。
///
/// `#[serde(default)]` 让「旧版本写的文件缺字段」不致命——
/// 升级加了新字段，老文件照样能读。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 月薪，单位毫。8000 元 → 8_000_000
    pub month_milli: i64,    /// 计薪日 × 100。21.75 天 → 2175
    pub pay_days_centi: u32,
    /// 每日工时，秒。7.5h → 27000
    pub daily_sec: u32,
    /// 加班倍率 × 100。1.5× → 150
    pub ot_rate_x100: u32,
    /// 加班怎么算：计费 / 只记白干
    #[serde(default, deserialize_with = "de_ot_mode")]
    pub ot_mode: OtMode,
    /// 下班后「我在加班」的起点（Unix 秒）。`None` = 没声明加班。
    ///
    /// 金额本身不需要它（金额是 f(已计薪秒) 的纯函数），它只决定界面状态。
    /// 但必须落盘：19:00 点了「我在加班」，19:30 重开程序，
    /// 状态不能退回「已下班」——否则金额会当场跳回去。
    pub ot_start: Option<i64>,
    /// 弹性排班：今天第一次活动的时间戳（Unix 秒）。
    ///
    /// 它必须落盘：11:00 才开始、用到一半重启程序，这一天不能重新从配置的
    /// 09:00 算起——那样重开一次就凭空多出两小时的钱。
    /// 只留最新一个；跨天之后 `today_anchor()` 会拒绝旧值。
    pub day_anchor: Option<i64>,
    /// 午休加班的区间列表。没收口的那段表示「现在正在记」。
    pub lunch_ots: Vec<LunchOtRec>,

    /// 窗口尺寸档位（紧凑 / 标准 / 展开）
    #[serde(default, deserialize_with = "de_size")]
    pub size: Size,

    /// 一天的作息。改它会同步改写 `daily_sec`（原型 04 章 calc-strip）。
    pub sched: SchedCfg,
    /// 一周怎么排班 + 法定节假日 / 调休补班
    pub week: WeekCfg,
    /// 年终奖摊分
    pub bonus: Bonus,
    /// 薪资口径：true = 税前（应发），false = 税后（到手）。
    /// 只影响界面上怎么称呼那个数，不改变任何计算。
    pub gross: bool,
    /// 时区策略：false = 跟随系统，true = 固定 UTC+8（Asia/Shanghai）。
    ///
    /// 为什么只有两档，没有原型里那个「按当日首次活动所在地」：
    /// 那一档要靠定位权限，而这个工具连网都不上。两档已经覆盖了真实场景——
    /// 出差别国时系统时区会跟着变，跟着变就意味着「09:00 上班」变成当地时间 09:00，
    /// 工作日被整体平移；固定住就等于「按公司那边的钟算」，这才是想问的事。
    ///
    /// 它只改**墙上时钟**的读法，不改任何时间戳：暂停、加班起始点存的都是 epoch 秒，
    /// 换时区不会让已经记下的账重算。
    pub tz_fixed: bool,
    /// 主题
    #[serde(deserialize_with = "de_theme")]
    pub theme: Theme,
    /// 数字字体
    #[serde(deserialize_with = "de_font")]
    pub num_font: NumFont,
    /// 文案风格
    #[serde(deserialize_with = "de_copy")]
    pub copy_style: CopyStyle,
    /// 开机自启（HKCU 的 Run 项）
    pub autostart: bool,
    /// 静默启动：只驻留托盘，不弹悬浮窗
    pub silent_start: bool,
    /// 边缘吸附阈值（逻辑像素）。0 = 关掉吸附
    pub snap_px: u32,
    /// 空闲检测：连续一段时间没键鼠活动就自动暂停
    pub idle_detect: bool,
    /// 空闲多久算离开（分钟）
    pub idle_min: u32,
    /// 点关闭按钮时干什么
    #[serde(deserialize_with = "de_close")]
    pub close_action: CloseAction,
    /// 全局快捷键
    pub hotkey: HotkeyCfg,
    /// 通知
    pub notify: NotifyCfg,

    /// 强调色。只影响「在岗」这类中性状态的色相——
    /// 午休琥珀、加班蓝、请假紫是**语义色**，不该被用户改掉。
    pub accent: String,
    /// 悬浮窗底色不透明度（50–100，%）
    pub opacity: u8,
    /// 信息密度开关
    pub density: Density,

    /// 请假类型名称（年假 / 事假 / 病假）
    pub leave_kind: String,
    /// 剩余额度，天 × 100。4.5 天 → 450
    pub leave_left_centi: u32,
    /// 请过假的日期，`YYYY-MM-DD`。
    ///
    /// 为什么存日期而不是「今天请假了没」一个布尔：只有存了日期，
    /// 「本月 -1 天」和「本月已赚要少算一天」才算得出来；
    /// 布尔到了第二天就失效，用户会看到数字自己跳回去。
    pub leave_days: Vec<String>,

    /// 等价物名称（展开态那句「已赚 ≈ 奶茶 11.6 杯」里的「奶茶」）
    pub eq_name: String,
    /// 等价物单价 × 100（分）。¥25 → 2500
    pub eq_price_centi: u32,
    /// 累计目标杯数（展开态第一行右槽「目标 1,000 杯」）
    pub eq_goal_cups: u32,
    /// 今日目标杯数（展开态第二行右槽「今日目标 12 杯」）
    pub eq_day_cups: u32,

    /// 托盘·隐藏金额（开会投屏时用）
    pub hide_money: bool,
    /// 托盘·鼠标穿透
    pub click_through: bool,
    /// 上次的窗口位置。`None` = 用默认的右上角。
    ///
    /// 它是**兜底**：真正被优先使用的是 `win_by_monitor` 里这一块屏那一份。
    /// 留着它有两个用处——老配置里只有这一个字段；以及在拿不到显示器名
    /// （某些虚拟屏返回空名）时还能记住位置。
    pub win: Option<WinPos>,
    /// 每块显示器各记一份位置，键是**显示器名**。
    ///
    /// 为什么按名字而不按索引：索引随插拔变（笔记本合盖再开，外接屏就从 1 变 0），
    /// 按索引记等于每次插拔都把位置记到别人头上。
    /// 名字也有不稳的时候（同一型号两块屏可能同名），所以这个字段只当"尽力而为"，
    /// 读出来之后一律过一遍 `clamp_into_screen`——记错了顶多回到屏幕内，不会把窗口丢在外面。
    pub win_by_monitor: std::collections::BTreeMap<String, WinPos>,
    /// 每次启动都摆到**主屏**右上角，不跟随上次所在的屏。
    ///
    /// 它只管启动那一下，不硬把窗口拉回来：拖到别的屏是用户的动作，
    /// 跟鼠标抢窗口比「不听话」更烦人。
    pub monitor_primary: bool,
    /// 暂停区间。扣减量由区间算出，不做累加——见 `paypulse_clock::PauseSpan`。
    pub pauses: Vec<PauseRec>,
    /// 一笔一笔记下来的额外进账（加班费、补贴、报销到账）。
    ///
    /// 它是**唯一**一处「与时间无关的钱」：日薪和时薪不认它，只在本月
    /// 已到账那一行加上去。所以它不进 `Params`，也不改 `calc_of`——
    /// 引擎里金额永远是 `f(now)`，这里补的是账面上的另一条线。
    #[serde(default)]
    pub extras: Vec<ExtraEntry>,

    /// 本版本不认识的字段，原样保留。
    ///
    /// 为什么留着：用户降级回旧版本时，新版本写的字段不该被吃掉——
    /// 否则「装回旧版看一眼再升上来」就丢配置了。
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            month_milli: 8_000_000,
            pay_days_centi: 2175,
            daily_sec: 27_000,
            ot_rate_x100: 150,
            ot_mode: OtMode::Shadow,
            ot_start: None,
            day_anchor: None,
            lunch_ots: Vec::new(),
            size: Size::Normal,
            sched: SchedCfg::default(),
            week: WeekCfg::default(),
            bonus: Bonus::default(),
            gross: true,
            tz_fixed: false,
            theme: Theme::Dark,
            num_font: NumFont::SfMono,
            copy_style: CopyStyle::Emotional,
            autostart: false,
            silent_start: false,
            snap_px: 24,
            idle_detect: true,
            idle_min: 5,
            close_action: CloseAction::Tray,
            hotkey: HotkeyCfg::default(),
            notify: NotifyCfg::default(),
            accent: "#35E07F".to_string(),
            opacity: 92,
            density: Density::default(),
            leave_kind: "年假".to_string(),
            leave_left_centi: 500,
            leave_days: Vec::new(),
            // 这三个是 07 章「设置」页的字段（原型 2516–2520 的等价物组）。
            // 默认值在这儿定死：奶茶 ¥25、目标 1000 杯、今日目标 12 杯——
            // 最后这个数在原型里就是硬编码的 12，不是从日薪推的。
            eq_name: "奶茶".to_string(),
            eq_price_centi: 2_500,
            eq_goal_cups: 1_000,
            eq_day_cups: 12,
            hide_money: false,
            click_through: false,
            win: None,
            win_by_monitor: std::collections::BTreeMap::new(),
            monitor_primary: false,
            pauses: Vec::new(),
            extras: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// 某月的额外进账合计，单位毫。`ym` 形如 `2026-11`。
    ///
    /// 只比前 7 个字符：日期是校验过的 `YYYY-MM-DD`，按字符串比就够了，
    /// 不必为了一次加法去解析日期。
    pub fn extras_in_month(&self, ym: &str) -> i64 {
        self.extras
            .iter()
            .filter(|e| e.date.get(..7) == Some(ym))
            .map(|e| e.amount_milli)
            .sum()
    }

    /// 把越界值夹回可用区间。
    ///
    /// 不是「严格校验后拒绝」：拒绝就等于让用户面对一个打不开的程序，
    /// 而夹回区间后用户能看到数字、能进设置改回来，体验差得远。
    pub fn validate(&mut self) {
        self.month_milli = self.month_milli.clamp(MONTH_MIN, MONTH_MAX);
        // 分母，必须 > 0
        self.pay_days_centi = self.pay_days_centi.clamp(1, 3_100);
        // 分母，且 0 秒会让「每秒多少钱」变成无穷大
        self.daily_sec = self.daily_sec.clamp(1, 86_400);
        self.ot_rate_x100 = self.ot_rate_x100.min(1_000);
        // 位置宽高都按 i32 存，这里只防明显离谱的值（多屏拼接的负坐标是合法的）
        if let Some(p) = self.win {
            if p.x.abs() > 100_000 || p.y.abs() > 100_000 {
                self.win = None;
            }
        }

        // 每屏一份位置：坐标离谱的、名字空/超长的都丢掉。
        // 条目数封顶 8——这个数只防「文件被写坏塞进几百条」，
        // 真插八块屏的机器极少，而多出来的条目除了让文件变脏没有别的后果。
        // 丢弃顺序取名字最小的那条，是为了让「同一份坏文件读两次结果一样」。
        self.win_by_monitor.retain(|name, p| {
            !name.trim().is_empty()
                && name.chars().count() <= 128
                && p.x.abs() <= 100_000
                && p.y.abs() <= 100_000
        });
        while self.win_by_monitor.len() > 8 {
            let Some(first) = self.win_by_monitor.keys().next().cloned() else {
                break;
            };
            self.win_by_monitor.remove(&first);
        }

        // 等价物单价是除数。0 会让「折合几杯」变成除以零，
        // 而这个值可以从文件里手填，所以必须在这里挡住。
        self.eq_price_centi = self.eq_price_centi.clamp(1, 100_000_000);
        // 目标杯数同样是除数（算那两条小进度条），0 会让进度恒为 0/0。
        self.eq_goal_cups = self.eq_goal_cups.clamp(1, 1_000_000);
        self.eq_day_cups = self.eq_day_cups.clamp(1, 10_000);
        // 名字会直接印在版面上：太长会把 322px 那一行撑开、把右槽挤出去。
        // 按字符数截，不能按字节——按字节切会切出半个汉字。
        if self.eq_name.chars().count() > 6 {
            self.eq_name = self.eq_name.chars().take(6).collect();
        }
        if self.eq_name.trim().is_empty() {
            self.eq_name = "奶茶".to_string();
        }

        // 暂停区间：结束早于开始的直接丢掉（手改文件、系统时间被回拨都会造出来）。
        // 留着会让扣减量变成负数，而负数会在 saturating_sub 那里静默吃掉一截工时。
        self.pauses.retain(|p| match p.end {
            Some(e) => e >= p.start,
            None => true,
        });

        // 额外进账：日期得是 `YYYY-MM-DD`，金额得是正的。
        //
        // 手改文件能造出 `2026-13-45`——那笔钱永远落不进任何一个月，
        // 用户只会觉得「记了但没算」；负数等于无中生有一笔支出。
        // 条数封顶 500：这是账本，不是日志，正常一年也就几十笔。
        self.extras
            .retain(|e| is_iso_date(&e.date) && e.amount_milli > 0 && e.amount_milli <= MONTH_MAX);
        if self.extras.len() > 500 {
            self.extras.sort_by(|a, b| a.date.cmp(&b.date));
            let drop = self.extras.len() - 500;
            self.extras.drain(..drop);
        }

        // 午休加班：封顶 0 会让白干永远是 0（等于没记），当成没记过；
        // 结束早于开始的丢掉（手改文件、时钟回拨都会造出来）。
        const DAY: u32 = 86_400;
        let mut kept: Vec<LunchOtRec> = Vec::new();
        for l in self.lunch_ots.iter().copied() {
            if l.cap_sec == 0 {
                continue;
            }
            if let Some(e) = l.end {
                if e < l.start {
                    continue;
                }
            }
            kept.push(LunchOtRec {
                start: l.start,
                end: l.end,
                cap_sec: l.cap_sec.min(DAY),
            });
        }
        // 只允许最后一段没收口。两段同时「正在记」是坏数据，
        // 会让白干时长凭空多出一截。
        let last_open = kept.iter().rposition(|l| l.end.is_none());
        self.lunch_ots = kept
            .into_iter()
            .enumerate()
            .filter(|(i, l)| l.end.is_some() || Some(*i) == last_open)
            .map(|(_, l)| l)
            .collect();
        // 只保留最后一段未结束的：两段同时「进行中」是坏数据，
        // `is_paused` 只看最后一段，前面那段会永远开着、扣减量一直涨。
        // 注意别用 `iter().rev().filter(..).rev()` —— 第二个 rev 会让 filter
        // 从尾部开始驱动，闭包看到的顺序正好相反，留下的是最旧那段。
        let last_open = self.pauses.iter().rposition(|p| p.end.is_none());
        self.pauses = self
            .pauses
            .iter()
            .enumerate()
            .filter(|(i, p)| p.end.is_some() || Some(*i) == last_open)
            .map(|(_, p)| *p)
            .collect();

        // 作息是整套时间判断的地基，先夹回合法区间
        self.sched.sanitize();

        // 排班：大小周的奇偶只能是 0/1；两张日期表去重排序并限长
        if let Some(p) = self.week.alt_sat {
            self.week.alt_sat = Some(p.min(1));
        }
        self.week.holidays = tidy_days(&self.week.holidays);
        self.week.makeup = tidy_days(&self.week.makeup);

        // 年终奖：月数上限 12（= 24 薪），比例上限 500%，金额上限一年月薪。
        // 这几个都是「乘进月薪」的数，手滑多一个零会让时薪离谱到没人信。
        self.bonus.months_x100 = self.bonus.months_x100.min(1_200);
        self.bonus.amount_milli = self.bonus.amount_milli.clamp(0, MONTH_MAX);
        self.bonus.rate_x100 = self.bonus.rate_x100.min(50_000);

        // 吸附阈值：0 = 关闭；上限 64pt，再大就成了「窗口自己跑」
        self.snap_px = self.snap_px.min(64);
        // 空闲判定 1–120 分钟：0 会让「刚坐下就被判定离开」
        self.idle_min = self.idle_min.clamp(1, 120);

        // 通知：提醒提前量不能超过午休本身；久坐间隔 15–240 分钟
        self.notify.lunch_before_min = self.notify.lunch_before_min.min(60);
        self.notify.sit_min = self.notify.sit_min.clamp(15, 240);
        self.notify.dnd_from_min = self.notify.dnd_from_min.min(24 * 60 - 1);
        self.notify.dnd_to_min = self.notify.dnd_to_min.min(24 * 60);

        // 强调色会被写进 `style.setProperty("--user-accent", …)`。
        // 只放行 #RRGGBB：其它形状（空串、`red;}` 这种）不该有机会进 CSS。
        if !is_hex_color(&self.accent) {
            self.accent = "#35E07F".to_string();
        }
        // 全透明就看不见了，全不透明就失去「玻璃」的意义
        self.opacity = self.opacity.clamp(50, 100);

        // 请假类型会直接印在版面上，和等价物名字同样的处理
        self.leave_kind = self.leave_kind.trim().to_string();
        if self.leave_kind.chars().count() > 4 {
            self.leave_kind = self.leave_kind.chars().take(4).collect();
        }
        if self.leave_kind.is_empty() {
            self.leave_kind = "年假".to_string();
        }
        // 余额上限一年：365 天。多出来的手滑输入没有意义。
        self.leave_left_centi = self.leave_left_centi.min(36_500);

        // 请假日期：认不出来的丢掉、重复的合并、按时间排好。
        // 不排序会让「本月 -N 天」的遍历每次都不一样，调试时看不出对错。
        let mut days: Vec<NaiveDate> = self
            .leave_days
            .iter()
            .filter_map(|s| parse_day(s))
            .collect();
        days.sort_unstable();
        days.dedup();
        // 十年够用了。这个上限纯粹是防「文件被写坏了塞进十万条」
        days.truncate(3_650);
        self.leave_days = days.iter().map(|d| d.to_string()).collect();
    }

    /// 这一天请假了没。
    pub fn is_leave_day(&self, d: NaiveDate) -> bool {
        let key = d.to_string();
        self.leave_days.iter().any(|s| *s == key)
    }

    /// 本月的请假统计：`(本月共几天, 今天之前几天, 今天之后几天)`。
    ///
    /// **只数工作日**：请假记录里万一手滑标了个周六，那天本来就不计薪，
    /// 再扣一次就等于一天扣两天钱。
    pub fn leave_stats(&self, today: NaiveDate, cal: &paypulse_clock::Calendar) -> (u32, u32, u32) {
        let (mut all, mut before, mut after) = (0, 0, 0);
        for d in self.leave_dates() {
            if d.year() != today.year() || d.month() != today.month() {
                continue;
            }
            // 用日历判「这天本来要不要上班」：单休的人周六请假是真的少一天钱，
            // 而双休的人周六请假本来就不计薪，不该再扣。
            if !cal.is_workday(d) {
                continue;
            }
            all += 1;
            if d < today {
                before += 1;
            } else if d > today {
                after += 1;
            }
        }
        (all, before, after)
    }

    /// 落盘的日期逐个解析。解析失败的记录在 `validate` 里已经被清掉了，
    /// 这里再 `filter_map` 一次是给「绕过 load 直接构造」的调用方兜底。
    pub fn leave_dates(&self) -> impl Iterator<Item = NaiveDate> + '_ {
        self.leave_days.iter().filter_map(|s| parse_day(s))
    }

    /// 现在是否正在记午休加班。
    pub fn lunch_ot_open(&self) -> bool {
        self.lunch_ots.last().map_or(false, |l| l.end.is_none())
    }

    /// 收口当前这段午休加班。已经在记才需要调。
    pub fn close_lunch_ot(&mut self, now: i64) {
        if let Some(last) = self.lunch_ots.last_mut() {
            if last.end.is_none() {
                last.end = Some(now.max(last.start));
            }
        }
    }

    /// 转成时钟层的出勤日历。
    ///
    /// 这是配置与计算之间唯一的接缝：`Settings` 认识 JSON，`Calendar` 不认识。
    pub fn calendar(&self) -> paypulse_clock::Calendar {
        self.calendar_shifted(0)
    }

    /// 带弹性偏移的日历：`delta` 是「整套作息沿时间轴平移多少分钟」。
    ///
    /// 平移而不是替换起点，是因为午休和下班时刻要跟着一起走——
    /// 只改 start_min 会让午休还卡在 12:00，于是 10:30 上班的人午休前
    /// 只干了 1.5 小时，白干条会莫名其妙地出现在中午。
    pub fn calendar_shifted(&self, delta: i32) -> paypulse_clock::Calendar {
        let sched = if delta == 0 { self.sched } else { self.sched.shifted(delta) };
        paypulse_clock::Calendar {
            shift: sched.shift(),
            days: self.week.days,
            alt_sat: self.week.alt_sat,
            holidays: self.week.holiday_dates().collect(),
            makeup: self.week.makeup_dates().collect(),
        }
    }

    /// 弹性排班下「今天从几点开始」。0 表示不平移。
    ///
    /// 只认**今天**的锚点：昨天那个锚点留在配置里没关系，但拿它来平移今天的班次，
    /// 会把今天的上班点定在昨天那个时间上，而且金额会当场从那个点开始算。
    pub fn flex_delta(&self, now: chrono::DateTime<chrono::FixedOffset>) -> i32 {
        let Some(anchor) = self.today_anchor(now) else {
            return 0;
        };
        let start = self.sched.start_min as i32;
        let at = (anchor.hour() * 60 + anchor.minute()) as i32;
        // 刻意**不**做 rem_euclid：08:00 上班的人相对 09:00 的配置应该是 −60，
        // 包成 1380 会把整套作息推到第二天，金额当场变成满勤。
        at - start
    }

    /// 今天的锚点时刻（已按当前时区解释）。没有「今天的」就返回 `None`。
    pub fn today_anchor(
        &self,
        now: chrono::DateTime<chrono::FixedOffset>,
    ) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        if !self.week.flex {
            return None;
        }
        let a = self.day_anchor?;
        let t = now.timezone().timestamp_opt(a, 0).single()?;
        if t.date_naive() == now.date_naive() {
            Some(t)
        } else {
            None
        }
    }

    /// 转成时钟层要的类型。
    pub fn spans(&self) -> Vec<paypulse_clock::PauseSpan> {        self.pauses
            .iter()
            .map(|p| paypulse_clock::PauseSpan {
                start: p.start,
                end: p.end,
            })
            .collect()
    }
}

pub fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// `#RRGGBB`，别的形状一律不认。
fn is_hex_color(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(|c| c.is_ascii_hexdigit())
}

/// `YYYY-MM-DD` → 日期。认不出来就是 `None`，绝不 panic。
fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// 宽松地读 `ot_mode`：认不出来的值退回默认，而不是让整个文件解析失败。
///
/// 标准 derive 遇到未知枚举值会报错，而报错在 `load_from` 里的后果是
/// 「整份配置被备份、全部退回默认值」——用户只是手滑把 "shadow" 打错一个字母，
/// 却把月薪、窗口位置、暂停记录一起丢了。这里宁可只丢这一个字段。
fn de_ot_mode<'de, D: serde::Deserializer<'de>>(d: D) -> Result<OtMode, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "paid" => OtMode::Paid,
        _ => OtMode::Shadow,
    })
}

/// 宽松地读 `size`。理由同 `de_ot_mode`：认不出来的值只该丢这一个字段。
fn de_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Size, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "compact" => Size::Compact,
        "expanded" => Size::Expanded,
        _ => Size::Normal,
    })
}

fn de_theme<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Theme, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "black" => Theme::Black,
        "light" => Theme::Light,
        "system" => Theme::System,
        _ => Theme::Dark,
    })
}

fn de_font<'de, D: serde::Deserializer<'de>>(d: D) -> Result<NumFont, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "jb" => NumFont::JetBrains,
        "sys" => NumFont::System,
        _ => NumFont::SfMono,
    })
}

fn de_copy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<CopyStyle, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "plain" => CopyStyle::Plain,
        "silent" => CopyStyle::Silent,
        _ => CopyStyle::Emotional,
    })
}

fn de_close<'de, D: serde::Deserializer<'de>>(d: D) -> Result<CloseAction, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "quit" => CloseAction::Quit,
        _ => CloseAction::Tray,
    })
}

fn de_bonus<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BonusMode, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "months" => BonusMode::Months,
        "amount" => BonusMode::Amount,
        "rate" => BonusMode::Rate,
        _ => BonusMode::None,
    })
}

fn de_notify_end<'de, D: serde::Deserializer<'de>>(d: D) -> Result<NotifyEnd, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "off" => NotifyEnd::Off,
        "amountwork" => NotifyEnd::AmountWork,
        "amounteq" => NotifyEnd::AmountEq,
        _ => NotifyEnd::Amount,
    })
}

/// 日期表清洗：认不出来的丢掉、重复的合并、排好序、限长。
///
/// 排序不是为了好看——「下一个法定假日还有几天」要靠顺序扫，
/// 不排序的话同一个文件每次读出来结果可能不一样。
fn tidy_days(raw: &[String]) -> Vec<String> {
    let mut v: Vec<NaiveDate> = raw.iter().filter_map(|s| parse_day(s)).collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(1_000);
    v.iter().map(|d| d.to_string()).collect()
}

/// 读配置。**任何失败都返回默认值**，不返回 Result——
/// 这个函数在启动路径上，返回值类型不允许调用方选择「失败就退出」。
pub fn load_from(dir: &Path) -> Settings {
    let path = settings_path(dir);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        // 文件不存在是最常见的情况（首次启动），不必吵闹
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            eprintln!("读配置失败（{e}），改用默认值：{}", path.display());
            return Settings::default();
        }
    };

    match serde_json::from_str::<Settings>(&raw) {
        Ok(mut s) => {
            s.validate();
            s
        }
        Err(e) => {
            // 把坏文件挪走而不是删掉：用户可能想自己找回来。
            let bad = path.with_extension("json.bad");
            let moved = fs::rename(&path, &bad).is_ok();
            eprintln!(
                "配置解析失败（{e}），改用默认值；原文件{}",
                if moved {
                    format!("已备份为 {}", bad.display())
                } else {
                    "备份失败，下次启动仍会读它".to_string()
                }
            );
            Settings::default()
        }
    }
}

/// 原子写：写 tmp → fsync → rename。
///
/// `rename` 在同一分区上是原子的，所以任何时刻磁盘上的 `settings.json`
/// 要么是旧的完整内容，要么是新的完整内容，不会是半截。
pub fn save_to(dir: &Path, s: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = settings_path(dir);
    let tmp = dir.join("settings.json.tmp");

    let body = serde_json::to_string_pretty(s)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        // 不 sync 的话，rename 可能先于内容落盘，掉电后得到空文件
        f.sync_all()?;
    }

    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "paypulse-settings-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let d = tmpdir("missing");
        let s = load_from(&d);
        assert_eq!(s, Settings::default(), "首次启动应拿到默认值");
        assert!(!settings_path(&d).exists(), "读不该顺手创建文件");
    }

    #[test]
    fn partial_file_fills_missing_fields() {
        let d = tmpdir("partial");
        fs::write(settings_path(&d), r#"{"month_milli": 12000000}"#).unwrap();
        let s = load_from(&d);
        assert_eq!(s.month_milli, 12_000_000, "写了的字段要生效");
        assert_eq!(s.pay_days_centi, 2175, "没写的字段用默认");
        assert_eq!(s.daily_sec, 27_000);
    }

    /// 存盘再读回来，每一项都得是原样——尤其是字体。
    ///
    /// 这条是为 `NumFont` 写的：落盘走的是 serde 的 `rename_all = "lowercase"`
    /// （写出 "jetbrains" / "system"），读回来走的是手写的 `de_font`（只认
    /// "jb" / "sys"）。两套拼写对不上，选过 JetBrains 或系统等宽的人一重启
    /// 就被悄悄换回 SF Mono——界面上看不出来，直到他再打开设置页，
    /// 发现高亮跑回了第一张卡。
    #[test]
    fn the_font_choice_survives_a_restart() {
        let d = tmpdir("font");
        for f in [NumFont::SfMono, NumFont::JetBrains, NumFont::System] {
            let s = Settings {
                num_font: f,
                ..Default::default()
            };
            save_to(&d, &s).unwrap();
            let back = load_from(&d);
            assert_eq!(back.num_font, f, "{} 没活过重启", f.as_str());
        }
    }

    /// 升级兼容：加 `passthru` 之前存下来的配置文件里没有这一项。
    ///
    /// `HotkeyCfg` 上的 `#[serde(default)]` 就是为这一条加的。少了它，
    /// 反序列化会因为「缺字段」整份失败，`load_from` 回落成默认值——
    /// 用户看到的是「更新之后什么都变回默认了」，比少一个快捷键难查得多。
    #[test]
    fn an_old_config_without_passthru_still_loads() {
        let old = r#"{"hotkey":{"enabled":true,"show":"Ctrl+Alt+P",
                       "clock":"Ctrl+Alt+Enter","extra":"Ctrl+Alt+E"}}"#;
        let s: Settings = serde_json::from_str(old).expect("旧配置要能读");
        assert_eq!(s.hotkey.show, "Ctrl+Alt+P", "别的字段不能被连坐");
        assert_eq!(s.hotkey.passthru, "Ctrl+Shift+L", "缺的那项取默认");
    }

    /// 一整份「什么都改过」的配置，存回去再读回来必须一字不差。
    ///
    /// 单个字段的往返错最阴：选过的字体悄悄变回默认，用户只会觉得「这软件记不住事」。
    /// 这里把每个枚举的**非默认**那一档都摆上去，任何一处拼写不一致都当场现形。
    /// 存之前先 `validate()`：清洗是读盘时就该发生的事，不算「没记住」。
    #[test]
    fn every_setting_survives_a_restart() {
        let d = tmpdir("roundtrip");
        let mut s = Settings {
            month_milli: 12_345_678,
            pay_days_centi: 2_000,
            daily_sec: 30_000,
            ot_rate_x100: 200,
            ot_mode: OtMode::Paid,
            ot_start: Some(1_700_000_000),
            size: Size::Expanded,
            gross: false,
            tz_fixed: true,
            theme: Theme::Light,
            num_font: NumFont::JetBrains,
            copy_style: CopyStyle::Silent,
            autostart: true,
            silent_start: true,
            snap_px: 20,
            idle_detect: true,
            idle_min: 10,
            close_action: CloseAction::Quit,
            eq_name: "咖啡".into(),
            eq_price_centi: 3_800,
            eq_goal_cups: 500,
            eq_day_cups: 6,
            accent: "#35E07F".into(),
            opacity: 74,
            leave_kind: "调休".into(),
            leave_left_centi: 150,
            leave_days: vec!["2026-11-10".into(), "2026-11-11".into()],
            hide_money: true,
            click_through: true,
            pauses: vec![PauseRec {
                start: 1_700_000_100,
                end: Some(1_700_000_200),
            }],
            ..Default::default()
        };
        s.sched.start_min = 21 * 60;
        s.sched.end_min = 6 * 60;
        s.sched.cross_midnight = true;
        s.week.days = [true, true, true, true, true, true, false];
        s.week.alt_sat = Some(1);
        s.week.holidays = vec!["2026-10-01".into(), "2026-10-02".into()];
        s.week.makeup = vec!["2026-10-10".into()];
        s.bonus.mode = BonusMode::Rate;
        s.bonus.rate_x100 = 2_000;
        s.hotkey.enabled = false;
        s.hotkey.show = "Ctrl+Alt+K".into();
        s.hotkey.passthru = "Ctrl+Alt+L".into();
        s.notify.end = NotifyEnd::AmountEq;
        s.notify.sit = true;
        s.notify.dnd = true;
        s.notify.dnd_from_min = 22 * 60;
        s.notify.dnd_to_min = 8 * 60;
        s.validate();

        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d), s);
    }

    /// 每屏位置：空名字丢掉、坐标离谱的丢掉、条目封顶 8 条。
    ///
    /// 这三条都是「文件被写坏」的防线，不是常规路径——正常用只会有一两条。
    #[test]
    fn monitor_positions_are_pruned() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.win_by_monitor
                .insert(format!("MON-{i:02}"), WinPos { x: i, y: i });
        }
        s.win_by_monitor.insert("".into(), WinPos { x: 1, y: 1 });
        s.win_by_monitor.insert("BAD".into(), WinPos { x: 999_999, y: 0 });
        s.validate();

        assert_eq!(s.win_by_monitor.len(), 8, "条目最多留 8 条");
        assert!(!s.win_by_monitor.contains_key(""), "空名字的丢掉");
        assert!(!s.win_by_monitor.contains_key("BAD"), "坐标离谱的丢掉");
    }

    #[test]
    fn broken_file_falls_back_and_keeps_evidence() {
        let d = tmpdir("broken");
        // 典型场景：上次写入时断电，得到半截 JSON
        fs::write(settings_path(&d), r#"{"month_milli": 8000"#).unwrap();
        let s = load_from(&d);
        assert_eq!(s, Settings::default(), "坏文件必须退回默认值，不能 panic");
        assert!(
            d.join("settings.json.bad").exists(),
            "坏文件要留证，用户可能想自己找回来"
        );
        assert!(!settings_path(&d).exists(), "坏文件不应留在原位继续害人");
    }

    #[test]
    fn zero_pay_days_cannot_reach_the_engine() {
        let d = tmpdir("zero");
        // 这是最危险的一个：计薪日是引擎的分母
        fs::write(
            settings_path(&d),
            r#"{"pay_days_centi": 0, "daily_sec": 0}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert!(s.pay_days_centi > 0, "分母不能是 0");
        assert!(s.daily_sec > 0, "分母不能是 0");

        // 真算一遍：这里若 panic，说明校验没挡住
        let p = paypulse_core::Params {
            month_milli: s.month_milli,
            pay_days_centi: s.pay_days_centi,
            daily_sec: s.daily_sec,
            ot_rate_x100: s.ot_rate_x100,
            shadow_rate_x100: 150,
            lunch_shadow_rate_x100: 100,
            bonus_milli: 0,
        };
        let _ = paypulse_core::calc_of(27_000, paypulse_core::State::On, &p);
        let _ = p.day_milli();
        let _ = p.per_sec_milli();
    }

    #[test]
    fn absurd_month_is_clamped_not_rejected() {
        let d = tmpdir("absurd");
        fs::write(settings_path(&d), r#"{"month_milli": -5}"#).unwrap();
        assert_eq!(load_from(&d).month_milli, MONTH_MIN);

        fs::write(settings_path(&d), r#"{"month_milli": 999999999999999}"#).unwrap();
        assert_eq!(load_from(&d).month_milli, MONTH_MAX);
    }

    #[test]
    fn unknown_fields_survive_round_trip() {
        let d = tmpdir("unknown");
        fs::write(
            settings_path(&d),
            // 这两个名字必须**永远不是真字段**，否则这条测试会自己过期
            // （上一次它用的就是 "theme"，等 theme 被实现就挂了）。
            r#"{"month_milli": 9000000, "future_knob": "dark", "beta_flag": 7}"#,
        )
        .unwrap();

        let mut s = load_from(&d);
        assert_eq!(s.month_milli, 9_000_000);
        assert_eq!(s.unknown.len(), 2, "不认识的字段要收起来");

        // 用户改个设置再存盘，不能把未来版本的字段吃掉
        s.hide_money = true;
        save_to(&d, &s).unwrap();

        let again = load_from(&d);
        assert_eq!(again.unknown.get("future_knob").unwrap(), "dark");
        assert_eq!(again.unknown.get("beta_flag").unwrap(), 7);
        assert!(again.hide_money);
    }

    #[test]
    fn save_leaves_no_tmp_behind() {
        let d = tmpdir("atomic");
        let mut s = Settings::default();
        s.win = Some(WinPos { x: 1900, y: 24 });
        save_to(&d, &s).unwrap();

        assert!(!d.join("settings.json.tmp").exists(), "tmp 应该被 rename 掉");
        let back = load_from(&d);
        assert_eq!(back.win, Some(WinPos { x: 1900, y: 24 }));

        // 再存一次（覆盖已存在的目标文件）——Windows 上 rename 到已存在文件
        // 容易踩坑，这条测试就是盯着它
        s.win = Some(WinPos { x: 100, y: 200 });
        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d).win, Some(WinPos { x: 100, y: 200 }));
    }

    #[test]
    fn offscreen_position_is_dropped() {
        // 用户把窗口拖到某个显示器上，之后那台显示器拔了/虚拟屏消失了，
        // 窗口就再也找不回来。离谱坐标直接丢掉，退回默认位置。
        let d = tmpdir("offscreen");
        fs::write(settings_path(&d), r#"{"win": {"x": 9999999, "y": 5}}"#).unwrap();
        assert_eq!(load_from(&d).win, None);

        // 但多屏拼接的负坐标是合法的，不能误杀
        fs::write(settings_path(&d), r#"{"win": {"x": -1920, "y": 24}}"#).unwrap();
        assert_eq!(load_from(&d).win, Some(WinPos { x: -1920, y: 24 }));
    }

    #[test]
    fn backwards_pause_span_is_dropped() {
        let d = tmpdir("bwspan");
        // 结束早于开始：手改文件或系统时间被回拨才会造出来。
        // 留着会让扣减量变负，静默吃掉一截工时。
        fs::write(
            settings_path(&d),
            r#"{"pauses": [{"start": 2000, "end": 1000}, {"start": 3000, "end": 3600}]}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.pauses.len(), 1, "倒着走的区间要丢掉");
        assert_eq!(s.pauses[0].start, 3000);
    }

    #[test]
    fn only_the_last_pause_may_be_open() {
        let d = tmpdir("open");
        // 两段都写着「未结束」是坏数据：is_paused 只看最后一段，
        // 前一段会永远开着，扣减量一直涨下去。
        fs::write(
            settings_path(&d),
            r#"{"pauses": [{"start": 1000, "end": null}, {"start": 2000, "end": null}]}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.pauses.len(), 1);
        assert_eq!(s.pauses[0].start, 2000, "留最后那段");
    }

    /// 默认是「不计加班费」——跟原型稿设置页里那个预选项一致。
    /// 这不是随手定的：多数人加班确实拿不到加班费，默认记白干才不算骗人。
    #[test]
    fn default_ot_mode_is_shadow() {
        assert_eq!(Settings::default().ot_mode, OtMode::Shadow);
        assert_eq!(Settings::default().ot_start, None);
        assert!(Settings::default().lunch_ots.is_empty());
    }

    #[test]
    fn ot_mode_round_trips_through_json() {
        let d = tmpdir("otmode");
        let mut s = Settings::default();
        s.ot_mode = OtMode::Paid;
        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d).ot_mode, OtMode::Paid);

        s.ot_mode = OtMode::Shadow;
        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d).ot_mode, OtMode::Shadow);
    }

    /// 手滑把 "shadow" 打成 "shdow"，只该丢这一个字段，
    /// 不能因为一个枚举值不认识就把整份配置备份走、全部退回默认。
    #[test]
    fn typo_in_ot_mode_does_not_nuke_the_whole_file() {
        let d = tmpdir("ottypo");
        fs::write(
            settings_path(&d),
            r#"{"ot_mode": "shdow", "month_milli": 9000000, "hide_money": true}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.ot_mode, OtMode::Shadow, "认不出来的值退回默认");
        assert_eq!(s.month_milli, 9_000_000, "别的字段必须还在");
        assert!(s.hide_money);
        assert!(!d.join("settings.json.bad").exists(), "不该被当成坏文件");
    }

    /// 封顶 0 的午休加班等于「永远记 0 秒」，是坏数据，当成没记过。
    #[test]
    fn zero_cap_lunch_ot_is_dropped() {
        let d = tmpdir("lunchcap");
        fs::write(
            settings_path(&d),
            r#"{"lunch_ots": [{"start": 1000, "end": null, "cap_sec": 0}]}"#,
        )
        .unwrap();
        assert!(load_from(&d).lunch_ots.is_empty());

        fs::write(
            settings_path(&d),
            r#"{"lunch_ots": [{"start": 1000, "end": null, "cap_sec": 999999999}]}"#,
        )
        .unwrap();
        assert_eq!(load_from(&d).lunch_ots[0].cap_sec, 86_400, "离谱封顶要夹回来");
    }

    /// 两段同时「正在记」是坏数据：只会让白干时长凭空多一截。
    #[test]
    fn only_the_last_lunch_ot_may_be_open() {
        let d = tmpdir("lunchopen");
        fs::write(
            settings_path(&d),
            r#"{"lunch_ots": [
                 {"start": 1000, "end": null, "cap_sec": 600},
                 {"start": 2000, "end": null, "cap_sec": 600}
               ]}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.lunch_ots.len(), 1);
        assert_eq!(s.lunch_ots[0].start, 2000, "留最后那段");
    }

    /// 收口之后长度就固定了，不再随时间增长。
    #[test]
    fn closing_lunch_ot_freezes_its_length() {
        let mut s = Settings::default();
        s.lunch_ots.push(LunchOtRec {
            start: 1000,
            end: None,
            cap_sec: 5_400,
        });
        assert!(s.lunch_ot_open());
        s.close_lunch_ot(1600);
        assert!(!s.lunch_ot_open());
        assert_eq!(s.lunch_ots[0].len(9_999), 600, "收了口就按 600 秒算，不能跟着 now 涨");
    }

    /// 加班声明的时间点要能存住：重启后 19:00 那次「我在加班」不能丢，
    /// 丢了状态会退回「已下班」，金额当场跳掉。
    #[test]
    fn declared_overtime_survives_restart() {
        let d = tmpdir("otstart");
        let mut s = Settings::default();
        s.ot_start = Some(1_762_768_800);
        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d).ot_start, Some(1_762_768_800));
    }

    /// 老版本写的文件里没有这些新字段，照样要能读。
    #[test]
    fn old_file_without_new_fields_still_loads() {
        let d = tmpdir("oldschema");
        fs::write(
            settings_path(&d),
            r#"{"month_milli": 7500000, "pay_days_centi": 2175, "daily_sec": 27000, "ot_rate_x100": 150}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.month_milli, 7_500_000);
        assert_eq!(s.ot_mode, OtMode::Shadow);
        assert!(s.lunch_ots.is_empty());
    }

    /// 三档循环：紧凑 → 标准 → 展开 → 紧凑。
    /// 往回退一步是最容易写错的那半边（`%` 在负数上会给出负值）。
    #[test]
    fn size_cycles_both_directions() {
        assert_eq!(Size::Compact.cycle(1), Size::Normal);
        assert_eq!(Size::Normal.cycle(1), Size::Expanded);
        assert_eq!(Size::Expanded.cycle(1), Size::Compact, "到头要绕回去");

        assert_eq!(Size::Compact.cycle(-1), Size::Expanded, "往回退要绕到最后一档");
        assert_eq!(Size::Normal.cycle(-1), Size::Compact);
        assert_eq!(Size::Expanded.cycle(-1), Size::Normal);

        // 连转三圈必须回到原点
        let mut s = Size::Normal;
        for _ in 0..3 {
            s = s.cycle(1);
        }
        assert_eq!(s, Size::Normal);
        for _ in 0..3 {
            s = s.cycle(-1);
        }
        assert_eq!(s, Size::Normal);
    }

    #[test]
    fn size_round_trips_and_survives_a_typo() {
        let d = tmpdir("size");
        let mut s = Settings::default();
        assert_eq!(s.size, Size::Normal, "默认是标准档");

        s.size = Size::Compact;
        save_to(&d, &s).unwrap();
        assert_eq!(load_from(&d).size, Size::Compact);

        // 手滑写成不认识的值：只该丢这一个字段，不能整份配置作废
        fs::write(
            settings_path(&d),
            r#"{"size": "tiny", "month_milli": 9000000}"#,
        )
        .unwrap();
        let back = load_from(&d);
        assert_eq!(back.size, Size::Normal);
        assert_eq!(back.month_milli, 9_000_000);
        assert!(!d.join("settings.json.bad").exists(), "不该被当成坏文件");
    }

    /// 单价是除数。0 会让「折合几杯」除以零，而它可以从文件里手填。
    #[test]
    fn zero_equivalent_price_cannot_reach_the_divider() {
        let d = tmpdir("eqprice");
        fs::write(
            settings_path(&d),
            r#"{"eq_price_centi": 0, "eq_goal_cups": 0, "eq_day_cups": 0}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert!(s.eq_price_centi >= 1, "单价不能是 0");
        assert!(s.eq_goal_cups >= 1, "目标杯数是除数，不能是 0");
        assert!(s.eq_day_cups >= 1);
    }

    /// 等价物名字直接印在版面上：太长会把 322px 那一行撑开，把右槽挤出去。
    #[test]
    fn equivalent_name_is_trimmed_by_char_not_by_byte() {
        let d = tmpdir("eqname");
        fs::write(
            settings_path(&d),
            r#"{"eq_name": "超大杯珍珠奶茶加倍"}"#,
        )
        .unwrap();
        let name = load_from(&d).eq_name;
        assert_eq!(name.chars().count(), 6, "按字符截，不按字节");
        assert!(name.chars().all(|c| c != '\u{FFFD}'), "不能切出半个汉字");

        fs::write(settings_path(&d), r#"{"eq_name": "   "}"#).unwrap();
        assert_eq!(load_from(&d).eq_name, "奶茶", "空白名字退回默认");
    }

    /// 「记一笔」的输入解析：元 → 毫，全程整数。
    ///
    /// 这组断言的用处是把「12.34 会不会变成 123400 还是 123399」钉死。
    /// 外壳里做这个乘法，`12.34 * 10000` 得到 123399.99999999999，
    /// 四舍五入后少一毫——单笔看不出来，一个月几十笔就是几厘的漂移。
    #[test]
    fn yuan_text_turns_into_milli_without_floating_point() {
        assert_eq!(parse_yuan("300"), Some(3_000_000));
        assert_eq!(parse_yuan("12.34"), Some(123_400));
        assert_eq!(parse_yuan("12.3"), Some(123_000));
        assert_eq!(parse_yuan("0.0001"), Some(1), "最小单位是毫");
        assert_eq!(parse_yuan("0"), Some(0));
        assert_eq!(parse_yuan("1,200"), Some(12_000_000), "千分位能粘贴进来");
        assert_eq!(parse_yuan(" ¥88.50 "), Some(885_000), "带符号带空格");
        assert_eq!(parse_yuan(".5"), Some(5_000));

        assert_eq!(parse_yuan(""), None);
        assert_eq!(parse_yuan("  "), None);
        assert_eq!(parse_yuan("abc"), None);
        assert_eq!(parse_yuan("-5"), None, "负数不是「记一笔」");
        assert_eq!(parse_yuan("12.34567"), None, "毫之下没有意义，当输入错误");
        assert_eq!(parse_yuan("1.2.3"), None);
        assert_eq!(parse_yuan("9".repeat(30).as_str()), None, "溢出返回 None，不 panic");
    }

    /// 只算当月、只认形状对的日期。
    #[test]
    fn extras_only_count_in_their_own_month() {
        let mut s = Settings::default();
        s.extras = vec![
            ExtraEntry { date: "2026-11-01".into(), amount_milli: 1_000_000 },
            ExtraEntry { date: "2026-11-30".into(), amount_milli: 2_000_000 },
            ExtraEntry { date: "2026-12-01".into(), amount_milli: 9_000_000 },
            ExtraEntry { date: "2025-11-15".into(), amount_milli: 5_000_000 },
        ];
        assert_eq!(s.extras_in_month("2026-11"), 3_000_000);
        assert_eq!(s.extras_in_month("2026-12"), 9_000_000);
        assert_eq!(s.extras_in_month("2026-10"), 0);
    }

    /// 文件被手改坏时，坏的那几笔单独丢掉，好的照旧。
    #[test]
    fn broken_extra_entries_are_dropped_one_by_one() {
        let d = tmpdir("extras");
        fs::write(
            settings_path(&d),
            r#"{"extras": [
                {"date": "2026-11-10", "amount_milli": 3000000},
                {"date": "2026-13-45", "amount_milli": 1000000},
                {"date": "2026-11-10", "amount_milli": -5000},
                {"date": "2026-11-10", "amount_milli": 0},
                {"date": "2026/11/10", "amount_milli": 1000000},
                {"date": "2026-11-11", "amount_milli": 2000000}
            ]}"#,
        )
        .unwrap();
        let s = load_from(&d);
        assert_eq!(s.extras.len(), 2, "只剩两条合法的");
        assert_eq!(s.extras[0].amount_milli, 3_000_000);
        assert_eq!(s.extras[1].date, "2026-11-11");
    }

    /// 账本封顶 500 笔，多的按日期丢最早的。
    #[test]
    fn extras_are_capped_keeping_the_newest() {
        let mut s = Settings::default();
        for i in 0..620u32 {
            // 造 620 天，日期各不相同且可排序
            let m = 1 + i / 28;
            let d = 1 + i % 28;
            s.extras.push(ExtraEntry {
                date: format!("20{:02}-{:02}-{:02}", 20 + m / 12, (m % 12) + 1, d),
                amount_milli: 1_000,
            });
        }
        s.validate();
        assert_eq!(s.extras.len(), 500);
        let last = s.extras.last().unwrap().date.clone();
        assert!(s.extras[0].date < last, "留下的是靠后的那一段");
    }
}
