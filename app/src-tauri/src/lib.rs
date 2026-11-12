//! 牛马薪动了！ —— 主程序（外壳层）
//!
//! 这一层**只做四件事**：开窗口、读时钟、把时钟换算成「已计薪秒」、把结果画出来。
//! 任何金额都来自 `paypulse_core`，这里不做除法、不做四舍五入、不持有费率常量。
//!
//! 分界线的意义：外壳是开源的（AGPL-3.0），引擎是私有仓。
//! 所以「21.75 从哪来」这个问题，在外壳里翻不到答案。
//!
//! ## 状态从哪来
//!
//! 界面状态是**两个输入的函数**：时钟给出的时段（上班前 / 在岗 / 午休 / 休息日），
//! 加上用户声明（有没有点「我在加班」）。所以 `tick` 不接受前端传来的状态——
//! 前端只负责画，不负责判断「现在算什么状态」。判断错了，金额和界面就会各说各话。

use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone, Timelike};
use paypulse_clock::{
    is_paused, lunch_in_at, lunch_remaining_at, paid_seconds_at, paused_seconds_at, phase_at,
    prune_pauses, Calendar, Phase,
};
use paypulse_core::{
    calc_of, shadow_seconds, shadow_sum, Params, ShadowSpan, State as EngineState,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

mod log;
mod platform;
mod ask_win;
mod detail;
mod menu;
mod onboard_win;
mod settings;
mod settings_win;
mod toast;
mod tray;
mod update;

use settings::{LunchOtRec, OtMode, PauseRec, Settings, Size, WinPos};

/// 悬浮窗的窗口标签。窗口位置记忆只认它——设置窗口移动不该改掉悬浮窗的位置。
pub(crate) const HUD_LABEL: &str = "hud";

/// 窗口位置写盘的最小间隔。
///
/// 拖动窗口会以每秒几十次的频率触发 `Moved`。每次都写盘等于拿磁盘当草稿纸，
/// 而完全不写又会在 `taskkill` / 崩溃时丢掉位置。1 秒一次是这两者之间的折中：
/// 最坏丢 1 秒内的位移，感受不出来。
const POS_SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// 全局状态：配置 + 暂停区间 + 写盘节流用的时间戳。
pub struct AppState {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// 这次启动算不算「第一次」（启动时 `settings.json` 还不存在）。
    ///
    /// 为什么用「文件在不在」判，而不是在配置里存一个 `first_run: bool`：
    /// 存了字段之后，所有老用户的配置文件里都没有它，`serde` 的默认值
    /// 会把每个人都当成新装的——那才是真正的回归。
    fresh: std::sync::atomic::AtomicBool,
}

struct Inner {
    settings: Settings,
    last_save: Instant,
    watch: Watch,
}

/// 空闲检测这一帧该干什么。
///
/// 单独抽出来是因为它有一个**必须靠测试守住的顺序**：判「回来没有」时不能再看
/// `paused`。之前写成「没暂停才检测，暂停了就把标记清掉」，而检测自己开的那段
/// 暂停会让下一帧变成「暂停中」，于是标记被清、暂停却留着——人回来了钱不涨，
/// 界面上还写着「在岗」。这类错不会报错、不会崩，只会让用户以为软件坏了。
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum IdleStep {
    /// 人走了：从「最后一次键鼠」起开一段暂停
    Pause,
    /// 人回来了：把暂停收口
    Resume,
    /// 不动
    Nothing,
}

fn idle_step(enabled: bool, open: bool, idle_sec: u64, threshold_sec: u64, paused: bool) -> IdleStep {
    if !enabled {
        return IdleStep::Nothing;
    }
    if open {
        // 只看键鼠：`paused` 在这条路上是我们自己造出来的，不能当门闩
        return if idle_sec < 5 { IdleStep::Resume } else { IdleStep::Nothing };
    }
    if paused {
        // 用户自己按了暂停，这段归他，我们不掺和
        return IdleStep::Nothing;
    }
    if idle_sec >= threshold_sec {
        IdleStep::Pause
    } else {
        IdleStep::Nothing
    }
}

/// 收掉上一进程留在配置里的「没收口的暂停」，返回收了几段。
///
/// 为什么非收不可：整条恢复链路里，能主动结束一段暂停的只有 `Act::Resume`，
/// 而它挂在 `idle_open` 这个**内存**标记上。进程被强杀（关黑窗口、任务管理器、
/// 旧版本那个空闲 bug）之后，`pauses` 里会留一段 `end: None`；新进程读进来是
/// `idle_open = false`、`paused = true`，`idle_step` 只会一直返回 `Nothing`——
/// 金额从那一刻起永不增长，而界面上状态还写着「在岗」。用户没有别的词能描述
/// 这件事，只能说「在岗，但今日已赚没在计算」。
///
/// 收口方式选「整段抹掉」而不是「补到 now」：这个程序本来就按时钟算钱，
/// 程序没跑的那段时间照样计入；补到 now 等于凭空把一大段扣掉，比留着更糟。
/// 抹掉最坏只是把「上次没跑那几小时」算成在岗，跟已有的口径一致。
fn drop_dangling_pauses(cfg: &mut Settings) -> usize {
    let before = cfg.pauses.len();
    cfg.pauses.retain(|p| p.end.is_some());
    before - cfg.pauses.len()
}

/// 这一帧该不该弹「今日已到账」。
///
/// 两条取或，缺一条都不行：
/// ① `done`：日工时攒满了（提前干完的那种）；
/// ② 班次走到终点：用户把下班时间改成十分钟后、日工时却还是七个半小时的时候，
///    ① 永远不成立，通知就永远不弹——他眼里「我到点了」和「我攒满了」是两件事，
///    只认得前者。这条是用户拿真机报的错。
///
/// ②还要配一个「今天真干过活」：夜班的班次外空档（06:00 下班、22:00 上班，
/// 中间那十几个小时）在 `past_end` 眼里也是「不在班里」，不拦一下的话，
/// 早上九点打开程序会收到一条「今日已到账 ¥0.00」。
fn end_notice_due(state: &str, shift_over: bool, worked_sec: u32, fired: bool) -> bool {
    if fired {
        return false;
    }
    state == "done" || (shift_over && worked_sec > 0)
}

/// 每帧 tick 顺手要看的那点状态。**不落盘**——重启之后
/// 「今天已经提醒过下班了」本来就该重来一次。
#[derive(Default)]
struct Watch {
    /// 上一帧的状态，用来做边沿检测（on → done 才弹通知，不是每帧都弹）
    prev_state: String,
    /// 上一帧看到的日期，跨天清零
    day: String,
    end_fired: bool,
    lunch_fired: bool,
    /// 上次久坐提醒的时刻
    sit_at: i64,
    /// 当前这段暂停是「空闲检测」开的（不是用户点的）
    idle_open: bool,
    /// 窗口是否已经露过脸（静默启动要跳过第一次）
    shown_once: bool,
    /// 上次记「空闲多少秒」时的读数，只为不刷屏
    idle_logged: i64,
    /// 是否已经看过至少一帧。第一帧只用来「认识现状」，不弹任何通知——
    /// 晚上八点才开程序的人，不该一开机就被告诉「今日已到账」。
    seeded: bool,
}

impl AppState {
    fn new(dir: PathBuf, settings: Settings, fresh: bool) -> Self {
        Self {
            dir,
            inner: Mutex::new(Inner {
                settings,
                last_save: Instant::now(),
                watch: Watch::default(),
            }),
            fresh: std::sync::atomic::AtomicBool::new(fresh),
        }
    }

    /// 还是不是「第一次启动」。设置页据此决定要不要显示引导条。
    fn is_fresh(&self) -> bool {
        self.fresh.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 把「第一次」这个状态消费掉。返回 true 表示「刚刚从新变成不新」——
    /// 调用方靠它来判断「这一刻该让悬浮窗露脸了」，而不是每次写盘都露。
    fn take_fresh(&self) -> bool {
        self.fresh.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    /// 锁中毒不该让悬浮窗跟着崩：金额算不出来大不了显示「—」，
    /// 而程序退出用户就什么都看不见了。
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// 改配置并立即写盘。用于托盘菜单、加班声明这类低频操作。
    fn mutate<F: FnOnce(&mut Settings)>(&self, f: F) -> Settings {
        let mut g = self.lock();
        let mut next = g.settings.clone();
        f(&mut next);
        next.validate();
        g.settings = next.clone();
        g.last_save = Instant::now();
        drop(g);
        if let Err(e) = settings::save_to(&self.dir, &next) {
            // 存不下不该影响使用：这次会话照样按新配置算，下次启动退回旧值
            eprintln!("保存配置失败：{e}");
        }
        next
    }

    /// 改配置但节流写盘。位置这种高频变化走这里。
    fn mutate_throttled<F: FnOnce(&mut Settings)>(&self, f: F) {
        let mut g = self.lock();
        let mut next = g.settings.clone();
        f(&mut next);
        next.validate();
        g.settings = next.clone();

        if g.last_save.elapsed() < POS_SAVE_INTERVAL {
            return; // 内存里已经是新值，磁盘等下一次
        }
        g.last_save = Instant::now();
        drop(g);
        if let Err(e) = settings::save_to(&self.dir, &next) {
            eprintln!("保存配置失败：{e}");
        }
    }

    /// 切换暂停。返回切换后「现在是否暂停中」。
    ///
    /// 暂停只往 `pauses` 里追加/收尾一段区间，**不碰任何计数器**：
    /// 扣减量永远由「区间 ∩ 今天」算出来。所以暂停期间合盖 8 小时，
    /// 醒来扣的就是真实的 8 小时，而不是「多跑了 8 万次 tick」。
    fn toggle_pause(&self, now_ts: i64) -> bool {
        let next = self.mutate(|s| {
            if is_paused(&s.spans()) {
                if let Some(last) = s.pauses.last_mut() {
                    if last.end.is_none() {
                        last.end = Some(now_ts);
                    }
                }
            } else {
                s.pauses.push(PauseRec {
                    start: now_ts,
                    end: None,
                });
            }
        });
        is_paused(&next.spans())
    }

    fn set_click_through(&self, on: bool) -> Settings {
        self.mutate(|s| s.click_through = on)
    }

    fn set_hide_money(&self, on: bool) -> Settings {
        self.mutate(|s| s.hide_money = on)
    }

    fn set_ot_mode(&self, paid: bool) -> Settings {
        self.mutate(|s| {
            s.ot_mode = if paid { OtMode::Paid } else { OtMode::Shadow };
        })
    }

    /// 切到相邻的尺寸档位，返回切完之后的档位。
    fn cycle_size(&self, dir: i32) -> Size {
        self.mutate(|s| s.size = s.size.cycle(dir)).size
    }

    /// 今天请假 / 取消请假。返回切换后「今天是不是请假」。
    ///
    /// 存的是**日期**而不是一个布尔：布尔到第二天就失效，用户会看到
    /// 「昨天请假」把今天也变成 0；而日期能撑起「本月 -N 天」和月合计的扣减。
    fn toggle_leave_today(&self, today: chrono::NaiveDate) -> bool {
        let key = today.to_string();
        let next = self.mutate(|s| {
            if let Some(pos) = s.leave_days.iter().position(|d| *d == key) {
                s.leave_days.remove(pos);
            } else {
                s.leave_days.push(key);
            }
        });
        next.is_leave_day(today)
    }

    /// 声明 / 结束加班。窗口上那个按钮和托盘共用这一个入口。
    ///
    /// 午休窗口内声明的是「午休加班」：封顶到 13:30，按 1× 记白干，
    /// 而且不受「加班怎么算」影响——午休本来就没有加班费可言。
    fn set_ot(&self, start: bool, now: DateTime<FixedOffset>, cal: &Calendar) {
        let ts = now.timestamp();

        if !start {
            // 结束加班：两个都收掉。
            // 午休那段是「收口」而不是「删除」——当天已经白干的时间要留着，
            // 这是原型稿明写的规矩（结束加班后白干条不消失）。
            // 午休还没到 13:30 的话，状态会自然退回「午休中」：
            // 时段是时钟算的，不由这里决定。
            self.mutate(|s| {
                s.ot_start = None;
                s.close_lunch_ot(ts);
            });
            return;
        }

        if phase_at(now, cal) == Phase::Lunch {
            let cap = lunch_remaining_at(now, cal);
            self.mutate(|s| {
                s.close_lunch_ot(ts); // 理论上不会有，手改文件才会出现两段都开着
                s.lunch_ots.push(LunchOtRec {
                    start: ts,
                    end: None,
                    cap_sec: cap,
                });
            });
        } else {
            self.mutate(|s| s.ot_start = Some(ts));
        }
    }

    /// 记下窗口位置。
    ///
    /// 两份都写：`win` 是兜底（拿不到显示器名时还有得用），
    /// `win_by_monitor` 是「这块屏上次在这儿」。
    /// 存的仍是**逻辑**像素，除的是这块屏自己的 scale——
    /// 换到另一块缩放不同的屏时，各屏各记一份才不会互相带错。
    fn remember_pos(&self, x: f64, y: f64, scale: f64, monitor: Option<String>) {
        let sx = if scale > 0.0 { scale } else { 1.0 };
        let lx = (x / sx).round() as i32;
        let ly = (y / sx).round() as i32;
        self.mutate_throttled(move |s| {
            let p = WinPos { x: lx, y: ly };
            s.win = Some(p);
            if let Some(name) = monitor {
                s.win_by_monitor.insert(name, p);
            }
        });
    }

    /// 退出前抢救一次：节流可能导致最后一次位移没落盘。
    fn flush(&self) {
        let s = self.settings();
        if let Err(e) = settings::save_to(&self.dir, &s) {
            eprintln!("退出前保存配置失败：{e}");
        }
    }
}

/// 配置 → 引擎参数。
///
/// 两处刻意的不对称：
/// - 白干倍率**恒等于**加班倍率：同一段时间，进主账还是只记白干，
///   是记账方式不同，不是费率不同。
/// - 午休加班恒按 1×：午休本来就没有加班费，现实里也不会有，所以不给它配项。
pub(crate) fn params(s: &Settings) -> Params {
    Params {
        month_milli: s.month_milli,
        pay_days_centi: s.pay_days_centi,
        daily_sec: s.daily_sec,
        ot_rate_x100: s.ot_rate_x100,
        shadow_rate_x100: s.ot_rate_x100,
        lunch_shadow_rate_x100: 100,
        // 年终奖摊进来的那部分交给引擎加：它出现在四个折算式的分子里，
        // 在外面加就要加四次，迟早漏一处（然后日薪和时薪互相矛盾）。
        bonus_milli: s.bonus.per_month_milli(s.month_milli),
    }
}

/// 界面状态。字符串与原型稿的 `data-state` 一一对应。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UiState {
    /// 在岗 / 上班前（原型稿里上班前也归在岗，金额 0）
    On,
    /// 午休中
    Break,
    /// 计费加班
    Ot,
    /// 义务加班（含午休加班）
    Unpaid,
    /// 已下班
    Done,
    /// 请假（不计薪，紫色）
    Leave,
    /// 休息日
    Rest,
}

impl UiState {
    fn as_str(self) -> &'static str {
        match self {
            UiState::On => "on",
            UiState::Break => "break",
            UiState::Ot => "ot",
            UiState::Unpaid => "unpaid",
            UiState::Done => "done",
            UiState::Leave => "leave",
            UiState::Rest => "rest",
        }
    }
}

/// 一次状态判定的结果。
struct Resolved {
    ui: UiState,
    /// 交给引擎的记账状态
    engine: EngineState,
    /// 白干来自哪一段：`break` 午休（1×）/ `after` 下班后（1.5×）/ 空
    src: &'static str,
}

/// 状态判定。**这是整个外壳里最该被测透的一段**：它决定钱进哪本账。
///
/// 输入全是显式的，不读全局时钟——于是「19:00 点了我在加班、但设置是不计加班费」
/// 这种组合能在测试里直接摆出来。
fn resolve(
    phase: Phase,
    paid_sec: u32,
    s: &Settings,
    declared_ot: bool,
    on_lunch_ot: bool,
    on_leave: bool,
) -> Resolved {
    let daily = s.daily_sec;
    let over = paid_sec >= daily;

    // 请假压在休息日之前判：用户明确标了「这天我请假」，就该看到「请假」，
    // 而不是被「今天休息」的遮罩盖住、以为标记没生效。
    if on_leave {
        return Resolved {
            ui: UiState::Leave,
            engine: EngineState::On,
            src: "",
        };
    }

    if phase == Phase::Rest {
        return Resolved {
            ui: UiState::Rest,
            engine: EngineState::Off,
            src: "",
        };
    }

    // 午休加班优先于一切：午休期间声明了加班，就只记白干，不推主账。
    if on_lunch_ot {
        return Resolved {
            ui: UiState::Unpaid,
            engine: EngineState::LunchOt,
            src: "break",
        };
    }

    if declared_ot && over {
        return match s.ot_mode {
            OtMode::Paid => Resolved {
                ui: UiState::Ot,
                engine: EngineState::Ot,
                src: "",
            },
            OtMode::Shadow => Resolved {
                ui: UiState::Unpaid,
                engine: EngineState::Unpaid,
                src: "after",
            },
        };
    }

    if over {
        return Resolved {
            ui: UiState::Done,
            engine: EngineState::On,
            src: "",
        };
    }

    if phase == Phase::Lunch {
        return Resolved {
            ui: UiState::Break,
            engine: EngineState::Break,
            src: "",
        };
    }

    Resolved {
        ui: UiState::On,
        engine: EngineState::On,
        src: "",
    }
}

/// 一次渲染需要的全部数值。
///
/// 金额一律用「毫」（1 元 = 1000 毫），时间一律用秒。前端的除法只发生在
/// `toFixed` 那一行——这样「金额」这件事在整条链路上只有一个表示法。
#[derive(Serialize)]
struct Snapshot {
    /// 计入月薪（毫）
    amount_milli: i64,
    /// 白干价值（毫）——**永不加进 amount**
    shadow_milli: i64,
    /// 白干累计秒数（右下角「已白干」用它，不能用超时秒数顶替）
    shadow_sec: u32,
    /// 白干来源：break（午休 1×）/ after（下班后 1.5×）
    shadow_src: String,

    /// 已工作秒数（= 已计薪秒，不是墙上时钟）
    worked_sec: u32,
    /// 超出每日工时的秒数。右下角「已加班 / 已过」用它，
    /// 前端不许自己拿 worked_sec 减工时——那是业务数值，不该出现在外壳。
    ot_sec: u32,
    /// 距下班的秒数
    remain_sec: u32,
    /// 午休还剩多少秒（只在午休中有意义）
    lunch_remain_sec: u32,
    /// 距午休开始还有多少秒（提醒用，不在上班时段一律 0）
    lunch_in_sec: u32,
    /// 进度 0.0–1.0
    progress: f64,

    /// 日薪 / 时薪（毫）
    day_milli: i64,
    hourly_milli: i64,
    /// 每秒进账（微元）。毫的精度不够显示四位小数。
    per_sec_micro: i64,

    /// 本月已赚（毫）= 已过完的工作日 × 日薪 + 今日已赚
    month_earn_milli: i64,
    /// 本月还剩几个工作日（不含今天）
    month_left_days: u32,

    /// 状态：on / break / ot / unpaid / done / rest
    state: String,

    /// 今天休息（周六/周日）→ 一律不给数字
    rest: bool,
    /// 现在处于暂停中（托盘切的）
    paused: bool,
    /// 今天累计暂停了多少秒
    pause_sec: u32,
    /// 托盘开的「隐藏金额」：数字要糊掉，但进度条照旧
    hide_money: bool,
    /// 鼠标穿透。放快照里是为了让托盘切换能反映到窗口上那个 ◎ 按钮
    click_through: bool,

    /// 尺寸档位：compact / normal / expanded
    size: String,

    /// 强调色（#RRGGBB）。只影响中性状态的色相，语义色不跟着变。
    accent: String,
    /// 强调色的三元组（"53, 224, 127"）：CSS 的 rgba() 要这个形状。
    /// 在 Rust 拆好，前端就不用做 hex → rgb 的转换。
    accent_rgb: String,
    /// 底色不透明度（50–100）
    opacity: u8,
    /// 主题：dark / black / light / system
    theme: String,
    /// 数字字体：sf / jb / sys
    num_font: String,
    /// 文案风格：emotional / plain / silent
    copy_style: String,
    /// 信息密度：四块内容的显隐
    density_metrics: bool,
    density_bar: bool,
    density_tip: bool,
    density_shadow: bool,

    /// 今天请假
    leave: bool,
    /// 请假类型（年假 / 事假 / 病假）
    leave_kind: String,
    /// 剩余额度，天 × 100
    leave_left_centi: u32,
    /// 本月请假天数（只数工作日，含今天）
    leave_month_days: u32,

    // ---- 展开态才用到的等价物换算（原型 01-C「展开 322×330」） ----
    //
    // 这几个数也是除法算出来的，所以必须在这儿算完：外壳 JS 里一行算术都没有，
    // 这是整个项目从头到尾的一条规矩，不因为「只是个装饰性换算」就破例。
    /// 等价物名字，例如「奶茶」
    eq_name: String,
    /// 今日已赚折合多少杯 × 10（11.6 杯 → 116）
    eq_cups_x10: i64,
    /// 距下班还能买多少杯 × 10
    eq_left_cups_x10: i64,
    /// 累计目标杯数（第一行右槽）
    eq_goal_cups: u32,
    /// 今日目标杯数（第二行右槽）
    eq_day_cups: u32,
    /// 第一条小进度条的比例 0.0–1.0（今日杯数 / 累计目标）
    eq_goal_pct: f64,
    /// 第二条小进度条的比例 0.0–1.0（今日杯数 / 今日目标）
    eq_day_pct: f64,
}

/// `#RRGGBB` → `"53, 224, 127"`。
///
/// 这个转换放这里而不是前端，是因为 `rgba()` 只认三元组，而前端一旦开始
/// `parseInt(hex.slice(1,3), 16)` 就成了「前端也在处理颜色」的第一步。
/// 认不出来的形状退回品牌绿——`validate` 已经挡过一次，这里是第二道。
fn hex_rgb(s: &str) -> String {
    if s.len() == 7 && s.is_ascii() && s.starts_with('#') {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&s[1..3], 16),
            u8::from_str_radix(&s[3..5], 16),
            u8::from_str_radix(&s[5..7], 16),
        ) {
            return format!("{r}, {g}, {b}");
        }
    }
    "53, 224, 127".to_string()
}

/// 毫 → `¥8,000.00`。
///
/// 这条格式化放在 Rust：通知文案里的金额也要四舍五入，
/// 而外壳 JS 里连这一处除法都不许有（见 README 那条规矩）。
/// `decimals` 只支持 2（元角分）和 3（毫，与悬浮窗主数字一致）。
pub(crate) fn yuan(milli: i64, decimals: usize) -> String {
    let neg = milli < 0;
    let m = milli.abs();
    let whole = m / 1000;
    let frac = match decimals {
        0 => 0,
        2 => m % 1000 / 10,
        _ => m % 1000,
    };
    // 千分位：从右往左每三位一个逗号
    let ws = whole.to_string();
    let mut out = String::with_capacity(ws.len() + 6);
    for (i, ch) in ws.chars().enumerate() {
        if i > 0 && (ws.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if decimals == 2 {
        out.push_str(&format!(".{frac:02}"));
    } else if decimals == 3 {
        out.push_str(&format!(".{frac:03}"));
    }
    format!("{}{}", if neg { "-¥" } else { "¥" }, out)
}

/// 秒 → `07:30`（超过一小时就是 `8 小时 5 分` 这种，通知里更好读）。
fn hm_text(sec: u32) -> String {
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

/// 托盘悬停提示。规格 02 的状态表：Win 托盘悬停显示今日已赚，mac 菜单栏直接显示。
///
/// 为什么要显示钱：这是这个产品唯一的存在理由，而用户把悬浮窗关掉之后，
/// 剩在桌面上的就只有托盘图标。图标只能证明「程序还在」，说不出「今天赚了多少」——
/// 悬停一句 `今日已赚 ¥123.45`，就把托盘从「还活着」升级成了「还能看」。
///
/// 但它**不能绕过隐藏金额**：那个开关是给开会投屏用的，托盘从背后把数字漏出来，
/// 等于开关是假的。请假 / 休息 / 暂停同理，那几种状态下金额本身没有意义。
pub(crate) fn tray_hint(s: &Snapshot) -> String {
    const BRAND: &str = "牛马薪动了！";
    if s.paused {
        return format!("{BRAND} 已暂停");
    }
    if s.leave {
        return format!("{BRAND} 今天请假");
    }
    if s.rest {
        return format!("{BRAND} 今天休息");
    }
    if s.hide_money {
        return BRAND.to_string();
    }
    format!("{BRAND} 今日已赚 {}", yuan(s.amount_milli, 2))
}

/// mac 菜单栏上那串字。
///
/// 菜单栏宽度是抢来的（旁边还有输入法、电池、时间），所以只留金额、不留品牌词：
/// `¥367.82` 一眼能读完，`牛马薪动了！¥367.82` 会把别人的图标挤走。
/// 休息 / 请假 / 隐藏金额时退成一个 `¥` —— 位置不能空着，否则菜单栏会跳。
///
/// Windows 上没人调它（那边的托盘不显示文字），但单测要能跑，所以不整个 cfg 掉。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn tray_title(s: &Snapshot) -> String {
    if s.rest || s.leave || s.hide_money {
        return "¥".to_string();
    }
    yuan(s.amount_milli, 2)
}

/// 北京时间（UTC+8）的固定偏移。`east_opt` 对 8 小时恒为 `Some`。
///
/// 「固定 UTC+8」这一档是给出差的人用的：公司按北京的钟发工资，
/// 人在东京也该「北京 9 点上班」。它不是唯一一档——默认仍然是跟随系统时区，
/// 只有真的跨了 2 小时以上才会被问一次（见 `tz_check`）。
const EAST8_SEC: i32 = 8 * 3600;

/// 按配置取「墙上时钟」。
///
/// 这一个函数是时区策略的唯一接缝：`Settings::tz_fixed` 为真时，无论系统时区
/// 被改成了什么（出差、误点、装的系统是英文版），作息表都按 UTC+8 解释。
/// 返回 `DateTime<FixedOffset>` 而不是 `DateTime<Local>`，是为了让两条分支
/// **是同一个类型**——否则调用方要写两份代码，迟早有一份忘了改。
///
/// 注意它只影响「现在几点」，不影响时间戳：`timestamp()` 在任何时区下都是同一个数，
/// 所以已经落盘的暂停区间、加班起始点不会因为切时区而重算。
pub(crate) fn clock(s: &Settings) -> DateTime<FixedOffset> {
    let now = Local::now().fixed_offset();
    if s.tz_fixed {
        let east8 = FixedOffset::east_opt(EAST8_SEC).unwrap_or_else(|| *now.offset());
        now.with_timezone(&east8)
    } else {
        now
    }
}

/// 今天零点（Unix 秒）。用来判断落盘的那些时间戳是不是还是「今天」的。
pub(crate) fn day_start(now: DateTime<FixedOffset>) -> i64 {
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| now.timezone().from_local_datetime(&d).earliest())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

/// 给定一个时刻，算出前端要的全部东西。
///
/// 这是**唯一**的取值入口：`tick` 只剩「取 now + 调它」。
/// 拆出来的理由和之前一样——`Local::now()` 在测试里没法摆布，
/// 而「晚上七点半、设置是不计加班费、午休加班过」这种组合必须能被断言。
pub(crate) fn snapshot_at(now: DateTime<FixedOffset>, s: &Settings) -> Snapshot {
    let p = params(s);
    // 作息从配置里来，不再是一个常量。
    //
    // 早先这里是 `Schedule::default()`，于是设置里改上班时间对界面无效——
    // 「改了没反应」比「没有这个设置」更让人不信任。现在只有一个来源。
    let cal = s.calendar_shifted(s.flex_delta(now));

    // 弹性排班：今天还没动过键鼠 = 这一天还没开始，一分钱都不该记。
    // 这里必须显式判一次：`calendar_shifted(0)` 会退回配置里的 09:00，
    // 而 10:30 才坐下的人就会看到「09:00 到现在已经赚了 80 块」。
    let waiting = s.week.flex && s.today_anchor(now).is_none();

    // 墙上时钟 → 已计薪秒。休息日这里会返回 (0, true)。
    let (wall_sec, rest) = if waiting {
        (0, false)
    } else {
        paid_seconds_at(now, &cal)
    };
    let pause_sec = paused_seconds_at(now, &s.spans());

    let today = now.date_naive();
    let on_leave = s.is_leave_day(today);

    // 请假当天一分钱都不计：直接把已计薪秒归零，而不是「算完再抹掉」。
    // 这样下游所有派生值（月合计、等价物、进度）自然全对，不用各自再判一次。
    let paid_sec = if on_leave {
        0
    } else {
        wall_sec.saturating_sub(pause_sec)
    };
    // 请假压在休息日前：请假是可撤销的用户声明，休息日是客观事实，
    // 但用户既然标了请假，就该看到「请假」——否则会以为标记没生效。
    let rest = rest && !on_leave;

    let phase = phase_at(now, &cal);

    // 声明是否仍然有效：跨天了就作废。不判这一天，昨晚点的「我在加班」
    // 会让今天 18:00 一到就自动变成加班状态。
    let lo = day_start(now);
    let declared_ot = s.ot_start.map_or(false, |t| t >= lo);
    // 午休加班只在午休窗口内有效。13:30 一过就自动收口——
    // 这是原型稿里「到点自动切回在岗」那条。
    let on_lunch_ot = s.lunch_ot_open() && phase == Phase::Lunch;

    let r = resolve(phase, paid_sec, s, declared_ot, on_lunch_ot, on_leave);
    let out = calc_of(paid_sec, r.engine, &p);

    // 午休白干：只能来自区间。午休期间已计薪秒不动，推不出「加班了多久」。
    //
    // 收口的段用它的固定长度当封顶（`len` 之后不再随 now 增长），
    // 没收口的那段才用整段窗口封顶。这样「12:00–12:30 记了一段、
    // 结束之后又开了新的一段」也能正确相加。
    let now_u = now.timestamp().max(0) as u64;
    let now_ts = now.timestamp();
    let lunch_spans: Vec<ShadowSpan> = s
        .lunch_ots
        .iter()
        .map(|l| ShadowSpan {
            start: l.start.max(0) as u64,
            rate_x100: p.lunch_shadow_rate_x100,
            cap_sec: if l.end.is_some() { l.len(now_ts) } else { l.cap_sec },
        })
        .collect();
    // 不看状态、只看记录：切回在岗之后，上午记下的白干不能凭空消失。
    let lunch_shadow_milli = shadow_sum(&lunch_spans, now_u, &p);
    let lunch_shadow_sec = shadow_seconds(&lunch_spans, now_u);

    // 下班后的白干由引擎按「秒数 − 日工时」算（out.shadow 只在 Unpaid 时非 0），
    // 午休那段由区间算，两者时间上不重叠，可以直接相加。
    let shadow_milli = out.shadow + lunch_shadow_milli;
    let shadow_sec = (if r.src == "after" { out.ot_sec } else { 0 }) + lunch_shadow_sec;

    let base = paid_sec.min(p.daily_sec);
    let done_before = cal.workdays_before(today) as i64;
    let day_milli = p.day_milli();
    let total_days = cal.workdays_in_month(today.year(), today.month());

    // 请假要从月合计里扣掉，否则「本月已赚」会凭空多出几天工资。
    // 只数工作日：手滑标了周六不该扣两次（那天本来就不计薪）。
    let (leave_month_days, leave_before, leave_after) = s.leave_stats(today, &cal);
    let earned_days = (done_before - leave_before as i64).max(0);
    let left_days = total_days
        .saturating_sub(done_before as u32)
        .saturating_sub(1)
        // 已经记在册的未来请假天数，不算「还剩几个工作日」——
        // 那天不会上班，列在「还剩」里就是虚的。
        .saturating_sub(leave_after);

    // 展开态的等价物换算。单价存的是「分」，换成毫（1 分 = 10 毫）。
    // `validate()` 已经把 0 夹走了，这里再兜一次底——这个值来自可以手改的文件，
    // 而除以 0 是 panic，会把整个悬浮窗带走。
    let price_milli = (s.eq_price_centi as i64).saturating_mul(10).max(1);
    // 折合杯数保留一位小数，所以先放大 10 倍再四舍五入。
    // 直接整数除法会把 11.6 截成 11.5，那种「少半杯」的误差在钱上很扎眼。
    let cups_x10 = |milli: i64| -> i64 {
        (milli.max(0) * 10 + price_milli / 2) / price_milli
    };
    let eq_cups_x10 = cups_x10(out.paid);
    // 「距下班还能买」＝ 今天还没赚到的那部分。加班 / 已下班时它是 0，
    // 因为日薪已经封顶了——那时候显示 0.0 杯才是实话。
    let remain_milli = (day_milli - out.paid).max(0);
    let eq_goal_cups = s.eq_goal_cups.max(1);
    let eq_day_cups = s.eq_day_cups.max(1);

    Snapshot {
        amount_milli: out.paid,
        shadow_milli,
        shadow_sec,
        shadow_src: r.src.to_string(),

        // 「已工作」封顶在日工时：到点/满勤之后它就该冻结在 daily_sec，
        // 多出来的那部分归 `ot_sec`（右下角「已过 / 已加班」），不能再往里涨——
        // 否则下班后「已工作的累计时间」会一路叠加，看起来像软件算错。
        worked_sec: base,
        ot_sec: out.ot_sec,
        remain_sec: p.daily_sec.saturating_sub(base),
        lunch_remain_sec: lunch_remaining_at(now, &cal),
        lunch_in_sec: lunch_in_at(now, &cal),
        progress: base as f64 / p.daily_sec as f64,

        day_milli,
        hourly_milli: p.hourly_milli(),
        per_sec_micro: p.per_sec_micro(),

        // 额外进账只加在这一行：日薪、时薪、等价物都不认它。
        // 这是「记一笔」能立刻被看见的唯一出口——展开档那句「本月已到账」。
        month_earn_milli: day_milli * earned_days
            + out.paid
            + s.extras_in_month(&today.format("%Y-%m").to_string()),
        month_left_days: left_days,

        state: r.ui.as_str().to_string(),

        rest,
        paused: is_paused(&s.spans()),
        pause_sec,
        hide_money: s.hide_money,
        click_through: s.click_through,

        size: s.size.as_str().to_string(),
        accent: s.accent.clone(),
        accent_rgb: hex_rgb(&s.accent),
        opacity: s.opacity,
        theme: s.theme.as_str().to_string(),
        num_font: s.num_font.as_str().to_string(),
        copy_style: s.copy_style.as_str().to_string(),
        density_metrics: s.density.metrics,
        density_bar: s.density.bar,
        density_tip: s.density.tip,
        density_shadow: s.density.shadow,

        leave: on_leave,
        leave_kind: s.leave_kind.clone(),
        leave_left_centi: s.leave_left_centi,
        leave_month_days,

        eq_name: s.eq_name.clone(),
        eq_cups_x10,
        eq_left_cups_x10: cups_x10(remain_milli),
        eq_goal_cups,
        eq_day_cups,
        eq_goal_pct: (eq_cups_x10 as f64 / (eq_goal_cups as f64 * 10.0)).min(1.0),
        eq_day_pct: (eq_cups_x10 as f64 / (eq_day_cups as f64 * 10.0)).min(1.0),
    }
}


/// 一帧的副作用：空闲检测 + 三种通知。
///
/// 为什么放在 `tick` 里而不是单独开一个线程：tick 已经在算「现在是什么状态」，
/// 而通知要的正是**边沿**（on → done 才弹一次）。另开线程就得把状态复制一份，
/// 两份状态迟早不同步——那正是「金额和界面各说各话」的来源。
///
/// 副作用收集成列表、出锁之后再执行：`mutate` 自己也要拿这把锁，
/// 在锁里调它就是死锁。
fn observe(app: &tauri::AppHandle, st: &AppState, snap: &Snapshot, now: DateTime<FixedOffset>) {
    enum Act {
        Pause(i64),
        Resume(i64),
        Anchor(i64),
        Toast(String, String, &'static str),
    }
    let mut acts: Vec<Act> = Vec::new();
    let ts = now.timestamp();
    let day = now.date_naive().to_string();
    let min_of_day = now.hour() * 60 + now.minute();

    {
        let mut g = st.lock();
        let s = g.settings.clone();
        let w = &mut g.watch;

        // 跨天：所有「今天已经提醒过」的标记归零
        if w.day != day {
            w.day = day;
            w.end_fired = false;
            w.lunch_fired = false;
            w.sit_at = 0;
        }

        // ---- 空闲检测 ----
        //
        // 起点用 `ts − idle`（最后一次键鼠活动那一刻），不是「发现它空闲的那一刻」：
        // 用后者的话，用户离开 5 分钟后回来，会发现那 5 分钟照样计薪，
        // 于是这个功能等于没有。
        //
        // **恢复判据只看「键鼠动没动」，绝不能再看 `snap.paused`。**
        // 这里踩过一个很贵的坑：早先写成 `if s.idle_detect && !snap.paused { A } else if w.idle_open { 清标记 }`，
        // 而我们自己刚开的那段暂停会让下一帧的 `snap.paused` 变成 true，
        // 于是走进 else 分支、把 `idle_open` 清掉、却**不关**那段暂停——
        // 表现就是「人回来了，金额从离开那一刻起再也不涨，状态还显示在岗」。
        // 用户分不清这是「没钱了」还是「程序坏了」，只能来问。
        if s.idle_detect {
            let idle = platform::idle_seconds();
            match idle_step(
                true,
                w.idle_open,
                idle,
                s.idle_min as u64 * 60,
                snap.paused,
            ) {
                IdleStep::Pause => {
                    w.idle_open = true;
                    acts.push(Act::Pause(ts - idle as i64));
                }
                IdleStep::Resume => {
                    w.idle_open = false;
                    acts.push(Act::Resume(ts - idle as i64));
                }
                IdleStep::Nothing => {}
            }
            // 空闲这一路全靠系统 API，出问题时界面上一点痕迹都没有——
            // 只在「真的闲着」时记一行，平时不写（阈值 10 秒，最多 30 秒一条）。
            if idle >= 10 && (idle as i64 - w.idle_logged).unsigned_abs() >= 30 {
                w.idle_logged = idle as i64;
                log::w(&format!(
                    "空闲 {idle}s（阈值 {}s，暂停中={}，这段是空闲开的={}）",
                    s.idle_min * 60,
                    snap.paused,
                    w.idle_open
                ));
            }
        } else if w.idle_open {
            // 用户自己点了暂停：这段归他，我们不再管
            w.idle_open = false;
        }

        // ---- 弹性排班：把今天钉在「第一次动键鼠」那一刻 ----
        //
        // 取 `ts - idle`（最后一次输入发生的时刻），不是 `ts`：
        // 用 `ts` 会把「程序启动到你动鼠标」那几秒也算成工时。
        // idle ≥ 5 分钟说明人还没坐下，这一帧先不钉——等他碰到鼠标再说，
        // 否则开机自启的机器会在凌晨三点给自己钉一个上班点。
        if s.week.flex && !snap.paused && s.today_anchor(now).is_none() {
            let idle = platform::idle_seconds();
            if idle < 300 {
                acts.push(Act::Anchor(ts - idle as i64));
            }
        }

        let dnd = s.notify.in_dnd(min_of_day);
        // 静默档不说话：午休 / 久坐这两条「主动关心」的提醒一律不发，
        // 只留「下班了，今天到账多少」那一条——那一条是账，不是唠叨。
        let chatty = s.copy_style != settings::CopyStyle::Silent;

        // 第一帧只认识现状：状态记下来，一条通知都不发。
        // 不这样做的话，下午六点之后第一次启动会立刻弹「今日已到账」——
        // 那不是提醒，那是把历史当成刚刚发生的事。
        if !w.seeded {
            w.seeded = true;
            w.prev_state = snap.state.clone();
            w.sit_at = if snap.state == "on" { ts } else { 0 };
            return;
        }

        // ---- 下班通知 ----
        //
        // 判据全在 `end_notice_due` 里，这里只负责文案。
        let shift_over = s.calendar_shifted(s.flex_delta(now)).shift.past_end(min_of_day);
        let ended = end_notice_due(&snap.state, shift_over, snap.worked_sec, w.end_fired);
        if ended {
            if !dnd && !matches!(s.notify.end, settings::NotifyEnd::Off) {
                let amount = yuan(snap.amount_milli, 2);
                // 语气跟着「文案风格」走，但**报什么**仍由 `notify.end` 决定——
                // 静默档例外：它连「报什么」都不问，只说数。
                let body = match s.copy_style {
                    settings::CopyStyle::Silent => amount.clone(),
                    settings::CopyStyle::Plain => match s.notify.end {
                        settings::NotifyEnd::AmountWork => {
                            format!("{amount}，工时 {}。", hm_text(snap.worked_sec))
                        }
                        settings::NotifyEnd::AmountEq => format!(
                            "{amount}，折合 {} {:.1} 杯。",
                            s.eq_name,
                            snap.eq_cups_x10 as f64 / 10.0
                        ),
                        _ => format!("{amount}。"),
                    },
                    settings::CopyStyle::Emotional => match s.notify.end {
                        settings::NotifyEnd::AmountWork => format!(
                            "{amount}，今天干了 {}。明天见",
                            hm_text(snap.worked_sec)
                        ),
                        settings::NotifyEnd::AmountEq => format!(
                            "{amount}，折合 {} {:.1} 杯。明天见",
                            s.eq_name,
                            snap.eq_cups_x10 as f64 / 10.0
                        ),
                        _ => format!("{amount}，明天见"),
                    },
                };
                acts.push(Act::Toast("今日已到账".into(), body, "ok"));
            }
            w.end_fired = true;
        }

        // ---- 午休提醒 ----
        let before = s.notify.lunch_before_min.saturating_mul(60);
        if chatty
            && s.notify.lunch
            && !w.lunch_fired
            && snap.lunch_in_sec > 0
            && snap.lunch_in_sec <= before
        {
            w.lunch_fired = true;
            if !dnd {
                acts.push(Act::Toast(
                    "该去吃饭了".into(),
                    format!("还有 {} 就午休了，先去占个座", hm_text(snap.lunch_in_sec)),
                    "info",
                ));
            }
        }

        // ---- 久坐提醒 ----
        if chatty && s.notify.sit && snap.state == "on" {
            if w.sit_at == 0 {
                w.sit_at = ts;
            } else if ts - w.sit_at >= s.notify.sit_min as i64 * 60 {
                w.sit_at = ts;
                if !dnd {
                    acts.push(Act::Toast(
                        "起来动一动".into(),
                        format!("你已经坐了 {} 分钟，站一会儿再赚", s.notify.sit_min),
                        "warn",
                    ));
                }
            }
        } else if snap.state != "on" {
            w.sit_at = 0;
        }

        if w.prev_state != snap.state {
            log::w(&format!(
                "状态 {} → {}（已计 {}s / 日额 {}s）",
                if w.prev_state.is_empty() { "∅" } else { w.prev_state.as_str() },
                snap.state,
                snap.worked_sec,
                s.daily_sec
            ));
        }
        w.prev_state = snap.state.clone();
    }

    for a in acts {
        match a {
            Act::Pause(at) => {
                st.mutate(|s| {
                    // 已经因为别的原因暂停着，就别叠第二段（会重复扣）
                    if !is_paused(&s.spans()) {
                        s.pauses.push(PauseRec {
                            start: at,
                            end: None,
                        });
                    }
                });
            }
            Act::Anchor(at) => {
                st.mutate(|s| {
                    // 只在「今天还没有锚点」时才写。两帧撞在一起时后写的那个
                    // 会把上班点往后推，金额当场缩水——这种错看起来像时钟不准。
                    let now = crate::clock(s);
                    if s.today_anchor(now).is_none() {
                        s.day_anchor = Some(at);
                    }
                });
            }
            Act::Resume(at) => {
                st.mutate(|s| {
                    if let Some(last) = s.pauses.last_mut() {
                        if last.end.is_none() {
                            last.end = Some(at.max(last.start));
                        }
                    }
                });
            }
            Act::Toast(title, body, tone) => {
                // 建窗口必须在主线程上：`tick` 是命令，跑在工作线程里。
                let app2 = app.clone();
                let _ = app.run_on_main_thread(move || {
                    toast::show(&app2, &title, &body, tone);
                });
            }
        }
    }
}

/// 主循环：前端每帧调一次（约 10 Hz）。
///
/// 注意这里**没有任何累加**。金额永远是「当前时刻」的纯函数，
/// 所以合盖 8 小时再唤醒，数字只跟真实经过时间有关。
/// 悬浮窗上按了右键：在鼠标位置弹自绘深色菜单。
/// 鼠标坐标不从前端传——`GetCursorPos` 在 Rust 侧拿物理坐标，跨 DPI 屏更准。
#[tauri::command]
fn hud_menu(app: tauri::AppHandle) -> Result<(), String> {
    menu::open(&app).map_err(|e| {
        log::w(&format!("hud_menu 打开菜单失败：{e}"));
        e.to_string()
    })
}

#[tauri::command]
fn tick(app: tauri::State<AppState>, handle: tauri::AppHandle) -> Snapshot {
    let mut s = app.settings();
    let now = clock(&s);

    // 顺手清掉已经过期的记录，否则它们会一直留在文件里，明天又被读出来。
    // 只在真的需要时写盘，不是每帧都写。
    let lo = day_start(now);
    let stale_ot = s.ot_start.map_or(false, |t| t < lo);
    let stale_lunch_day = s.lunch_ots.iter().any(|l| l.start < lo);
    let lunch_should_close = s.lunch_ot_open() && phase_at(now, &s.calendar()) != Phase::Lunch;
    if stale_ot || stale_lunch_day || lunch_should_close {
        let ts = now.timestamp();
        s = app.mutate(|x| {
            if stale_ot {
                x.ot_start = None;
            }
            if stale_lunch_day {
                x.lunch_ots.retain(|l| l.start >= lo);
            }
            // 13:30 一过就收口：长度本来就被 cap 卡住了，这里只是把状态收干净
            if lunch_should_close {
                x.close_lunch_ot(ts);
            }
        });
    }

    let snap = snapshot_at(now, &s);
    observe(&handle, &app, &snap, now);
    snap
}

/// 悬浮窗底部那个按钮。`start` = 我在加班，`stop` = 结束加班。
#[tauri::command]
fn act(start: bool, app: tauri::State<AppState>) -> Snapshot {
    let s = app.settings();
    let now = clock(&s);
    app.set_ot(start, now, &s.calendar());
    snapshot_at(now, &s)
}

/// 鼠标穿透：开启后点击直接落到下面的窗口。
///
/// 用 `set_ignore_cursor_events` 而不是「隐藏窗口」——后者会丢掉置顶层级，
/// 重新显示时窗口可能被其它置顶程序压住。
fn apply_click_through(app: &tauri::AppHandle, on: bool) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(HUD_LABEL) {
        w.set_ignore_cursor_events(on).map_err(|e| e.to_string())?;
    }
    // 记下来：下次启动要恢复，托盘菜单的勾选状态也靠它
    app.state::<AppState>().set_click_through(on);
    // 窗口上那个 ◎ 改了穿透，托盘菜单得跟着变，否则两个入口各说各话
    tray::sync_checks(app);
    Ok(())
}

#[tauri::command]
fn set_click_through(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    apply_click_through(&app, on)
}

/// 暂停 / 恢复。托盘菜单专用（悬浮窗上没有这个按钮，原型稿里就没有）。
#[tauri::command]
fn toggle_pause(app: tauri::State<AppState>, handle: tauri::AppHandle) -> bool {
    let paused = app.toggle_pause(Local::now().timestamp());
    tray::sync_checks(&handle);
    paused
}

/// 首帧画完了，可以露脸了。
///
/// 窗口在配置里是 `visible: false`：WebView2 起来要一秒多，这段时间窗口是
/// 一块纯黑的圆角矩形。先藏起来、等第一帧渲染完再显示，开机时就不会闪一下黑块。
/// 这个调用必须是幂等的——前端每次热重载都会再调一次。
#[tauri::command]
fn ready(window: tauri::Window, handle: tauri::AppHandle, app: tauri::State<AppState>) {
    // 静默启动：第一次露脸的请求直接吃掉，只留托盘。
    // 只吃第一次——用户之后从托盘/快捷键叫出来，`ready` 再来就该显示。
    // 首启也走同一条路：那一天用户还没填自己的月薪，露出来只会看到示例的 8000。
    let mut show = true;
    {
        let mut g = app.lock();
        if !g.watch.shown_once {
            g.watch.shown_once = true;
            show = !g.settings.silent_start;
        }
    }
    if app.is_fresh() {
        show = false;
    }
    if show {
        let _ = window.show();
    }
    log::w(&format!(
        "HUD 首帧就绪 · 露脸={} · 首启={}",
        show,
        app.is_fresh()
    ));
    // 窗口是藏起来启动的，托盘菜单里「显示悬浮窗」的勾选得跟着变成「已显示」
    tray::sync_checks(&handle);
}

/// 设置页问一句「我是不是第一次被打开」。
///
/// 前端据此决定要不要显示引导条。做成命令而不是往 URL 里塞参数，
/// 是因为设置窗口可能已经开着（双击托盘又开一次），而 URL 只在创建那一下有效。
#[tauri::command]
fn first_run(app: tauri::State<AppState>) -> bool {
    app.is_fresh()
}

/// 让窗口贴住内容。
///
/// 为什么必须做：窗口是透明的，超出内容的那部分仍然会吃掉鼠标点击——
/// 用户点桌面上的图标，点到的却是这块看不见的窗口。所以「白干条出现时窗口长高」
/// 不只是好看，是可用性问题。
///
/// 宽度也要传：三档尺寸的三套版式宽度不同（236 / 302 / 322），
/// 而宽度是 CSS 定的、高度是内容撑的，只有前端量得准。
#[tauri::command]
fn resize_hud(window: tauri::WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    let w = width.clamp(180.0, 640.0);
    let h = height.clamp(100.0, 640.0);
    window
        .set_size(tauri::LogicalSize::new(w, h))
        .map_err(|e| e.to_string())?;
    clamp_into_screen(&window);
    Ok(())
}

/// 切换尺寸档，返回切换后的快照。
///
/// 这里顺手把宽度改掉，不等前端下一帧：等的话中间会有一帧「窗口比内容宽」，
/// 多出来的透明区域会吃掉桌面上那一下点击。高度没法预估（要看白干条、
/// 动作行在不在），还是交给前端下一帧回填。
#[tauri::command]
fn cycle_size(window: tauri::WebviewWindow, dir: i32, app: tauri::State<AppState>) -> Snapshot {
    let size = app.cycle_size(dir);
    apply_width(&window, size);
    let s = app.settings();
    let now = clock(&s);
    snapshot_at(now, &s)
}

/// 立刻把窗口宽度改成这一档，高度先沿用旧的。
pub(crate) fn apply_width(win: &tauri::WebviewWindow, size: Size) {
    let Ok(scale) = win.scale_factor() else { return };
    let h = win
        .outer_size()
        .map(|s| s.height as f64 / scale)
        .unwrap_or(242.0);
    let _ = win.set_size(tauri::LogicalSize::new(size.width(), h));
    clamp_into_screen(win);
}

/// 变宽之后可能顶出屏幕右边缘，把它推回来。
///
/// 触发场景很具体：窗口贴在屏幕右侧（默认位置就是右上角），
/// 从标准档 302 切到展开档 322 就多出 20px 跑到屏幕外。不处理的话，
/// 那 20px 里的内容用户永远看不到，而且下次启动还会按存下来的坐标摆回去。
///
/// 注意用**物理**像素比较：`outer_position` 和 `monitor.size` 都是物理的，
/// 混进逻辑像素在 150% 缩放下会差出一大截。
///
/// 边界同样取**工作区**：上次关程序时窗口贴在屏幕底部（那时还在任务栏之上），
/// 下次开机任务栏还在那儿，按屏幕矩形夹就会把它放进任务栏底下。
fn clamp_into_screen(win: &tauri::WebviewWindow) {
    let Ok(pos) = win.outer_position() else { return };
    let Ok(size) = win.outer_size() else { return };
    // `hwnd()` 是 Windows 专属方法，macOS/Linux 没有。平台层的 work_area
    // 在 macOS/Linux 上忽略 hwnd（回退到 current_monitor），所以传 0 即可。
    #[cfg(target_os = "windows")]
    let hwnd = win.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(target_os = "windows"))]
    let hwnd: isize = 0;

    let area = platform::work_area(hwnd).or_else(|| {
        let mon = win.current_monitor().ok().flatten()?;
        let mp = mon.position();
        let ms = mon.size();
        Some((mp.x, mp.y, mp.x + ms.width as i32, mp.y + ms.height as i32))
    });
    let Some((left, top, right, bottom)) = area else { return };

    // 窗口比工作区还宽/高时（切到展开档 + 小屏），让它贴左上，别算出负数。
    let nx = pos.x.min(right - size.width as i32).max(left);
    let ny = pos.y.min(bottom - size.height as i32).max(top);

    if nx != pos.x || ny != pos.y {
        let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
}

/// 该用哪一份位置：先看这块屏自己的记录，再看兜底的那份。
///
/// 单独抽出来不是为了好看，是因为 `setup()` 里的代码没法测——而这个函数
/// 包着「换屏之后位置从哪来」这条唯一的判断。两条优先级搞反了，
/// 表现是「插上外接屏，悬浮窗还在笔记本那块屏上」，只有真机才看得见。
pub(crate) fn pick_position(s: &Settings, monitor: Option<&str>) -> Option<WinPos> {
    monitor
        .and_then(|name| s.win_by_monitor.get(name).copied())
        .or(s.win)
}

/// 把窗口放到某块屏的右上角，留 24px 边距。`None` = 窗口当前所在那块。
fn place_on(win: &tauri::WebviewWindow, monitor: Option<tauri::Monitor>) {
    let mon = match monitor {
        Some(m) => Some(m),
        None => win.current_monitor().ok().flatten(),
    };
    let Some(monitor) = mon else { return };
    let screen = monitor.size();
    let size = win.outer_size().unwrap_or_default();
    let scale = monitor.scale_factor();
    let margin = (24.0 * scale) as i32;
    let origin = monitor.position();
    let x = origin.x + screen.width as i32 - size.width as i32 - margin;
    let y = origin.y + margin;
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 默认位置：当前这块屏的右上角。
fn place_default(win: &tauri::WebviewWindow) {
    place_on(win, None);
}

/// 按相对坐标把窗口摆到主屏上，返回落位后的**逻辑**坐标。
///
/// `rx / ry` 取 0~1：0 贴左边/上边，1 贴右边/下边，两头各留 24px 边距。
/// 引导第 3 步拖那张示意图得到的就是这个比例——那块示意区是 520×132，
/// 和真实屏幕比例毫无关系，所以比例只能在窗口这边折算成像素。
///
/// 返回逻辑坐标是为了跟 `Settings.win` 的口径一致：`remember_pos` 存的也是
/// 逻辑像素（物理值除以该屏 scale）。混两种口径，高 DPI 屏上下次启动会跳位。
pub(crate) fn place_by_rel(
    win: &tauri::WebviewWindow,
    rx: f64,
    ry: f64,
) -> Option<crate::settings::WinPos> {
    let mon = win.primary_monitor().ok().flatten()?;
    let scale = mon.scale_factor();
    let origin = mon.position();
    let screen = mon.size();
    let size = win.outer_size().ok()?;
    let margin = (24.0 * scale) as i32;

    // 可用挪动范围要减掉窗口自身和两侧边距，否则 rx=1 时窗口会有一半探出去
    let free_x = (screen.width as i32 - size.width as i32 - margin * 2).max(0);
    let free_y = (screen.height as i32 - size.height as i32 - margin * 2).max(0);

    let rx = if rx.is_finite() { rx.clamp(0.0, 1.0) } else { 1.0 };
    let ry = if ry.is_finite() { ry.clamp(0.0, 1.0) } else { 1.0 };

    let x = origin.x + margin + (rx * free_x as f64).round() as i32;
    let y = origin.y + margin + (ry * free_y as f64).round() as i32;

    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));

    let s = if scale > 0.0 { scale } else { 1.0 };
    Some(crate::settings::WinPos {
        x: (x as f64 / s).round() as i32,
        y: (y as f64 / s).round() as i32,
    })
}

/// 「固定在主屏」用：主屏的右上角。
///
/// 跟 `place_default` 的差别只在「哪块屏」——不这么分，接着
/// `current_monitor()` 算就会在副屏上摆出「副屏右上角」，而那正是
/// 用户勾了「固定在主屏」想避开的东西。
fn place_on_primary(win: &tauri::WebviewWindow) {
    let primary = win.primary_monitor().ok().flatten();
    place_on(win, primary);
}

pub(crate) fn reset_position(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window(HUD_LABEL) {
        let state = app.state::<AppState>();
        // 「重置位置」要重置到用户选的那块屏，不是「当前碰巧在的那块」
        if state.settings().monitor_primary {
            place_on_primary(&win);
        } else {
            place_default(&win);
        }
        // 顺便把存下来的位置清掉（包括每屏那份），否则下次启动又跳回那个拉不回来的地方
        state.mutate(|s| {
            s.win = None;
            s.win_by_monitor.clear();
        });
    }
}


/// 把配置里的三个快捷键注册上去。
///
/// 每次设置改动都要重来一遍：用户可能刚把 `Ctrl+Shift+P` 改成别的，
/// 不重注册的话旧的那个还在抢按键。
/// 快捷键被别的程序占走了，当场说一声。
///
/// 为什么非得弹出来：GUI 程序没有控制台，只写日志的话用户看到的现象
/// 就是「这个键按了没反应」，然后来问「快捷键是不是坏的」。原型稿第 2790 行
/// 把这条写成硬要求——「注册失败要当场提示冲突，不能静默失败」。
fn hotkey_conflict(app: &tauri::AppHandle, id: u32, label: &str) {
    let what = match id {
        platform::HK_SHOW => "显隐",
        platform::HK_PASSTHRU => "穿透",
        platform::HK_CLOCK => "计时",
        platform::HK_EXTRA => "记一笔",
        _ => "快捷键",
    };
    // 文案要能塞进 340×96 的卡片：标题一行、正文一行。
    // 早先写成「…被别的程序占用了，去设置里换一组」会折成两行，
    // 第二行被卡片底边切掉——提示本身成了新的毛病。
    toast::show(
        app,
        &format!("{what}键没抢到"),
        &format!("{label} 被别人占着，换一组"),
        "warn",
    );
}

pub(crate) fn apply_hotkeys(app: &tauri::AppHandle) {
    let Some(h) = app.try_state::<platform::SharedHotkeys>() else {
        return;
    };
    let s = app.state::<AppState>().settings();
    let mut list = Vec::new();
    if s.hotkey.enabled {
        for (id, spec) in [
            (platform::HK_SHOW, &s.hotkey.show),
            (platform::HK_PASSTHRU, &s.hotkey.passthru),
            (platform::HK_CLOCK, &s.hotkey.clock),
            (platform::HK_EXTRA, &s.hotkey.extra),
        ] {
            // 解析不出来就只跳过这一个：写错一个不该让另外两个失效
            if let Some((mods, vk)) = platform::parse(spec) {
                list.push(platform::Binding {
                    id,
                    mods,
                    vk,
                    label: spec.trim().to_string(),
                });
            } else {
                log::w(&format!("快捷键 {id} 的写法解析失败：{spec:?}"));
            }
        }
    } else {
        log::w("全局快捷键：关（配置里没开）");
    }
    // 单独一层 let：`if let Ok(x) = mutex.lock()` 里的临时 Result 会活到
    // 整个 if 语句结束，而它借用的是 `h`（State 守卫），于是编译报
    // 「h dropped while still borrowed」。绑成具名局部变量就各归各位。
    let locked = h.0.lock();
    if let Ok(guard) = locked {
        guard.set(list);
    }
}

/// 快捷键按下之后干什么。**在热键线程上跑**，所以只做窗口操作和发消息，
/// 不做任何可能阻塞的事——阻塞会让另外两个快捷键一起失灵。
fn on_hotkey(app: &tauri::AppHandle, id: u32) {
    log::w(&format!("快捷键触发 id={id}"));
    match id {
        platform::HK_SHOW => {
            if let Some(w) = app.get_webview_window(HUD_LABEL) {
                if w.is_visible().unwrap_or(true) {
                    let _ = w.hide();
                } else {
                    let _ = w.show();
                }
            }
            tray::sync_checks(app);
        }
        platform::HK_CLOCK => {
            app.state::<AppState>()
                .toggle_pause(chrono::Local::now().timestamp());
            tray::sync_checks(app);
        }
        platform::HK_PASSTHRU => {
            // 读当前值再取反：热键线程上没有 `Window` 参数，走同一个 helper，
            // 免得命令和快捷键两条路各写一遍窗口调用最后走岔
            let on = !app.state::<AppState>().settings().click_through;
            let _ = apply_click_through(app, on);
        }
        platform::HK_EXTRA => {
            let _ = open_extra(app);
        }
        _ => {}
    }
}

/// 原型稿第 1375 行：**松手 200ms 后**才往边上吸。
///
/// 这个 200ms 不是随便定的。原来我们是「收到一个 `Moved` 就吸」，也就是
/// 用户还按着鼠标、系统正处在自己的拖动循环里的时候，我们从旁边把窗口
/// 挪走——两个东西抢同一个窗口，结果就是用户看到的那片**残影**：窗口被
/// 拖到任务栏上、被我们拽回任务栏上沿、又被拖动循环拖下去，来回钉在
/// 那儿，被拽走时腾出来的那块地方没人擦。
///
/// 等静默再吸，拖动全程就只归系统管，我们只在最后落一次。
const SNAP_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// 「刚动过、还没吸」的标记。位移只负责把它立起来，真正下判断的是等静默的那条线程。
#[derive(Default)]
struct SnapDebounce {
    pending: bool,
}

impl SnapDebounce {
    fn note_move(&mut self) {
        self.pending = true;
    }

    /// 取走一次待办。返回 true 表示「这次该吸了」。
    ///
    /// 取走（而不是只读）是关键：只读的话，静默之后每一轮都会再吸一次，
    /// 每 200ms 一次，永远停不下来。
    fn take(&mut self) -> bool {
        std::mem::take(&mut self.pending)
    }
}

static SNAP_POKE: std::sync::OnceLock<std::sync::mpsc::Sender<()>> = std::sync::OnceLock::new();

/// 专门等「拖完了没有」的一条线程。
///
/// 收到一次位移就重新计 200ms；安静满 200ms 才吸一次。
fn spawn_snapper(app: tauri::AppHandle) {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if SNAP_POKE.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("paypulse-snap".into())
        .spawn(move || {
            let mut deb = SnapDebounce::default();
            loop {
                match rx.recv_timeout(SNAP_DELAY) {
                    // 又动了：重新计时
                    Ok(()) => deb.note_move(),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if !deb.take() {
                            continue;
                        }
                        let th = app.state::<AppState>().settings().snap_px;
                        if let Some(win) = app.get_webview_window(HUD_LABEL) {
                            snap_to_edge(&win, th);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
}

/// 托盘看板：每秒把最新快照写到托盘提示上，顺带看一眼时区。
///
/// ## 为什么不挂在前端 `tick` 上
///
/// `tick` 是悬浮窗里的 JS 在调。窗口一藏起来，WebView 的定时器就会被系统降频
/// （后台标签页的节流在桌面端同样生效），而「窗口藏起来、只看托盘」恰好是
/// 托盘提示唯一有用的场景——它要是在那个场景下不更新，这个功能就等于没有。
/// 托盘是这套程序里唯一不依赖窗口的东西，就别让它去依赖窗口的 JS。
///
/// ## 为什么是 1 秒
///
/// 提示里的金额保留两位，¥49/时的薪水大约 1.5 秒跳一分；1 秒的节拍已经比数字
/// 本身的变化更快，再密只是白拿 CPU。时区也顺路在这一拍里查——它一天才变一次，
/// 但只有在跑着的时候查得到。
fn spawn_beacon(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("paypulse-beacon".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(1_000));
            let st = app.state::<AppState>();
            let s = st.settings();
            let now = clock(&s);
            let snap = snapshot_at(now, &s);
            tz_check(&app, &s);
            let h = app.clone();
            // 托盘 API 归主线程管：从这条线程直接调，轻则被忽略、重则把
            // Windows 的消息循环拧在一起，所以统一甩回主线程执行。
            let _ = app.run_on_main_thread(move || tray::sync_hint(&h, &snap));
        })
        .ok();
}

/// 时区判断的结论。
///
/// 抽成枚举是为了能被测试：`tz_check` 本身要 AppHandle，单测里造不出来，
/// 但「差多少小时才该问」这条边界正是最该被钉住的东西。
#[derive(Debug, PartialEq, Eq)]
enum TzAct {
    /// 什么都不用做（偏移没变）
    Nothing,
    /// 静默把基准更新成现在这个偏移
    Remember,
    /// 停下来问一次
    Ask,
}

/// 纯判断，不碰任何全局状态。
fn tz_decide(prev: Option<i32>, now: i32, fixed: bool) -> TzAct {
    let Some(prev) = prev else {
        // 首次运行：只记不问。
        return TzAct::Remember;
    };
    if prev == now {
        return TzAct::Nothing;
    }
    // 差不到 2 小时（夏令时、+5:30 这类微调），或者用户已经选了「固定 UTC+8」
    // （那一档根本不读本地时区），都不值得打断。
    if (now - prev).abs() < 2 * 3600 || fixed {
        return TzAct::Remember;
    }
    TzAct::Ask
}

/// 时区变了的应对。规格 02-B：变更超过 2 小时就弹窗问「按哪边算」，一次确认。
///
/// 四种走法见 `tz_decide`。这里只负责「按结论执行」，以及一条执行期的细节：
/// 窗口已经弹着就别再弹（1 秒一拍，不判这个会一秒一弹）。
fn tz_check(app: &tauri::AppHandle, s: &Settings) {
    let off = Local::now().offset().local_minus_utc();
    // 首启**一个字段都不写**。这条判断不是洁癖：`fresh` 的判据就是
    // 「settings.json 还不存在」（见 setup 里那行），这里若顺手记一笔时区，
    // 文件就被创建了——用户这次关掉引导没填工资，下次启动会被当成老用户：
    // 不弹引导，直接看到一个按 8000 块示例算出来的金额。
    // 引导第 4 步落盘之后再记，那时写盘本来就是对的。
    if app.state::<AppState>().is_fresh() {
        return;
    }
    match tz_decide(s.tz_off, off, s.tz_fixed) {
        TzAct::Nothing => {}
        TzAct::Remember => tz_remember(app, off),
        TzAct::Ask => {
            if ask_win::is_open(app) {
                return;
            }
            let prev = s.tz_off.unwrap_or(off);
            log::w(&format!(
                "检测到时区变化：{} → {}，停一次问用户按哪边算",
                tz_label(prev),
                tz_label(off)
            ));
            ask_win::ask(app, prev, off);
        }
    }
}

/// 记下「这台机器当前时区」，下次拿它当基准。
fn tz_remember(app: &tauri::AppHandle, off: i32) {
    app.state::<AppState>().mutate(|x| x.tz_off = Some(off));
}

/// 秒偏移 → `UTC+8` / `UTC+5:30`。负偏移写 `UTC-3`。
pub(crate) fn tz_label(off_sec: i32) -> String {
    let sign = if off_sec < 0 { '-' } else { '+' };
    let m = off_sec.abs() / 60;
    let (h, mi) = (m / 60, m % 60);
    if mi == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{mi:02}")
    }
}

/// 系统当前的 UTC 偏移（秒）。东八区是 `28800`。
///
/// 单独抽一行是因为它有两个用途：一是判断时区变没变，二是「记下这个偏移」
/// 这条写盘路径。两处都必须读同一个数，否则「检测到 +9、记成 +8」这种事
/// 会让窗口每秒弹一次。
pub(crate) fn local_offset_sec() -> i32 {
    Local::now().offset().local_minus_utc()
}

/// 让已经开着的设置窗口重新拉一次配置。
///
/// 场景很具体：时区询问窗改了 `tz_fixed`，而设置窗口这会儿可能正开着，
/// 上面那个「时区策略」下拉还停在旧值。不刷的话，用户会看到「设置里写着跟随系统、
/// 实际已经按 UTC+8 算」——两个界面各说各话，比不做这个功能更糟。
pub(crate) fn refresh_settings_view(app: &tauri::AppHandle) {
    let _ = app.emit_to(settings_win::LABEL, "settings-changed", ());
}

/// 吸附正在改位置 → 别再处理它自己引发的那次 `Moved`。
///
/// 不加这个标志会看到窗口抖：吸附改位置 → 触发 Moved → 又算一次吸附 →
/// 位置又被改 → …… 直到阈值内不再触发为止（看起来像窗口在抽搐）。
static SNAPPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 一根轴上的吸附判定：贴左边/上边、贴右边/下边，还是不贴。
///
/// 单独抽成纯函数是因为这里踩过一个**只有真机能看见**的错：
/// 原来写成 `(right - (pos + len)).abs() <= th`，窗口一旦越过边线，
/// 差值变负、绝对值只会越来越大，于是再也吸不住——用户把悬浮窗往屏幕
/// 下方、右方一拖，只剩一条缝露在桌面上，松手它就停在那儿，看起来像
/// 「吸附根本没生效」。
///
/// 改成**单向**比较：`pos - lo <= th` 对「越过」也成立（负数更小），
/// 越出去多少都能拉回来贴住。
///
/// `free_lo` / `free_hi` 是「越过边线之后那边还有没有别的屏」：
/// 双屏时把窗口往右挪，越过边线那一刻它还挂在左屏名下，若无条件拉回来，
/// 就永远挪不到右屏。越出去且那边有屏，就不吸。
fn snap_axis(pos: i32, len: i32, lo: i32, hi: i32, th: i32, free_lo: bool, free_hi: bool) -> Option<i32> {
    // 贴起点：在阈值内，或者已经越过去了且那边是空桌面
    let near_lo = pos - lo <= th && (pos >= lo || free_lo);
    // 贴终点：这里是左闭右开，`hi - (pos + len)` 是窗口右边到边界还剩多少
    let near_hi = hi - (pos + len) <= th && (pos + len <= hi || free_hi);
    if near_lo {
        Some(lo)
    } else if near_hi {
        Some(hi - len)
    } else {
        None
    }
}

/// 松手时贴近屏幕边缘就贴上去。
///
/// 阈值从配置来（8–40pt，0 = 关）。用**物理**像素算：`outer_position`
/// 和 `monitor` 都是物理的，混进逻辑像素在 125% 缩放下会差出小半屏。
///
/// 边界用的是**工作区**（屏幕减掉任务栏）：贴屏幕底边的话，窗口下半截
/// 会被任务栏盖着，用户看到的不是「贴住了」而是「卡了一半」。
fn snap_to_edge(win: &tauri::WebviewWindow, threshold: u32) {
    if threshold == 0 || SNAPPING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    #[cfg(target_os = "windows")]
    let hwnd = win.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    #[cfg(not(target_os = "windows"))]
    let hwnd: isize = 0;
    let Ok(pos) = win.outer_position() else {
        SNAPPING.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    };
    let Ok(size) = win.outer_size() else {
        SNAPPING.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    };

    // 工作区拿不到（平台不支持、句柄还没好）就退回当前屏的矩形，
    // 至少不会因为一次 API 失败就把吸附整个关掉。
    let area = platform::work_area(hwnd).or_else(|| {
        let mon = win.current_monitor().ok().flatten()?;
        let mp = mon.position();
        let ms = mon.size();
        Some((
            mp.x,
            mp.y,
            mp.x + ms.width as i32,
            mp.y + ms.height as i32,
        ))
    });

    if let Some((left, top, right, bottom)) = area {
        let scale = win.scale_factor().unwrap_or(1.0);
        // 阈值是逻辑 pt，换算成物理像素再和坐标比
        let th = ((threshold as f64 * scale).round() as i32).max(1);

        let cur = platform::monitor_of(hwnd);
        // 「越出去的那边」是空桌面（或者还是自己这块屏）才允许拉回来
        let free = |x: i32, y: i32| {
            let m = platform::monitor_at(x, y);
            m == 0 || m == cur
        };
        let mid_x = pos.x + size.width as i32 / 2;
        let mid_y = pos.y + size.height as i32 / 2;

        let nx = snap_axis(
            pos.x,
            size.width as i32,
            left,
            right,
            th,
            free(left - 1, mid_y),
            free(right + 1, mid_y),
        );
        let ny = snap_axis(
            pos.y,
            size.height as i32,
            top,
            bottom,
            th,
            free(mid_x, top - 1),
            free(mid_x, bottom + 1),
        );

        let fx = nx.unwrap_or(pos.x);
        let fy = ny.unwrap_or(pos.y);
        if fx != pos.x || fy != pos.y {
            let _ = win.set_position(tauri::PhysicalPosition::new(fx, fy));
            // 透明窗口（`WS_EX_LAYERED`）挪走之后，它原来压着的那块地方
            // 不一定有人重画——从任务栏上沿被拽上来时，任务栏懒得擦，
            // 桌面就留一片窗口的残影。这里把「新旧两块」合起来请求重画。
            let (w, h) = (size.width as i32, size.height as i32);
            platform::repaint_rect(
                pos.x.min(fx),
                pos.y.min(fy),
                pos.x.max(fx) + w,
                pos.y.max(fy) + h,
            );
        }
    }
    SNAPPING.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 打开设置窗口。托盘菜单和悬浮窗都可以调它。
pub(crate) fn open_settings(app: &tauri::AppHandle) -> tauri::Result<()> {
    settings_win::open(app)
}

/// 「记一笔」那扇小窗的标签。
pub(crate) const EXTRA_LABEL: &str = "extra";

/// 记一笔额外进账的小窗。
///
/// 原型稿里没有这扇窗——它只有一行「记录一笔额外收入 / 快速追加」的说明。
/// 按下键之后弹什么由实现决定：一张只有金额的卡，回车记下、Esc 走开、
/// 点到别处也走开。记完自己消失，不留「保存」按钮（和设置页一致）。
pub(crate) fn open_extra(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(EXTRA_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        EXTRA_LABEL,
        tauri::WebviewUrl::App("extra.html".into()),
    )
    .title("记一笔")
    .inner_size(340.0, 196.0)
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .center()
    .build()?;

    // 点到别处就收起来：这扇窗是「插一句就走」的，不该留在屏幕上等人处理。
    // 只 hide 不 destroy——下次按快捷键是同一个窗口，省一次 WebView 创建。
    // 注意这个回调要挂在 build 之后：`WebviewWindowBuilder` 上没有
    // `on_window_event`，那是窗口自己的方法。
    if let Some(w) = app.get_webview_window(EXTRA_LABEL) {
        let me = w.clone();
        w.on_window_event(move |e| {
            if let tauri::WindowEvent::Focused(false) = e {
                let _ = me.hide();
            }
        });
    }
    Ok(())
}

/// 记一笔额外进账。
///
/// 金额穿过来的是**字符串**：`12.34` 这种数在 JS 里乘 10000 会得到
/// 123399.99999999999，一舍一入就少一毫。换算全在 `parse_yuan` 里做。
#[tauri::command]
fn add_extra(app: tauri::AppHandle, amount_text: String) -> Result<String, String> {
    let milli = settings::parse_yuan(&amount_text).ok_or_else(|| "这个金额看不懂".to_string())?;
    if milli <= 0 {
        return Err("金额要大于 0".into());
    }
    if milli > settings::MONTH_MAX {
        return Err("这个数太大了".into());
    }
    let date = Local::now().format("%Y-%m-%d").to_string();
    app.state::<AppState>().mutate(|s| {
        s.extras.push(settings::ExtraEntry {
            date: date.clone(),
            amount_milli: milli,
        });
    });
    Ok(yuan(milli, 2))
}

/// 记完（或按 Esc）把窗收起来。
///
/// 放一个命令而不是让前端自己 `hide()`：前端要 hide 就得开窗口权限，
/// 而这一扇窗的权限集越小越好——它只需要能调这两个命令。
#[tauri::command]
fn close_extra(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(EXTRA_LABEL) {
        let _ = w.hide();
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            // 日志要尽早起来：后面每一步失败都靠它留痕
            log::init(dir.clone());
            log::w(&format!(
                "---- 启动 v{} · 配置目录 {}",
                env!("CARGO_PKG_VERSION"),
                dir.display()
            ));
            // 首启判定必须发生在 `load_from` 之前：那个函数读不到文件也不会创建它，
            // 但它返回的是 `Settings::default()`——光看返回值分不出「新装机」和「配置损坏」。
            let fresh = !settings::settings_path(&dir).exists();
            let mut cfg = settings::load_from(&dir);

            // 启动时清一次过期暂停区间：不清会越攒越多，每次 tick 都要遍历一遍
            let mut spans = cfg.spans();
            prune_pauses(clock(&cfg), &mut spans);
            cfg.pauses = spans
                .iter()
                .map(|s| PauseRec {
                    start: s.start,
                    end: s.end,
                })
                .collect();

            // 再收一遍「上个进程没来得及结束」的暂停：这一段留着就是永久冻结。
            let dropped = drop_dangling_pauses(&mut cfg);
            if dropped > 0 {
                log::w(&format!(
                    "收掉 {dropped} 段没收口的暂停（上个进程没正常收尾，留着会冻住今日已赚）"
                ));
            }

            app.manage(AppState::new(dir, cfg.clone(), fresh));

            if let Some(win) = app.get_webview_window("hud") {
                // 先问「这块屏上次在哪儿」，没有才退回兜底那份，都没有就贴右上角。
                // 恢复之后一律夹一次：那块屏被拔掉了的话，坐标可能落在屏幕外，
                // 而落在屏幕外的窗口用户是拖不回来的（看不见也就抓不到）。
                if cfg.monitor_primary {
                    place_on_primary(&win);
                } else {
                    let here = win
                        .current_monitor()
                        .ok()
                        .flatten()
                        .and_then(|m| m.name().cloned());
                    match pick_position(&cfg, here.as_deref()) {
                        Some(p) => {
                            let _ = win.set_position(tauri::LogicalPosition::new(
                                p.x as f64,
                                p.y as f64,
                            ));
                            clamp_into_screen(&win);
                        }
                        None => place_default(&win),
                    }
                }
                if cfg.click_through {
                    let _ = win.set_ignore_cursor_events(true);
                }
            }

            // 开机自启：配置里开着就顺手把注册表对齐一次。
            // 必须每次启动都对齐——程序被挪了位置（换目录、改安装路径）以后，
            // 注册表里那条旧路径会让下次开机启不起来，而用户不会知道为什么。
            if cfg.autostart {
                if let Ok(exe) = std::env::current_exe() {
                    match platform::set_autostart(true, &exe) {
                        Ok(()) => log::w(&format!("开机自启已同步：{}", exe.display())),
                        Err(e) => {
                            log::w(&format!("开机自启同步失败：{e}"));
                            eprintln!("开机自启同步失败：{e}");
                        }
                    }
                }
            } else {
                log::w("开机自启：关（配置里没开，不写注册表）");
            }

            // 全局快捷键。先挂状态再注册——回调里要读 AppState。
            let hot = platform::spawn_hotkeys(
                {
                    let h = app.handle().clone();
                    move |id| on_hotkey(&h, id)
                },
                {
                    // 注册失败必须看得见。跑在热键线程上，所以甩回主线程再弹窗。
                    let h = app.handle().clone();
                    move |id, label| {
                        let h2 = h.clone();
                        let _ = h.run_on_main_thread(move || hotkey_conflict(&h2, id, &label));
                    }
                },
            );
            app.manage(platform::SharedHotkeys(std::sync::Mutex::new(hot)));
            apply_hotkeys(app.handle());

            tray::setup(app.handle())?;

            // 悬浮窗右键菜单改成自绘深色窗（menu.rs）：右键由前端转成 `hud_menu`
            // 命令并带上鼠标坐标。这里不再挂 ctx 菜单的 on_menu_event——
            // 那套原生菜单已经删掉，托盘菜单走它自己的 on_menu_event(on_menu)。

            // 等「拖完了没有」的那条线程。窗口位移只往它那儿敲一下，
            // 吸附由它安静 200ms 之后落一次（原型稿 1375 行的写法）。
            spawn_snapper(app.handle().clone());

            // 通知窗先建好藏着：到点只 show，不再现建窗口。
            // 现建那条路实测弹出来是空白透明的（详见 toast::prewarm 的注释）。
            toast::prewarm(app.handle());

            // 右键菜单窗、今日明细窗同理：懒创建的透明窗口第一次 show 是空白的，
            // 预先建好让页面加载一次，用的时候只 show。
            menu::prewarm(app.handle());
            detail::prewarm(app.handle());

            // 设置窗口也是懒创建 + 透明：首次启动（fresh）时要直接推出来，
            // 不预热的话那一帧就是白的。先建好藏着，open 只 show。
            settings_win::prewarm(app.handle());

            // 引导窗口同理。它在原型里是 560×640 的四步向导，只在首启弹一次，
            // 但预热不设条件：等 `setup()` 判断出 fresh 再建就晚了，第一帧是空白。
            onboard_win::prewarm(app.handle());

            // 时区询问窗：出差落地、开机、发现时区跨了 2 小时才用得上，
            // 一年可能一次都没有——但它出现的那一刻正是最不能白的一刻。
            ask_win::prewarm(app.handle());

            // 托盘看板：每秒把「今日已赚」写到托盘提示上，顺带看时区变没变。
            spawn_beacon(app.handle().clone());

            // 第一次启动：推开**四步引导**（不是设置页），悬浮窗继续藏着。
            //
            // 判据是「配置文件当时不存在」，所以升级上来的老用户不会被弹一次。
            // 用户若把这扇窗口关掉而没填，下次启动会再来一次——这是有意的：
            // 数据没落盘就等于没设置过。悬浮窗的露脸绑在第一次写盘上
            // （`apply_settings` 末尾的 `take_fresh()`），所以引导页里改来改去
            // 不会提前把它叫出来。
            if fresh {
                let _ = onboard_win::open(app.handle());
            }

            // 调试用：`PAYPULSE_ONBOARD=1` 启动时直接拉起引导窗口。
            // 非交互桌面上点不到托盘，首启又只发生一次，没有这个钩子就没法截图验版式。
            #[cfg(debug_assertions)]
            if std::env::var("PAYPULSE_ONBOARD").is_ok() {
                let _ = onboard_win::open(app.handle());
            }

            // 调试用：`PAYPULSE_ASK=1` 直接拉起「时区变了」询问窗。
            //
            // 复现它本来要靠改系统时区：谁也不会为了验版式去把机器调到东京。
            // 这里把「上次」当成真实偏移、「现在」当成 +1 小时——也就是一次东飞落地，
            // 页面上的四个时刻全部照真实算法算出来（当地 09:00 = 北京 08:00 …），
            // 不是写死的假数据。真实触发阈值仍是 2 小时（见 `tz_decide`），
            // 这条钩子只是绕开阈值把窗口叫出来。
            #[cfg(debug_assertions)]
            if std::env::var("PAYPULSE_ASK").is_ok() {
                let here = local_offset_sec();
                ask_win::ask(app.handle(), here, here + 3600);
            }

            // 调试用：`PAYPULSE_SETTINGS=1` 启动时直接把设置窗口拉起来。
            //
            // 为什么要这个钩子：设置窗口本来只能从托盘菜单打开，而截图/自动化
            // 跑在非交互桌面上，既点不到托盘也没有通知区域。只在 debug 构建里生效，
            // 发布版不会因为这个环境变量多出任何行为。
            #[cfg(debug_assertions)]
            if std::env::var("PAYPULSE_SETTINGS").is_ok() {
                let _ = open_settings(app.handle());
            }

            // 调试用：`PAYPULSE_TOAST=1` 启动两秒后弹一条通知。
            //
            // 「到点通知没弹」这件事没法靠等：等到下班点才验一次，一天只能验一次，
            // 而且分不清是「没触发」还是「窗口建了但空白」。这个钩子把后半段
            // 单独拎出来验——两秒后一定会有一条，看得见就是窗口和渲染没问题。
            #[cfg(debug_assertions)]
            if std::env::var("PAYPULSE_TOAST").is_ok() {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(2000));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        toast::show(&h2, "今日已到账", "¥367.82，明天见", "ok");
                    });
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Alt+F4 之类：按设置决定是「最小化到托盘」还是真退出。
            // 悬浮窗没有标题栏，但快捷键和任务栏菜单照样能关它。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == HUD_LABEL {
                    let app = window.app_handle();
                    if matches!(
                        app.state::<AppState>().settings().close_action,
                        settings::CloseAction::Tray
                    ) {
                        api.prevent_close();
                        let _ = window.hide();
                        tray::sync_checks(app);
                    }
                }
                return;
            }

            if let tauri::WindowEvent::Moved(pos) = event {
                // 只记悬浮窗的位置。设置窗口也会发 Moved，不拦的话
                // 把设置窗口拖一下，悬浮窗下次启动就会跳到那个位置去。
                if window.label() != HUD_LABEL {
                    return;
                }
                let app = window.app_handle();
                let scale = window.scale_factor().unwrap_or(1.0);
                // 记在这块屏名下。拿不到名字（虚拟屏可能返回空）就只写兜底那份，
                // 功能退化但不会记错地方。
                let mon = window
                    .current_monitor()
                    .ok()
                    .flatten()
                    .and_then(|m| m.name().cloned());
                let st = app.state::<AppState>();
                st.remember_pos(pos.x as f64, pos.y as f64, scale, mon);
                // 只敲一下「刚动过」，**不在这里吸**。拖动过程中系统自己
                // 正握着这扇窗，我们插进去改位置就会打架、留下残影。
                if let Some(tx) = SNAP_POKE.get() {
                    let _ = tx.send(());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            tick,
            act,
            ready,
            hud_menu,
            set_click_through,
            toggle_pause,
            resize_hud,
            cycle_size,
            add_extra,
            close_extra,
            detail::detail_view,
            detail::close_detail,
            menu::menu_state,
            menu::menu_action,
            menu::menu_close,
            settings_win::get_settings,
            settings_win::apply_settings,
            first_run,
            ask_win::ask_pull,
            ask_win::ask_answer,
            update::check_update,
            toast::toast_dismiss,
            toast::toast_pull
        ])
        .build(tauri::generate_context!())
        .expect("启动失败：窗口创建异常");

    app.run(|handle, event| match event {
        // 谁让它退的，得留个底。`code` 是 `app.exit(n)` 传进来的；
        // `None` 表示「没有窗口了」——用户点了关闭、或者系统在关机。
        tauri::RunEvent::ExitRequested { code, .. } => match code {
            Some(n) => log::w(&format!("请求退出：app.exit({n})")),
            None => log::w("请求退出：没有窗口了（点了关闭 或 系统在关机）"),
        },
        tauri::RunEvent::Exit => {
            // 节流的最后一次位移可能还没落盘，退出前补一次
            handle.state::<AppState>().flush();
            log::w("进程已退出");
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// 2026-11-10 是周二，也就是原型稿里那张截图的那一天。
    ///
    /// 测试里一律造 `FixedOffset`（跟 `snapshot_at` 的入参同类型），
    /// 但用 `Local` 造——这样断言里的「09:00」就是这台机器上的 09:00，
    /// 不会因为跑测试的机器时区不同而整体平移 8 小时。
    fn at(h: u32, mi: u32) -> DateTime<FixedOffset> {
        Local
            .with_ymd_and_hms(2026, 11, 10, h, mi, 0)
            .single()
            .unwrap()
            .fixed_offset()
    }

    fn day(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        Local
            .with_ymd_and_hms(2026, 11, d, h, mi, 0)
            .single()
            .unwrap()
            .fixed_offset()
    }

    fn snap(now: DateTime<FixedOffset>) -> Snapshot {
        snapshot_at(now, &Settings::default())
    }

    fn snap_with(now: DateTime<FixedOffset>, s: &Settings) -> Snapshot {
        snapshot_at(now, s)
    }

    /// 弹性排班：今天第一次动键鼠是几点，这一天的上班点就是几点。
    ///
    /// 平移的是**整套**作息（午休跟着走），不是只把起点挪一下——
    /// 只挪起点的话，10:30 上班的人在 12:00 就被判成午休，白干条会莫名出现在中午。
    #[test]
    fn flex_start_slides_the_whole_day() {
        let mut s = Settings::default();
        s.week.flex = true;
        s.day_anchor = Some(at(10, 30).timestamp());

        // 10:30 坐下、12:00 看：正好 5400 秒。按固定 09:00 算会是 9000 秒，
        // 两个数差 3600——这就是「早上那两小时到底算不算」的差别。
        let noon = snap_with(at(12, 0), &s);
        assert_eq!(noon.worked_sec, 5400);
        assert_eq!(noon.state, "on", "12:00 还在岗：午休已经平移到 13:30");

        // 13:45 = 平移后的午休中（原 12:00–13:30 → 13:30–15:00）。
        // 已计薪 10800 秒 = 10:30→13:45 的 11700 秒扣掉已走过的 15 分钟午休。
        let lunch = snap_with(at(13, 45), &s);
        assert_eq!(lunch.state, "break");
        assert_eq!(lunch.worked_sec, 10800);
    }

    /// 弹性模式下「今天还没动过键鼠」：一分钱都不记。
    ///
    /// 不判这一条，退回配置的 09:00 会让 10:30 才坐下的人看到
    /// 「09:00 到现在已经赚了 80 块」——那不是估算，那是编的。
    #[test]
    fn flex_without_any_activity_pays_nothing() {
        let mut s = Settings::default();
        s.week.flex = true;
        s.day_anchor = None;

        let snap_ = snap_with(at(11, 0), &s);
        assert_eq!(snap_.worked_sec, 0);
        assert_eq!(snap_.amount_milli, 0);
        assert_eq!(snap_.progress, 0.0);
    }

    /// 昨天的锚点不能拿来定今天的上班点。
    ///
    /// 这一条是「跨天」最容易漏的地方：锚点留在配置里，第二天照着它平移，
    /// 于是今天的班次从昨天那个时刻开始算，金额一早就多出一截。
    #[test]
    fn yesterdays_anchor_does_not_apply_to_today() {
        let mut s = Settings::default();
        s.week.flex = true;
        s.day_anchor = Some(day(9, 9, 30).timestamp()); // 昨天 09:30

        let snap_ = snap_with(at(11, 0), &s);
        assert_eq!(snap_.worked_sec, 0, "昨天的锚点不许给今天定价");
    }

    /// 换屏之后位置从哪儿来：先认这块屏自己的记录，再退回兜底那份，
    /// 两份都没有才交给默认摆放。
    ///
    /// 顺序反了的表现很具体：插上外接屏，悬浮窗还留在笔记本那块屏上——
    /// 而这正是用户勾「多显示器」想解决的事。
    #[test]
    fn position_comes_from_the_screen_it_was_on() {
        let mut s = Settings::default();
        s.win = Some(WinPos { x: 10, y: 20 });
        s.win_by_monitor
            .insert("DISPLAY2".into(), WinPos { x: 900, y: 40 });

        assert_eq!(
            pick_position(&s, Some("DISPLAY2")),
            Some(WinPos { x: 900, y: 40 }),
            "这块屏有自己的记录就用它"
        );
        assert_eq!(
            pick_position(&s, Some("DISPLAY9")),
            Some(WinPos { x: 10, y: 20 }),
            "第一次来这块屏，退回兜底那份"
        );
        assert_eq!(
            pick_position(&s, None),
            Some(WinPos { x: 10, y: 20 }),
            "拿不到显示器名（部分虚拟屏返回空名）时同样退回兜底"
        );

        s.win = None;
        assert_eq!(
            pick_position(&s, Some("DISPLAY9")),
            None,
            "两份都没记过，交给默认摆放（贴当前屏右上角）"
        );
    }

    /// 满勤必须精确落在 367.816 元 —— 原型稿上那个数字。
    #[test]
    fn full_day_lands_on_screenshot_value() {
        let s = snap(at(18, 0));
        assert_eq!(s.amount_milli, 367_816);
        assert_eq!(s.remain_sec, 0);
        assert!((s.progress - 1.0).abs() < 1e-9);
        assert_eq!(s.state, "done", "18:00 整就该是已下班");
    }

    /// 下班后「已工作」必须冻结在日工时，不能继续叠加。
    /// 18:00 和 22:00 的 worked_sec 都该是 27000（日工时），
    /// 多出来的那部分只进 ot_sec（右下角「已过」），不该涨进「已工作」。
    #[test]
    fn worked_freezes_after_shift_end() {
        let at_end = snap(at(18, 0));
        let late = snap(at(22, 0));
        assert_eq!(at_end.worked_sec, 27_000);
        assert_eq!(late.worked_sec, 27_000, "下班后已工作不该继续涨");
        // 22:00 比 18:00 多 4 小时 = 14400 秒，全进 ot_sec
        assert_eq!(late.ot_sec, 14_400);
        assert_eq!(at_end.ot_sec, 0);
    }

    /// 上午过半：进度条和「距下班」都该正好是一半。
    /// 14:15 时已计薪「5h15m − 1h30m 午休」= 13500 秒，正好是 27000 的一半。
    #[test]
    fn halfway_is_actually_half() {
        let s = snap(at(14, 15));
        assert!((s.progress - 0.5).abs() < 1e-9, "progress = {}", s.progress);
        assert_eq!(s.remain_sec, 13_500);
        assert_eq!(s.worked_sec, 13_500);
        assert_eq!(s.state, "on");
        // 367.816 的一半，允许 1 毫的取整误差
        assert!((s.amount_milli - 183_908).abs() <= 1);
    }

    /// 上班前：金额 0，但还是「在岗」，不能是「已下班」。
    #[test]
    fn before_work_shows_zero_but_stays_on() {
        let s = snap(at(8, 30));
        assert_eq!(s.amount_milli, 0);
        assert_eq!(s.state, "on");
        assert_eq!(s.progress, 0.0);
        assert_eq!(s.remain_sec, 27_000);
    }

    /// 午休窗口内：状态是「午休中」，金额冻在 3 小时那一格，倒计时给的是午休剩余。
    #[test]
    fn lunch_freezes_the_amount_and_counts_down_the_break() {
        let s = snap(at(12, 30));
        assert_eq!(s.state, "break");
        assert_eq!(s.worked_sec, 10_800, "午休 30 分钟，已计薪仍是 3 小时");
        assert_eq!(s.lunch_remain_sec, 3_600);
        // 已计薪秒在午休期间不动：12:00 与 12:30 金额必须一样
        assert_eq!(s.amount_milli, snap(at(12, 0)).amount_milli);
    }

    /// 休息日：一个数字都不给。给 0 会被当成「今天赚了 0 元」，
    /// 给满勤又会虚增 367.82 —— 所以只能标记 rest，让前端整块盖住。
    #[test]
    fn rest_day_gives_no_number() {
        let s = snap(day(14, 10, 0)); // 周六
        assert!(s.rest);
        assert_eq!(s.state, "rest");
        assert_eq!(s.amount_milli, 0);
        assert_eq!(s.shadow_milli, 0);
    }

    /// 过了下班点、没声明加班 → 已下班，金额封顶。
    #[test]
    fn passing_the_end_of_day_marks_it_done() {
        let s = snap(at(19, 5));
        assert_eq!(s.state, "done");
        assert_eq!(s.amount_milli, 367_816, "在岗态金额封顶，不能自己往上涨");
        assert_eq!(s.shadow_milli, 0);
    }

    /// 声明了加班 + 设置是「计费」→ 主账按 1.5× 继续涨，白干为 0。
    #[test]
    fn paid_overtime_grows_the_main_account() {
        let mut s = Settings::default();
        s.ot_mode = OtMode::Paid;
        s.ot_start = Some(at(18, 0).timestamp());

        let snap19 = snap_with(at(19, 0), &s);
        assert_eq!(snap19.state, "ot");
        // 一小时加班 ≈ 49.042 × 1.5 = 73.56 元
        assert!((snap19.amount_milli - (367_816 + 73_561)).abs() < 60, "got {}", snap19.amount_milli);
        assert_eq!(snap19.shadow_milli, 0, "计费加班不该同时记白干");
    }

    /// 声明了加班 + 设置是「不计加班费」→ 主账冻结，白干按 1.5× 累计。
    /// 这是默认设置，也是原型稿里按一下「我在加班」之后的样子。
    #[test]
    fn shadow_overtime_freezes_the_main_account() {
        let mut s = Settings::default();
        assert_eq!(s.ot_mode, OtMode::Shadow, "默认就是不计加班费");
        s.ot_start = Some(at(18, 0).timestamp());

        let snap19 = snap_with(at(19, 0), &s);
        assert_eq!(snap19.state, "unpaid");
        assert_eq!(snap19.amount_milli, 367_816, "主账必须封顶");
        assert_eq!(snap19.shadow_src, "after");
        assert_eq!(snap19.shadow_sec, 3_600);
        assert!((snap19.shadow_milli - 73_561).abs() < 60, "got {}", snap19.shadow_milli);
    }

    /// 昨天点的「我在加班」不能让今天 18:00 自动变成加班。
    #[test]
    fn yesterday_declaration_does_not_leak_into_today() {
        let mut s = Settings::default();
        s.ot_start = Some(day(9, 19, 0).timestamp());
        let snap19 = snap_with(at(19, 0), &s);
        assert_eq!(snap19.state, "done", "今天没声明过加班，就是已下班");
        assert_eq!(snap19.shadow_milli, 0);
    }

    /// 午休加班：主账不动，白干按 1× 计，右槽走「已白干」。
    #[test]
    fn lunch_overtime_records_shadow_at_one_x() {
        let mut s = Settings::default();
        s.lunch_ots = vec![LunchOtRec {
            start: at(12, 0).timestamp(),
            end: None,
            cap_sec: 5_400,
        }];

        let snap = snap_with(at(12, 30), &s);
        assert_eq!(snap.state, "unpaid");
        assert_eq!(snap.shadow_src, "break");
        assert_eq!(snap.shadow_sec, 1_800);
        // 12:00 时已计薪 3 小时（午休那 90 分钟全扣掉）→ 367.816 × 4/9 ≈ 147.126
        assert!((snap.amount_milli - 147_126).abs() < 60, "got {}", snap.amount_milli);
        // 半小时 × 49.042 = 24.52 元，1×
        assert!((snap.shadow_milli - 24_521).abs() < 40, "got {}", snap.shadow_milli);
    }

    /// 午休加班到 13:30 自动收口，之后的时段该计薪就计薪。
    #[test]
    fn lunch_overtime_stops_at_the_end_of_lunch() {
        let mut s = Settings::default();
        s.lunch_ots = vec![LunchOtRec {
            start: at(12, 0).timestamp(),
            end: None,
            cap_sec: 5_400,
        }];

        // 14:00 时午休已过，状态退回在岗，白干封在 90 分钟
        let snap = snap_with(at(14, 0), &s);
        assert_eq!(snap.state, "on");
        assert_eq!(snap.shadow_sec, 5_400, "封顶 90 分钟，不是 2 小时");
        // 14:00 已计薪 3h30m = 12600 秒 → 367.816 × 7/15 ≈ 171.647
        assert!((snap.amount_milli - 171_647).abs() < 60, "got {}", snap.amount_milli);
    }

    /// 结束午休加班之后，已经记下的白干不能清零，长度也不再随时间增长。
    #[test]
    fn closing_lunch_overtime_keeps_the_recorded_shadow() {
        let mut s = Settings::default();
        s.lunch_ots = vec![LunchOtRec {
            start: at(12, 0).timestamp(),
            end: None,
            cap_sec: 5_400,
        }];
        // 12:20 收口
        s.close_lunch_ot(at(12, 20).timestamp());

        let at_noon_end = snap_with(at(13, 30), &s);
        let later = snap_with(at(16, 0), &s);
        assert_eq!(at_noon_end.shadow_sec, 1_200, "只记了 20 分钟");
        assert_eq!(
            at_noon_end.shadow_milli, later.shadow_milli,
            "收口之后长度固定，到下午也不能继续涨"
        );
        assert_eq!(later.state, "on", "结束加班后回到在岗");
    }

    /// 暂停不改变「金额 = f(已计薪秒)」这条性质：暂停期间秒数不涨，
    /// 金额自然冻结，不需要额外的「冻结逻辑」。
    #[test]
    fn pause_freezes_the_amount_without_extra_state() {
        let mut s = Settings::default();
        s.pauses = vec![PauseRec {
            start: at(10, 0).timestamp(),
            end: None,
        }];
        let a = snap_with(at(11, 0), &s);
        assert!(a.paused);
        assert_eq!(a.pause_sec, 3_600, "10:00–11:00 暂停了一小时");
        assert_eq!(a.worked_sec, 3_600, "本来 2 小时，扣掉暂停只算 1 小时");

        let b = snap_with(at(11, 1), &s);
        assert!(b.amount_milli >= a.amount_milli);
        // 同一时刻再算一次，必须一模一样（纯函数的意思）
        assert_eq!(snap_with(at(11, 0), &s).amount_milli, a.amount_milli);
    }

    /// 本月的两个数字要自洽：已赚 = 已过完的工作日 × 日薪 + 今天已赚。
    #[test]
    fn month_numbers_are_consistent() {
        let s = snap(at(12, 0)); // 11/10 是第 7 个工作日，之前 6 个
        assert!((s.amount_milli - 147_126).abs() < 60, "半天 40% → 147.126");
        assert_eq!(
            s.month_earn_milli,
            367_816 * 6 + s.amount_milli,
            "已过完的 6 天按整数天算，今天按实际进度加"
        );
        assert_eq!(s.month_left_days, 21 - 6 - 1, "21 个工作日，去掉已过和今天");
    }

    /// 「记一笔」只能抬本月总额，不能碰日薪、时薪和今天的已赚。
    ///
    /// 这是这一项最容易被做错的地方：顺手加进 `amount_milli` 看着也「对」，
    /// 但那样一笔报销会变成「今天赚的」，进度条跟着冲到 100%，
    /// 而日薪除出来的时薪又没变——两个数字互相打脸。
    #[test]
    fn extra_income_lifts_month_total_but_not_the_rate() {
        let mut s = Settings::default();
        let before = snapshot_at(day(10, 10, 0), &s);

        s.extras.push(settings::ExtraEntry {
            date: "2026-11-10".into(),
            amount_milli: 3_000_000,
        });
        let after = snapshot_at(day(10, 10, 0), &s);

        assert_eq!(
            after.month_earn_milli - before.month_earn_milli,
            3_000_000,
            "本月已到账要涨这一笔"
        );
        assert_eq!(after.amount_milli, before.amount_milli, "今天的已赚不认它");
        assert_eq!(after.day_milli, before.day_milli, "日薪不认它");
        assert_eq!(after.hourly_milli, before.hourly_milli, "时薪不认它");
        assert_eq!(after.progress, before.progress, "进度条不认它");

        // 别的月份那一笔不该在今天被看见
        s.extras[0].date = "2026-12-10".into();
        let other = snapshot_at(day(10, 10, 0), &s);
        assert_eq!(other.month_earn_milli, before.month_earn_milli);
    }

    /// 界面上的三个静态数值必须来自 Params，不能在外壳里写死。
    #[test]
    fn rates_come_from_the_engine() {
        let s = snap(at(10, 0));
        assert_eq!(s.day_milli, 367_816);
        assert_eq!(s.hourly_milli, 49_042);
        assert_eq!(s.per_sec_micro, 13_622);

        let mut cfg = Settings::default();
        cfg.month_milli = 16_000_000;
        let doubled = snap_with(at(10, 0), &cfg);
        assert_eq!(doubled.day_milli, 735_632, "日薪应随月薪翻倍");
        assert_eq!(doubled.hourly_milli, 98_084);
    }

    /// 状态判定：六种状态各走一遍，确保不会互相抢。
    #[test]
    fn resolve_covers_every_state() {
        let s = Settings::default();
        let daily = s.daily_sec;

        assert_eq!(resolve(Phase::Rest, 0, &s, false, false, false).ui, UiState::Rest);
        assert_eq!(resolve(Phase::BeforeWork, 0, &s, false, false, false).ui, UiState::On);
        assert_eq!(resolve(Phase::Working, 3_600, &s, false, false, false).ui, UiState::On);
        assert_eq!(resolve(Phase::Lunch, 10_800, &s, false, false, false).ui, UiState::Break);
        assert_eq!(resolve(Phase::Lunch, 10_800, &s, false, true, false).ui, UiState::Unpaid);
        assert_eq!(resolve(Phase::Working, daily, &s, false, false, false).ui, UiState::Done);
        assert_eq!(resolve(Phase::Working, daily + 60, &s, true, false, false).ui, UiState::Unpaid);
        assert_eq!(
            resolve(Phase::Working, daily + 60, &s, true, false, false).engine,
            EngineState::Unpaid
        );

        let mut paid = Settings::default();
        paid.ot_mode = OtMode::Paid;
        let r = resolve(Phase::Working, daily + 60, &paid, true, false, false);
        assert_eq!(r.ui, UiState::Ot);
        assert_eq!(r.engine, EngineState::Ot);
    }

    /// 声明了加班但还没到下班点：仍然是「在岗」，不能提前按加班算。
    #[test]
    fn declared_but_not_yet_past_the_end_stays_on() {
        let mut s = Settings::default();
        s.ot_start = Some(at(15, 0).timestamp());
        let snap = snap_with(at(16, 0), &s);
        assert_eq!(snap.state, "on");
        assert_eq!(snap.shadow_milli, 0);
    }

    /// 天数不能在月初算成负数（工作日的「还剩」用 saturating 减）。
    #[test]
    fn month_left_days_never_goes_negative() {
        // 11/30 是本月最后一个工作日
        let s = snapshot_at(day(30, 10, 0), &Settings::default());
        assert!(s.month_left_days <= 21);
        let d = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
        assert_eq!(Settings::default().calendar().workdays_before(d), 20);
    }

    // ---------------- 请假 ----------------

    fn with_leave(days: &[&str]) -> Settings {
        let mut s = Settings::default();
        s.leave_days = days.iter().map(|d| d.to_string()).collect();
        s.validate();
        s
    }

    /// 请假当天一分钱都不计，而且状态是「请假」而不是「在岗」。
    #[test]
    fn leave_day_pays_nothing_and_shows_leave() {
        let s = with_leave(&["2026-11-10"]);
        let snap = snap_with(at(14, 0), &s);
        assert_eq!(snap.state, "leave");
        assert_eq!(snap.amount_milli, 0, "请假不计薪");
        assert_eq!(snap.worked_sec, 0, "已计薪秒要归零，不能只是不显示");
        assert_eq!(snap.progress, 0.0);
        assert!(snap.leave);
        assert_eq!(snap.leave_kind, "年假");
        assert_eq!(snap.leave_month_days, 1);
        // 没请假的日子不受影响
        assert_eq!(snap_with(at(14, 0), &Settings::default()).state, "on");
    }

    /// 请假**压在休息日之前**：用户在周六点了「今天请假」，得看到「请假」，
    /// 而不是被「今天休息」的遮罩盖住、以为标记没生效。
    #[test]
    fn leave_beats_rest_mask() {
        // 2026-11-14 是周六
        let mut s = with_leave(&["2026-11-14"]);
        s.leave_left_centi = 450;
        let snap = snapshot_at(day(14, 10, 0), &s);
        assert_eq!(snap.state, "leave");
        assert!(!snap.rest, "请假态不该再显示休息日遮罩");
    }

    /// 月合计要扣掉已过去的请假天，否则会凭空多出一天工资。
    #[test]
    fn month_total_discounts_past_leave_days() {
        // 11/10 之前有 6 个工作日（2–6、9），当天请假 → 合计 6 天日薪 + 今天的 0
        let s = with_leave(&["2026-11-10"]);
        let snap = snap_with(at(14, 0), &s);
        assert_eq!(snap.month_earn_milli, 367_816 * 6, "当天请假不计薪");

        // 11/3 请过一天假：过去 6 天里少一天，加上今天的实时进账
        let s = with_leave(&["2026-11-03"]);
        let snap = snap_with(at(14, 0), &s);
        assert_eq!(snap.month_earn_milli, 367_816 * 5 + snap.amount_milli);

        // 同一时刻不请假：6 天 + 今天的实时进账，正好比上面多一天
        let plain = snap_with(at(14, 0), &Settings::default());
        assert_eq!(
            plain.month_earn_milli - snap.month_earn_milli,
            367_816,
            "差的就是被请假扣掉的那一天"
        );
    }

    /// 未来的请假要从「本月还剩几个工作日」里扣掉：那天不会上班，列在「还剩」里是虚的。
    #[test]
    fn future_leave_reduces_remaining_workdays() {
        let base = snap_with(at(10, 0), &Settings::default()).month_left_days;
        let s = with_leave(&["2026-11-25"]);
        assert_eq!(snap_with(at(10, 0), &s).month_left_days, base - 1);
    }

    /// 标在周六的请假不该扣两次：那天本来就不计薪。
    #[test]
    fn leave_on_weekend_is_not_double_counted() {
        let s = with_leave(&["2026-11-14"]); // 周六
        let snap = snapshot_at(day(10, 10, 0), &s);
        assert_eq!(snap.leave_month_days, 0, "周末不计入请假天数");
        assert_eq!(
            snap.month_earn_milli,
            snapshot_at(day(10, 10, 0), &Settings::default()).month_earn_milli,
            "周末请假不该动月合计"
        );
    }

    /// 请假日期必须是「日期」，不能只是「今天」——跨天后自动失效。
    #[test]
    fn leave_expires_the_next_day() {
        let s = with_leave(&["2026-11-09"]); // 昨天
        assert_eq!(snap_with(at(10, 0), &s).state, "on");
    }

    /// 展开态的折合杯数：保留一位小数，**四舍五入**而不是截断。
    /// 183.908 ÷ 25 = 7.356 → 7.4；截断会给出 7.3，
    /// 那种「少半杯」的误差在钱上是能看出来的。
    #[test]
    fn equivalent_cups_round_to_one_decimal() {
        let s = snap(at(14, 15));
        assert!((s.amount_milli - 183_908).abs() <= 1);
        assert_eq!(s.eq_cups_x10, 74, "7.356 要进位到 7.4");
        assert_eq!(s.eq_name, "奶茶");
        assert_eq!(s.eq_goal_cups, 1_000);
        assert_eq!(s.eq_day_cups, 12);

        // 单价换了，杯数要跟着变：183.908 ÷ 50 = 3.678 → 3.7
        let mut cfg = Settings::default();
        cfg.eq_price_centi = 5_000;
        assert_eq!(snap_with(at(14, 15), &cfg).eq_cups_x10, 37);
    }

    /// 「距下班还能买」＝ 今天还没赚到的那部分。下班之后它就是 0——
    /// 日薪已经封顶了，那时候显示「还能买 3 杯」是骗人的。
    #[test]
    fn equivalent_remaind_goes_to_zero_after_hours() {
        let s = snap(at(19, 0));
        assert_eq!(s.state, "done");
        assert_eq!(s.eq_left_cups_x10, 0, "下班了就没有「还能买」了");

        // 中午 12:00：已赚 147.126，还剩 220.690 → ÷25 = 8.83 → 8.8 杯
        let noon = snap(at(12, 0));
        assert_eq!(noon.eq_left_cups_x10, 88);
    }

    /// 两条小进度条都要夹在 100%：满勤 14.7 杯已经超过「今日目标 12 杯」，
    /// 不夹的话进度条会画出格子外面。
    #[test]
    fn equivalent_bars_are_clamped() {
        let s = snap(at(18, 0));
        assert!(s.eq_day_pct <= 1.0);
        assert!((s.eq_day_pct - 1.0).abs() < 1e-9, "14.7 杯 / 12 杯 要封顶");
        // 累计目标 1000 杯，14.7 杯只走了 1.47%
        assert!(s.eq_goal_pct < 0.02);
    }

    /// 尺寸档位要跟着配置走，前端靠这个字段决定套哪套版式。
    #[test]
    fn size_reaches_the_snapshot() {
        let mut cfg = Settings::default();
        assert_eq!(snap_with(at(10, 0), &cfg).size, "normal");

        cfg.size = Size::Compact;
        assert_eq!(snap_with(at(10, 0), &cfg).size, "compact");

        cfg.size = Size::Expanded;
        assert_eq!(snap_with(at(10, 0), &cfg).size, "expanded");
    }

    // ---------------- 空闲检测：那笔「钱再也不涨」的旧账 ----------------

    /// 走开够久就该开一段暂停。
    #[test]
    fn getting_idle_opens_a_pause() {
        assert_eq!(idle_step(true, false, 5 * 60, 5 * 60, false), IdleStep::Pause);
        // 阈值差一秒不算
        assert_eq!(idle_step(true, false, 5 * 60 - 1, 5 * 60, false), IdleStep::Nothing);
    }

    /// **回归测试**：暂停是我们自己开的，下一帧 `paused` 必然是 true，
    /// 但那条路不能挡住「人回来了」。
    ///
    /// 旧实现在这里返回 `Nothing`（因为外层拿 `!snap.paused` 当门闩），
    /// 于是暂停永远收不回来，金额从离开那一刻起冻住——用户报的
    /// 「在岗但今日已赚不计算」就是它。
    #[test]
    fn coming_back_resumes_even_while_we_are_the_one_holding_the_pause() {
        assert_eq!(idle_step(true, true, 0, 5 * 60, true), IdleStep::Resume);
        assert_eq!(idle_step(true, true, 4, 5 * 60, true), IdleStep::Resume);
    }

    /// 人还没回来：保持暂停，别反复开关（反复开关会让扣减量来回跳）。
    #[test]
    fn still_away_keeps_the_pause_quietly() {
        assert_eq!(idle_step(true, true, 600, 5 * 60, true), IdleStep::Nothing);
    }

    /// 用户自己按的暂停，空闲检测不去碰它——这时候 `open = false`。
    #[test]
    fn users_own_pause_is_left_alone() {
        assert_eq!(idle_step(true, false, 3600, 5 * 60, true), IdleStep::Nothing);
    }

    /// 关掉空闲检测：什么都不做。
    #[test]
    fn disabled_idle_detection_does_nothing() {
        assert_eq!(idle_step(false, false, 3600, 5 * 60, false), IdleStep::Nothing);
        assert_eq!(idle_step(false, true, 3600, 5 * 60, false), IdleStep::Nothing);
    }

    /// 强杀之后留在配置里的悬空暂停，必须在启动时收掉。
    ///
    /// 这是「在岗但不涨钱」的第二条来路：内存里的 `idle_open` 一重启就归零，
    /// 而落盘的 `end: None` 还在，于是 `idle_step` 永远只会返回 `Nothing`。
    #[test]
    fn a_pause_left_open_by_a_killed_run_is_dropped_at_startup() {
        let mut cfg = Settings::default();
        cfg.pauses.push(PauseRec {
            start: 1000,
            end: Some(2000),
        });
        // 悬空那段只可能是最后一段（`sanitize` 只允许最后一段没收口）
        cfg.pauses.push(PauseRec {
            start: 3000,
            end: None,
        });
        assert!(is_paused(&cfg.spans()), "前提：这份配置读进来就是暂停中");

        assert_eq!(drop_dangling_pauses(&mut cfg), 1, "只收没收口的那段");
        assert_eq!(cfg.pauses.len(), 1);
        assert_eq!(cfg.pauses[0].start, 1000, "收过口的那段原样留着");
        assert!(
            !is_paused(&cfg.spans()),
            "收掉之后不能再算暂停中，否则钱还是不动"
        );
    }

    /// 没有悬空暂停时不能误伤：正常收过口的区间一段都不许少。
    #[test]
    fn startup_keeps_every_closed_pause() {
        let mut cfg = Settings::default();
        cfg.pauses.push(PauseRec {
            start: 1000,
            end: Some(2000),
        });
        cfg.pauses.push(PauseRec {
            start: 3000,
            end: Some(4000),
        });
        assert_eq!(drop_dangling_pauses(&mut cfg), 0);
        assert_eq!(cfg.pauses.len(), 2);
    }

    /// 用户报的原话：下班时间设成 1 分钟后，钱不涨，通知也不弹。
    ///
    /// 那时候日工时还是 7.5 小时、班次只剩 1 分钟，`done` 永远等不到。
    /// 通知必须跟着「班次到点」走。
    #[test]
    fn the_end_notice_fires_when_the_shift_ends_even_if_the_day_is_not_full() {
        // 12:34 到点、只干了 3 小时半、日工时 7.5 小时
        assert!(end_notice_due("on", true, 12_840, false), "到点就弹");
        assert!(
            !end_notice_due("on", false, 12_840, false),
            "没到点不弹，哪怕只差一分钟"
        );
        // 钟没到点但日工时攒满了（提前干完的班）——老规矩还得留着
        assert!(end_notice_due("done", false, 27_000, false));
        // 弹过一次就不再弹
        assert!(!end_notice_due("done", true, 27_000, true));
    }

    /// 吸附：靠近就贴，**越过去了也要拉回来贴住**。
    ///
    /// 这条测试来自用户真机：「拖动最下方或者最后方，只留出一点在桌面上，
    /// 没有自动调整贴住边缘」。当时是 `(right - (pos + len)).abs() <= th`，
    /// 越出去越多、绝对值越大，越吸不住。
    #[test]
    fn snapping_pulls_a_window_back_from_past_the_edge() {
        // 工作区 0..2304 × 0..1392（底部 48 是任务栏），窗口 302×220，阈值 24
        let (lo, hi, th, len) = (0, 2304, 24, 302);

        // 中间：不动
        assert_eq!(snap_axis(1000, len, lo, hi, th, true, true), None);
        // 靠近右边 10px：贴上去
        assert_eq!(snap_axis(hi - len - 10, len, lo, hi, th, true, true), Some(hi - len));
        // 已经越出右边 150px：还能拉回来（这就是用户那一下）
        assert_eq!(snap_axis(hi - 150, len, lo, hi, th, true, true), Some(hi - len));
        // 越出左边 200px：拉回 0
        assert_eq!(snap_axis(-200, len, lo, hi, th, true, true), Some(lo));
        // 贴着左边 5px：贴到 0
        assert_eq!(snap_axis(5, len, lo, hi, th, true, true), Some(lo));
    }

    /// 双屏：越出去那边还有屏，就不许拉回来，否则永远挪不到第二块屏。
    #[test]
    fn snapping_leaves_a_window_alone_when_it_crosses_into_another_monitor() {
        let (lo, hi, th, len) = (0, 2304, 24, 302);
        // 右边连着第二块屏
        assert_eq!(snap_axis(hi - 150, len, lo, hi, th, true, false), None);
        // 左边连着第二块屏
        assert_eq!(snap_axis(-200, len, lo, hi, th, false, true), None);
    }

    /// 窗口在阈值内但还没越界时，即使那边有别的屏也照样贴——
    /// 它还在自己这块屏里，贴边不会挡住跨屏。
    #[test]
    fn snapping_still_works_inside_the_monitor_with_a_neighbour() {
        let (lo, hi, th, len) = (0, 2304, 24, 302);
        assert_eq!(snap_axis(hi - len - 8, len, lo, hi, th, true, false), Some(hi - len));
        assert_eq!(snap_axis(8, len, lo, hi, th, false, true), Some(lo));
    }

    /// 上下轴：底边是工作区底边（任务栏之上），不是屏幕底边。
    #[test]
    fn snapping_uses_the_work_area_not_the_screen() {
        // 工作区 0..1392（屏幕 1440，任务栏 48），窗口高 220
        let (top, bottom, th, h) = (0, 1392, 24, 220);
        assert_eq!(snap_axis(1305, h, top, bottom, th, true, true), Some(bottom - h));
        assert_eq!(snap_axis(bottom - h - 10, h, top, bottom, th, true, true), Some(bottom - h));
        assert_eq!(snap_axis(600, h, top, bottom, th, true, true), None);
    }

    /// 吸附要等松手（原型稿 1375 行：松手 **200ms 后**才吸）。
    ///
    /// 这条测试钉的是「吸一次就停」：只在静默时读标记的话，之后每 200ms
    /// 都会再吸一遍，停不下来——拖动过程中系统握着窗口，我们从旁边插一脚
    /// 正是残影的来源。
    #[test]
    fn the_snap_debounce_fires_once_per_drag_gesture() {
        assert_eq!(
            SNAP_DELAY.as_millis(),
            200,
            "原型稿写的是松手 200ms，别随手改这个数"
        );

        let mut d = SnapDebounce::default();
        assert!(!d.take(), "还没动过，不该吸");

        d.note_move();
        assert!(d.take(), "动过之后该吸一次");
        assert!(!d.take(), "吸完就该安静下来，不能再吸第二遍");

        // 拖动中位移是几十次每秒：攒下来也只算一次待办
        for _ in 0..40 {
            d.note_move();
        }
        assert!(d.take());
        assert!(!d.take(), "一串位移只落一次吸附");
    }

    /// 夜班的班次外空档不能误弹「今日已到账 ¥0.00」。
    #[test]
    fn a_night_shift_morning_gap_does_not_fire_a_zero_notice() {
        // 22:00 上班、06:00 下班，早上 09:00 打开程序：不在班里，但今天还没干活
        assert!(!end_notice_due("rest", true, 0, false));
        assert!(!end_notice_due("on", true, 0, false));
        // 06:00 刚下班，今天凌晨那段是干过的
        assert!(end_notice_due("on", true, 21_600, false));
    }

    /// 时区判断的边界：2 小时是「问不问」的分水岭，方向两边都要对。
    ///
    /// 这条边界错一边的后果不一样：
    ///   问太勤（把 1 小时也算进来）——夏令时地区一年被弹两次，用户会去关掉它；
    ///   问太懒（把 2 小时漏掉）——在东京按北京的钟算了一整天，账是错的还看不出来。
    #[test]
    fn timezone_asks_only_past_two_hours() {
        let east8 = 8 * 3600;
        // 东飞 2 小时（北京 → 东京是 1 小时，这里按 2 小时边界测）
        assert_eq!(tz_decide(Some(east8), east8 + 7200, false), TzAct::Ask);
        // 西飞 2 小时
        assert_eq!(tz_decide(Some(east8), east8 - 7200, false), TzAct::Ask);

        // 差 1 小时（夏令时）：静默跟，不打扰
        assert_eq!(
            tz_decide(Some(east8), east8 + 3600, false),
            TzAct::Remember
        );
        // 差 1 小时 59 分：还不够
        assert_eq!(
            tz_decide(Some(east8), east8 + 7140, false),
            TzAct::Remember
        );

        // 没变：什么都不做（不能每拍都写一次盘）
        assert_eq!(tz_decide(Some(east8), east8, false), TzAct::Nothing);

        // 首次运行：只记不问
        assert_eq!(tz_decide(None, east8, false), TzAct::Remember);

        // 已经固定 UTC+8：本地时区怎么变都不问，那一档不读本地钟
        assert_eq!(
            tz_decide(Some(east8), 9 * 3600, true),
            TzAct::Remember
        );
    }

    /// 托盘提示：正常给钱，剩下三种情况只说状态。
    ///
    /// 「隐藏金额」是给开会投屏用的。托盘从背后把数字漏出来，等于开关是假的——
    /// 而这恰恰是用户最不能容忍的一种「没生效」。
    #[test]
    fn tray_hint_never_leaks_the_amount_when_hidden() {
        let s = Settings::default();
        let normal = snap_with(at(10, 0), &s);
        assert_eq!(normal.state, "on", "10:00 应该还在岗");
        let hint = tray_hint(&normal);
        assert!(
            hint.starts_with("牛马薪动了！ 今日已赚 ¥"),
            "正常状态要给钱，实际是 {hint}"
        );
        // mac 菜单栏那一路：有数就是钱，不带品牌词
        assert!(tray_title(&normal).starts_with('¥'));

        let mut hidden = Settings::default();
        hidden.hide_money = true;
        assert_eq!(tray_hint(&snap_with(at(10, 0), &hidden)), "牛马薪动了！");
        assert_eq!(tray_title(&snap_with(at(10, 0), &hidden)), "¥");

        // 休息日：不给数字
        let mut weekend = Settings::default();
        weekend.week.days = [false; 7];
        let r = snap_with(at(10, 0), &weekend);
        assert!(r.rest, "全周不上班应该是休息态");
        assert_eq!(tray_hint(&r), "牛马薪动了！ 今天休息");
        // 休息时菜单栏退成一个 `¥`：位置不能空着，否则菜单栏会跳
        assert_eq!(tray_title(&r), "¥");
    }
}
