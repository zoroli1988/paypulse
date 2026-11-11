/* 设置窗口的假后端（只给截图探针用，不进安装包）。
 *
 * 数字全部抄自原型稿 04 章：月薪 8000、21.75 天、7.5 h → 日薪 367.82 / 时薪 52.01
 * （含年终奖摊分后）/ 每秒 0.014447。
 * 截图和原型能逐项对上，才看得出是「照做」还是「大概是那个意思」。
 *
 * 日历格子在这里生成而不是从后端拿，是因为探针没有后端。**这是唯一允许
 * 在前端算日期的地方**，而且它只存在于 tools/ 里，不进安装包。
 * 固定的月份是 2026-11：1 号是周日（周一起算要空 6 格），11-11 标成假日、
 * 11-21 标成补班，好让三种格子颜色同时出现在一张图上。
 */
(function () {
  "use strict";

  var saved = false;   // 假后端也要有「写过盘了」这个状态
  var LEAD = 6;
  var DAYS = 30;

  function cells() {
    var out = [];
    for (var d = 1; d <= DAYS; d++) {
      var iso = "2026-11-" + (d < 10 ? "0" + d : "" + d);
      var dow = (LEAD + d - 1) % 7;          // 0 = 周一
      var kind = dow >= 5 ? "rest" : "work";
      if (d === 11) kind = "holiday";
      if (d === 21) kind = "makeup";
      out.push({ d: d, iso: iso, kind: kind, today: d === 10 });
    }
    return out;
  }

  var VIEW = {
    month_yuan: 8000,
    pay_days_centi: 2175,
    daily_hours: 7.5,
    ot_paid: false,
    sched: {
      start: "09:00", end: "18:00", lunch1: "12:00", lunch2: "13:30",
      span_h: 9.0, lunch_h: 1.5, paid_h: 7.5, cross_midnight: false, overnight: false
    },
    week: {
      form: "bigsmall",
      days: [true, true, true, true, true, true, false],
      alt_sat: 1,
      month: "2026 年 11 月",
      lead: LEAD,
      cells: cells(),
      holidays: 1,
      rest: 9,
      makeup: 1,
      next_holiday: "2027-01-01 · 还有 52 天"
    },
    bonus: {
      mode: "months", months: 1.0, amount_yuan: 0, rate_pct: 0,
      total_yuan: 8000, per_month_yuan: 666.67, annual_months: 13.0
    },
    gross: true,
    tz_fixed: false,
    theme: "dark",
    num_font: "sf",
    copy_style: "emotional",
    autostart: true,
    silent_start: false,
    snap_px: 24,
    monitor_primary: false,
    idle_detect: true,
    idle_min: 5,
    close_action: "tray",
    hotkey: { enabled: true, show: "Ctrl+Shift+P", passthru: "Ctrl+Shift+L", clock: "Ctrl+Shift+Enter", extra: "Ctrl+Shift+E" },
    notify: {
      end: "amountwork", lunch: true, lunch_before: 10, sit: true, sit_min: 60,
      dnd: true, dnd_from: "19:00", dnd_to: "08:30"
    },
    idle_now_sec: 132,
    accent: "#35E07F",
    opacity: 92,
    size: "normal",
    density: { metrics: true, bar: true, tip: true, shadow: true },
    eq_name: "奶茶",
    eq_price_yuan: 25,
    eq_goal_cups: 1000,
    eq_day_cups: 12,
    leave_kind: "年假",
    leave_left_days: 4.5,
    today_leave: false,
    click_through: false,
    today_cups_x10: 116,
    day_yuan: 367.82,
    hourly_yuan: 52.01,
    per_sec_yuan: 0.014447,
    version: "0.1.0",
    data_dir: "C:\\Users\\zorol\\AppData\\Roaming\\com.bydota.paypulse",
    paused: false
  };

  // ?ot=paid / ?leave=1 / ?size=expanded / ?thru=1 / ?theme=light / ?bonus=none
  // 用来拍同一组里的另一种状态
  var q = new URLSearchParams(location.search);
  if (q.get("ot") === "paid") VIEW.ot_paid = true;
  if (q.get("leave") === "1") VIEW.today_leave = true;
  if (q.get("size")) VIEW.size = q.get("size");
  if (q.get("thru") === "1") VIEW.click_through = true;
  if (q.get("theme")) VIEW.theme = q.get("theme");
  if (q.get("font")) VIEW.num_font = q.get("font");
  if (q.get("copy")) VIEW.copy_style = q.get("copy");
  if (q.get("mm") === "primary") VIEW.monitor_primary = true;
  if (q.get("flex") === "1") VIEW.week.flex = true;
  if (q.get("bonus") === "none") VIEW.bonus.mode = "none";
  if (q.get("form") === "five") {
    VIEW.week.form = "five";
    VIEW.week.days = [true, true, true, true, true, false, false];
    VIEW.week.alt_sat = null;
  }
  if (q.get("density") === "sparse") {
    VIEW.density = { metrics: false, bar: false, tip: false, shadow: false };
  }

  window.__TAURI__ = {
    core: {
      invoke: function (cmd) {
        if (cmd === "get_settings" || cmd === "apply_settings") {
          // 落盘一次之后就不再是「新装机」了，和 Rust 侧 take_fresh 的语义一致
          if (cmd === "apply_settings") saved = true;
          return Promise.resolve(VIEW);
        }
        // ?welcome=1 用来拍「首次启动」那张：引导条只在写盘之前为真
        if (cmd === "first_run") {
          var fresh = new URLSearchParams(location.search).get("welcome") === "1";
          return Promise.resolve(fresh && !saved);
        }
        return Promise.resolve(null);
      }
    },
    window: {
      getCurrentWindow: function () {
        return { close: function () {} };
      }
    }
  };
})();
