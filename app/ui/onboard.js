/* 首次引导的前端。
 *
 * ## 为什么这里破例做算术
 *
 * settings.js 的头一条规矩是「这里不做算术，全由 Rust 算好送过来」。这个文件是例外，
 * 而且是有理由的例外：**第 1 步不能写盘**。写盘会触发 `apply_settings` 末尾的
 * `take_fresh()`，那一刻悬浮窗就露脸了——而用户这时可能才刚看到月薪输入框。
 * 不写盘就拿不到 Rust 算好的视图，所以「日薪 / 时薪 / 每秒」只能在本地按同一个
 * 公式算一遍：
 *
 *     日薪 = 月薪 ÷ 计薪天数          （计薪天数默认 21.75）
 *     时薪 = 日薪 ÷ 每日计薪工时       （工时由第 2 步的作息推导）
 *     每秒 = 时薪 ÷ 3600
 *
 * 第 4 步汇总卡用的是同一组数。真正的落盘值是第 4 步 `apply_settings` 之后由
 * Rust 重新算的，所以这里算错顶多让引导页上的数字偏一点，不会写进配置。
 *
 * ## 唯一的写盘时机
 *
 * 第 4 步「开始计时」。前面三步所有值只活在 `draft` 里。关掉窗口 = 没写盘 =
 * 下次启动再来一次——数据没落盘就等于没设置过（README「首启：先填数，再露脸」）。
 */
(function () {
  "use strict";

  document.addEventListener("contextmenu", function (e) { e.preventDefault(); }, true);

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;

  var el = {};
  [
    "barTitle", "steps", "closeBtn",
    "fMonth", "basisChips", "cDay", "cHour", "cSec", "next1",
    "weekChips", "fStart", "fEnd", "fL1", "fL2", "spanHint", "prev2", "next2",
    "desk", "hudMini", "sizeChips", "swAuto", "prev3", "next3",
    "sDay", "sHour", "sSpan", "sDays", "finish"
  ].forEach(function (id) { el[id] = document.getElementById(id); });

  var panes = Array.prototype.slice.call(document.querySelectorAll(".pane"));
  var dots = Array.prototype.slice.call(el.steps.querySelectorAll("i"));

  var payDays = 21.75;

  /* 四步共用的草稿。不写盘，直到第 4 步。 */
  var draft = {
    month: 8000,
    gross: true,
    form: "five",
    flex: false,
    start: "09:00",
    end: "18:00",
    l1: "12:00",
    l2: "13:30",
    size: "standard",
    autostart: true,
    // 初始位置：靠右、垂直居中。原型里那张卡就在这个位置——虽然第 4 步文案
    // 说「在桌面右下角待命」，但那说的是真实窗口的默认角，不是这块示意区里的排版。
    relX: 1,
    relY: 0.5
  };

  /* ---------- 小工具 ---------- */

  function clamp(v, lo, hi) { return v < lo ? lo : (v > hi ? hi : v); }

  function money(v, d) {
    if (!isFinite(v)) v = 0;
    return "¥" + v.toFixed(d);
  }

  function num(v) {
    var n = parseFloat(v);
    return isFinite(n) ? n : 0;
  }

  /** `"09:30"` → 570。认不出来返回 null，绝不把坏值当 0 用。 */
  function mins(s) {
    var m = /^(\d{1,2}):(\d{2})$/.exec(String(s == null ? "" : s).trim());
    if (!m) return null;
    var h = +m[1], mi = +m[2];
    if (h > 23 || mi > 59) return null;
    return h * 60 + mi;
  }

  /** 计薪工时（小时）= 在岗时长 − 午休。跨零点按 +24h 处理。 */
  function spanHours() {
    var a = mins(draft.start), b = mins(draft.end);
    if (a === null || b === null) return 0;
    var span = b - a;
    if (span <= 0) span += 1440;
    var x = mins(draft.l1), y = mins(draft.l2), lunch = 0;
    if (x !== null && y !== null) {
      lunch = y - x;
      if (lunch < 0) lunch += 1440;
    }
    var h = (span - lunch) / 60;
    return h > 0 ? h : 0;
  }

  function hm(m) {
    if (m === null) return "";
    return pad2(Math.floor(m / 60)) + ":" + pad2(m % 60);
  }
  function pad2(n) { return (n < 10 ? "0" : "") + n; }

  /* ---------- 实时折算 ---------- */

  function calc() {
    var perDay = payDays > 0 ? draft.month / payDays : 0;
    var hours = spanHours();
    var perHour = hours > 0 ? perDay / hours : 0;
    var perSec = perHour / 3600;

    el.cDay.textContent = money(perDay, 2);
    el.cHour.textContent = money(perHour, 2);
    el.cSec.textContent = money(perSec, 4);

    var a = mins(draft.start), b = mins(draft.end), x = mins(draft.l1), y = mins(draft.l2);
    var span = "";
    if (a !== null && b !== null) {
      span = hm(a) + "–" + hm(b);
      if (x !== null && y !== null) {
        span += " 扣除 " + ((y - x + 1440) % 1440) / 60 + "h 午休";
      }
      span += " = ";
    }
    el.spanHint.innerHTML = "计薪工时 = " + span + "<b>" + hours.toFixed(1) + " 小时</b>";

    return { perDay: perDay, perHour: perHour, hours: hours };
  }

  /* ---------- 步骤切换 ---------- */

  function go(n) {
    var s = clamp(n, 1, 4);
    panes.forEach(function (p) { p.classList.toggle("on", +p.getAttribute("data-s") === s); });
    dots.forEach(function (d, i) { d.classList.toggle("on", i < s); });
    el.barTitle.textContent = "牛马薪动了 · 第 " + s + " 步 / 共 4 步";
    if (s === 3) placeMini();
    if (s === 4) summary();
    if (s === 2) calc();
    if (s === 1) setTimeout(function () { el.fMonth.focus(); el.fMonth.select(); }, 30);
  }

  function summary() {
    var c = calc();
    el.sDay.textContent = money(c.perDay, 2);
    el.sHour.textContent = money(c.perHour, 2);
    el.sSpan.textContent = c.hours.toFixed(1) + " h";
    // 本月工作日：从 Rust 送来的日历格子里数 kind === "work"。
    // 拿不到（浏览器里跑探针、或视图还没回来）就显示「—」，不瞎猜一个数。
    var days = "—";
    if (view && view.week && view.week.cells) {
      var n = 0;
      view.week.cells.forEach(function (c2) { if (c2.kind === "work") n++; });
      if (n > 0) days = n + " 天";
    }
    el.sDays.textContent = days;
  }

  /* ---------- 第 3 步：桌面示意区拖拽 ---------- */

  var PAD = 12;

  /* 用户到底动没动过那个卡片。
     没动过就不传 `win_rel`，让悬浮窗保持默认的右下角（第 4 步那句
     「已经在桌面右下角待命」才不是假话）；动过才把位置写进配置。 */
  var dragged = false;

  function placeMini() {
    if (!el.desk || !el.hudMini) return;
    var dw = el.desk.clientWidth, dh = el.desk.clientHeight;
    var mw = el.hudMini.offsetWidth, mh = el.hudMini.offsetHeight;
    // 可移动范围要减掉卡片自身尺寸，否则 relX=1 时卡片会有一半探出桌面区
    var maxX = Math.max(1, dw - mw - PAD * 2);
    var maxY = Math.max(1, dh - mh - PAD * 2);
    el.hudMini.style.left = (PAD + draft.relX * maxX) + "px";
    el.hudMini.style.top = (PAD + draft.relY * maxY) + "px";
    el.hudMini.style.bottom = "auto";
  }

  (function bindDrag() {
    var on = false, sx = 0, sy = 0, ox = 0, oy = 0;
    el.hudMini.addEventListener("mousedown", function (e) {
      e.preventDefault();
      on = true;
      sx = e.clientX; sy = e.clientY;
      ox = el.hudMini.offsetLeft; oy = el.hudMini.offsetTop;
    });
    window.addEventListener("mousemove", function (e) {
      if (!on) return;
      var dw = el.desk.clientWidth, dh = el.desk.clientHeight;
      var mw = el.hudMini.offsetWidth, mh = el.hudMini.offsetHeight;
      var maxX = Math.max(1, dw - mw - PAD * 2);
      var maxY = Math.max(1, dh - mh - PAD * 2);
      var nx = clamp(ox + e.clientX - sx, PAD, PAD + maxX);
      var ny = clamp(oy + e.clientY - sy, PAD, PAD + maxY);
      el.hudMini.style.left = nx + "px";
      el.hudMini.style.top = ny + "px";
      draft.relX = (nx - PAD) / maxX;
      draft.relY = (ny - PAD) / maxY;
      dragged = true;
    });
    window.addEventListener("mouseup", function () { on = false; });
    // 键盘也能挪：拖拽是鼠标专属动作，但这不是「必须用鼠标」的功能
    el.hudMini.addEventListener("keydown", function (e) {
      var d = 0.05, moved = true;
      if (e.key === "ArrowLeft") draft.relX = clamp(draft.relX - d, 0, 1);
      else if (e.key === "ArrowRight") draft.relX = clamp(draft.relX + d, 0, 1);
      else if (e.key === "ArrowUp") draft.relY = clamp(draft.relY - d, 0, 1);
      else if (e.key === "ArrowDown") draft.relY = clamp(draft.relY + d, 0, 1);
      else moved = false;
      if (moved) { e.preventDefault(); placeMini(); dragged = true; }
    });
  })();

  /* ---------- chips 与开关 ---------- */

  function bindChips(box, attr, onPick) {
    box.addEventListener("click", function (e) {
      var c = e.target.closest(".chip");
      if (!c) return;
      Array.prototype.forEach.call(box.querySelectorAll(".chip"), function (n) {
        n.classList.toggle("on", n === c);
      });
      onPick(c.getAttribute(attr));
    });
  }

  bindChips(el.basisChips, "data-b", function (v) { draft.gross = v === "1"; });

  bindChips(el.weekChips, "data-f", function (v) {
    if (v === "flex") {
      // 「弹性」不是一个新的排班形态，它是叠在现有形态上的一个开关：
      // 哪几天上班还是照原来，变的只是「一天的起点按首次活动算」。
      draft.flex = true;
    } else {
      draft.flex = false;
      draft.form = v;
    }
    calc();
  });

  bindChips(el.sizeChips, "data-z", function (v) { draft.size = v; placeMini(); });

  el.swAuto.addEventListener("click", function () {
    draft.autostart = !draft.autostart;
    el.swAuto.classList.toggle("on", draft.autostart);
    el.swAuto.setAttribute("aria-checked", draft.autostart ? "true" : "false");
  });
  el.swAuto.addEventListener("keydown", function (e) {
    if (e.key === " " || e.key === "Enter") { e.preventDefault(); el.swAuto.click(); }
  });

  /* ---------- 输入 ---------- */

  el.fMonth.addEventListener("input", function () {
    draft.month = num(el.fMonth.value);
    calc();
  });
  // 滚轮在数字框上会悄悄改数值（8000 → 8100），而用户以为自己在滚页面。
  // 失焦即可：焦点离开后滚轮回到「滚页面」的正常语义。
  el.fMonth.addEventListener("wheel", function () { el.fMonth.blur(); }, { passive: true });

  [
    ["fStart", "start"], ["fEnd", "end"], ["fL1", "l1"], ["fL2", "l2"]
  ].forEach(function (pair) {
    var node = el[pair[0]];
    function read() {
      // 只收合法时刻：`9:5`、`25:00` 这种先不写进草稿，等它变成合法值再收。
      // 收坏值的后果是计薪工时算出 0，第 4 步汇总显示 ¥0.00 —— 看着像坏了。
      if (mins(node.value) !== null) {
        draft[pair[1]] = node.value.trim();
        calc();
      }
    }
    node.addEventListener("input", read);
    node.addEventListener("blur", function () {
      if (mins(node.value) === null) {
        node.value = draft[pair[1]];
      } else {
        var m = mins(node.value);
        node.value = hm(m);
        draft[pair[1]] = node.value;
        calc();
      }
    });
  });

  /* ---------- 导航 ---------- */

  el.next1.addEventListener("click", function () { go(2); });
  el.prev2.addEventListener("click", function () { go(1); });
  el.next2.addEventListener("click", function () { go(3); });
  el.prev3.addEventListener("click", function () { go(2); });
  el.next3.addEventListener("click", function () { go(4); });

  function close() {
    if (window.__TAURI__ && window.__TAURI__.window) {
      window.__TAURI__.window.getCurrentWindow().close();
    }
  }

  el.closeBtn.addEventListener("click", close);
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") close();
  });

  /* ---------- 落盘 ---------- */

  var saving = false;

  el.finish.addEventListener("click", function () {
    if (saving) return;
    saving = true;
    el.finish.disabled = true;
    el.finish.textContent = "正在保存…";

    var hours = spanHours();
    var patch = {
      month_yuan: draft.month,
      gross: !!draft.gross,
      // 工时四舍五入到 0.1 小时：输入框是 0.5 步进，但推导出来的可能是 7.3333
      daily_hours: Math.round(hours * 10) / 10,
      sched: {
        start: draft.start,
        end: draft.end,
        lunch1: draft.l1,
        lunch2: draft.l2
      },
      work_form: draft.form,
      flex_start: !!draft.flex,
      size: draft.size,
      autostart: !!draft.autostart
    };

    // 只有在示意区里真拖过，才把位置写进去。
    // 传的是相对坐标（0~1），后端按主屏工作区折算成像素——这块示意区和真实屏幕
    // 比例不一致，前端算像素必然错，而后端手上就有工作区尺寸。
    if (dragged) {
      patch.win_rel = [
        Math.round(draft.relX * 1000) / 1000,
        Math.round(draft.relY * 1000) / 1000
      ];
    }

    if (!invoke) {
      // 浏览器里跑探针：没有 Rust，就只把按钮恢复，别假装保存成功
      saving = false;
      el.finish.disabled = false;
      el.finish.textContent = "开始计时";
      return;
    }

    invoke("apply_settings", { patch: patch })
      .then(function () {
        // 写盘成功 → Rust 侧 `take_fresh()` 已把悬浮窗露脸。
        // 这里只负责把引导窗收掉。
        close();
      })
      .catch(function (e) {
        console.error("引导保存失败", e);
        saving = false;
        el.finish.disabled = false;
        el.finish.textContent = "开始计时";
      });
  });

  /* ---------- 启动：拿现状预填 ---------- */

  var view = null;

  function prefill() {
    el.fMonth.value = draft.month;
    Array.prototype.forEach.call(el.basisChips.querySelectorAll(".chip"), function (n) {
      n.classList.toggle("on", (n.getAttribute("data-b") === "1") === draft.gross);
    });
    var form = draft.form;
    var matched = false;
    Array.prototype.forEach.call(el.weekChips.querySelectorAll(".chip"), function (n) {
      var v = n.getAttribute("data-f");
      var on = draft.flex ? (v === "flex") : (v === form && v !== "flex");
      n.classList.toggle("on", on);
      if (on) matched = true;
    });
    // form 是 custom（老用户自定义过哪几天上班）时四个 chip 都不匹配，
    // 退回「双休」并把它点亮——引导页不该出现「四个都没选」这种状态
    if (!matched) {
      Array.prototype.forEach.call(el.weekChips.querySelectorAll(".chip"), function (n) {
        n.classList.toggle("on", n.getAttribute("data-f") === "five");
      });
      draft.form = "five";
    }
    el.fStart.value = draft.start;
    el.fEnd.value = draft.end;
    el.fL1.value = draft.l1;
    el.fL2.value = draft.l2;
    Array.prototype.forEach.call(el.sizeChips.querySelectorAll(".chip"), function (n) {
      n.classList.toggle("on", n.getAttribute("data-z") === draft.size);
    });
    el.swAuto.classList.toggle("on", draft.autostart);
    el.swAuto.setAttribute("aria-checked", draft.autostart ? "true" : "false");
  }

  function boot() {
    calc();
    if (!invoke) { prefill(); go(1); return; }
    invoke("get_settings")
      .then(function (v) {
        view = v;
        payDays = (v.pay_days_centi || 2175) / 100;
        draft.month = v.month_yuan || 8000;
        draft.gross = !!v.gross;
        if (v.sched) {
          draft.start = v.sched.start || draft.start;
          draft.end = v.sched.end || draft.end;
          draft.l1 = v.sched.lunch1 || draft.l1;
          draft.l2 = v.sched.lunch2 || draft.l2;
        }
        if (v.week) {
          draft.form = v.week.form || "five";
          draft.flex = !!v.week.flex;
        }
        draft.size = v.size || "standard";
        draft.autostart = !!v.autostart;
        prefill();
        calc();
        go(1);
      })
      .catch(function (e) {
        console.error("读设置失败，用默认值继续", e);
        prefill();
        go(1);
      });
  }

  window.addEventListener("resize", placeMini);

  /* 标题栏拖动。同 ask.js / settings.js，实现在 drag.js。 */
  if (window.bindBarDrag) window.bindBarDrag();

  boot();
})();
