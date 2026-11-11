/* 探针专用假后端。
 *
 * 真机上 hud.js 通过 window.__TAURI__.core.invoke 拿快照；这里用一份等价数据
 * 顶上去，只为把平时看不到的状态画出来（午休、加班、白干、暂停……都要么等时间、
 * 要么点按钮，探针里直接给）。
 *
 * 数值是照原型稿反推的，别当真；但**字段名必须和 Rust 的 Snapshot 一致**。
 * 加字段时两边都要改，否则页面读到 undefined，表现是某块文字变成 NaN 或空白。
 *
 * 两个 URL 参数：
 *   ?s=on        状态（九选一，见 SNAP）
 *   ?z=expanded  尺寸档（compact / normal / expanded），决定套哪套版式
 */
(function () {
  "use strict";

  var q = new URLSearchParams(location.search);
  var target = q.get("s") || "on";
  var SIZE = q.get("z") || "normal";
  /* ?accent=%23FF9F45 / ?alpha=70 / ?off=m,t —— 用来验「强调色 / 不透明度 / 信息密度」
     这三项改动是不是真的落到了版面上。 */
  var ACCENT = q.get("accent") || "#35E07F";
  var ALPHA = parseInt(q.get("alpha") || "92", 10);
  var OFF = (q.get("off") || "").split(",").filter(Boolean);

  /* 单价 ¥25（25000 毫），和 Rust 侧 Settings::default() 一致。
     折合杯数的算法也照抄 Rust：放大十倍再四舍五入，保留一位小数。
     这里必须自己算一遍而不是写死：截图是用来核对「版面 + 数字自洽」的，
     写死的杯数配不上随状态变的金额，一眼就能看出是假数据。 */
  var PRICE = 25000;

  /* 和 Rust 的 hex_rgb 同一个作用：给 rgba() 用的三元组。 */
  function hexRgb(hex) {
    return [1, 3, 5].map(function (i) {
      return parseInt(hex.slice(i, i + 2), 16);
    }).join(", ");
  }

  /* 每帧都有的开关，默认全关，个别状态单独打开。 */
  function base(o) {
    var d = Object.assign(
      {
        amount_milli: 0, shadow_milli: 0, shadow_sec: 0, shadow_src: "",
        worked_sec: 0, ot_sec: 0, remain_sec: 27000, lunch_remain_sec: 0,
        progress: 0,
        day_milli: 367816, hourly_milli: 49042, per_sec_micro: 13622,
        month_earn_milli: 2495864, month_left_days: 14,
        state: "on", rest: false, paused: false, pause_sec: 0,
        hide_money: false, click_through: false,
        accent: "#35E07F", accent_rgb: "53, 224, 127", opacity: 92,
        density_metrics: true, density_bar: true, density_tip: true, density_shadow: true,
        leave: false, leave_kind: "年假", leave_left_centi: 450, leave_month_days: 1
      },
      o || {}
    );

    var cups = Math.round(d.amount_milli * 10 / PRICE);
    var left = Math.round(Math.max(0, d.day_milli - d.amount_milli) * 10 / PRICE);

    d.size = SIZE;
    d.accent = ACCENT;
    d.accent_rgb = hexRgb(ACCENT);
    d.opacity = ALPHA;
    d.density_metrics = OFF.indexOf("m") < 0;
    d.density_bar = OFF.indexOf("b") < 0;
    d.density_tip = OFF.indexOf("t") < 0;
    d.density_shadow = OFF.indexOf("s") < 0;
    d.eq_name = "奶茶";
    d.eq_cups_x10 = cups;
    d.eq_left_cups_x10 = left;
    d.eq_goal_cups = 1000;
    d.eq_day_cups = 12;
    d.eq_goal_pct = Math.min(1, cups / 10000);
    d.eq_day_pct = Math.min(1, cups / 120);
    /* 主题 / 字体 / 文案风格：三个都只是 dataset，截图用来验「换过去长什么样」。
       ?theme=light / ?font=jb / ?copy=plain —— 不传就是默认那档。 */
    d.theme = q.get("theme") || "dark";
    d.num_font = q.get("font") || "sf";
    d.copy_style = q.get("copy") || "emotional";
    return d;
  }

  /* 05:53:32 = 21212 秒 → 288.968 元（原型稿截图上的那个数） */
  var ON = base({
    amount_milli: 288968, worked_sec: 21212, remain_sec: 5788,
    progress: 21212 / 27000, state: "on"
  });

  var SNAP = {
    on: ON,

    /* 午休中：已计薪冻在 3 小时，右槽换成「午休剩余」，底部长出「我在加班」 */
    break: base({
      amount_milli: 147126, worked_sec: 10800, remain_sec: 16200,
      lunch_remain_sec: 3600, progress: 0.4, state: "break"
    }),

    /* 计费加班：主色转蓝，主账继续涨 */
    ot: base({
      amount_milli: 443667, worked_sec: 30712, ot_sec: 3712, remain_sec: 0,
      progress: 1, state: "ot"
    }),

    /* 义务加班（下班后）：主账封顶，白干条出现 */
    unpaid: base({
      amount_milli: 367816, worked_sec: 30712, ot_sec: 3712, remain_sec: 0,
      progress: 1, state: "unpaid",
      shadow_milli: 75851, shadow_sec: 3712, shadow_src: "after"
    }),

    /* 午休加班：主账不动，白干按 1×，右槽「已白干」 */
    unpaid_break: base({
      amount_milli: 147126, worked_sec: 10800, remain_sec: 16200,
      lunch_remain_sec: 3600, progress: 0.4, state: "unpaid",
      shadow_milli: 24521, shadow_sec: 1800, shadow_src: "break"
    }),

    /* 过了下班点没点加班：金额封顶，右槽「已过」 */
    done: base({
      amount_milli: 367816, worked_sec: 30712, ot_sec: 3712, remain_sec: 0,
      progress: 1, state: "done"
    }),

    /* 托盘开了「暂停计时」：金额灰掉，标签变「已暂停」 */
    paused: base({
      amount_milli: 183908, worked_sec: 13500, remain_sec: 13500,
      progress: 0.5, state: "on", paused: true, pause_sec: 754
    }),

    /* 托盘开了「隐藏金额」：数字糊掉，进度条照旧 */
    masked: base({
      amount_milli: 183908, worked_sec: 13500, remain_sec: 13500,
      progress: 0.5, state: "on", hide_money: true
    }),

    /* 周六：整块盖住，一个数字都不给 */
    rest: base({ state: "rest", rest: true, remain_sec: 27000 }),

    /* 请假：不计薪，主色转紫，三列换成 类型 / 剩余 / 本月 */
    leave: base({
      amount_milli: 0, worked_sec: 0, remain_sec: 0, progress: 0, state: "leave",
      leave: true, leave_kind: "年假", leave_left_centi: 450, leave_month_days: 1,
      month_earn_milli: 2206 * 1000 + 900
    })
  };

  /* 既记最大值也记最后一次：白干行出现会让窗口变高，而 --dump-dom
     可能在第二次回调前就返回，所以按最大值报。宽度则按最后一次报
     （三档宽度是下面定死的，只有切档才会变）。 */
  var maxH = 0;

  window.__TAURI__ = {
    core: {
      invoke: function (cmd, args) {
        if (cmd === "tick" || cmd === "cycle_size" || cmd === "act") {
          return Promise.resolve(SNAP[target] || ON);
        }
        if (cmd === "resize_hud") {
          /* 借 title 把量到的宽高带出来，shot.py 用 --dump-dom 读它。 */
          var h = Math.round(args && args.height);
          var w = Math.round(args && args.width);
          if (h > maxH) maxH = h;
          document.title = "hud-w=" + w + " h=" + maxH;
          return Promise.resolve(null);
        }
        return Promise.resolve(null);
      }
    }
  };
})();
