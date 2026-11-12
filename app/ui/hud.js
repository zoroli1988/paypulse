/* 悬浮窗前端。
 *
 * 这个文件**一行金额算术都没有**：所有数字由 Rust 算好，这里只做三件事——
 *   1. 把整数（毫 / 秒）格式化成字符串；
 *   2. 按状态从固定表里取文案（标签、提示、按钮）；
 *   3. 内容高度变了就通知 Rust 把窗口改高。
 *
 * 「文案表放前端、数字放后端」是刻意的：文案要改词、要本地化，改这里就够；
 * 而金额、进度、白干、本月合计这些，错一个数字就是错账，必须在 Rust 里被测试盖住。
 */
(function () {
  "use strict";

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  var el = {};
  [
    "hud", "stateLabel", "amount", "perSec", "hourly", "daily", "worked", "pct",
    "remainLabel", "remain", "barFill", "monthEarn", "monthLeft", "shadowRow",
    "shadowAmt", "tip", "actStart", "actStartTxt", "actStop", "actStopTxt",
    "thruBtn", "restMask", "k1", "k2", "k3",
    "eqName", "eqCups", "eqGoal", "eqLeft", "eqDay", "eqGoalBar", "eqDayBar"
  ].forEach(function (id) { el[id] = document.getElementById(id); });

  /* 三档尺寸的 class 名。顺序即循环顺序（原型 01-D「缩放」）。 */
  var SIZES = ["size-compact", "size-normal", "size-expanded"];

  /* 主题：`system` 不是一个能画出来的皮肤，得先在本地解析成 dark / light。
     这是「显示哪一套颜色」，不是「算多少钱」，所以放在这里而不是 Rust——
     系统的深浅色只有浏览器知道，后端每次 tick 去问它反而更绕。
     `change` 事件也接上：用户在系统设置里切了深色，悬浮窗当场跟着变，
     不用重启。 */
  var darkMQ = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  /* null 而不是 "dark"：第一帧必须无条件走一次 paintTheme，
     否则配置里本来就是深色时 dataset.theme 永远不被写上。 */
  var wantTheme = null;

  function paintTheme() {
    var t = wantTheme;
    if (t === "system") t = !darkMQ || darkMQ.matches ? "dark" : "light";
    el.hud.dataset.theme = t;
  }

  if (darkMQ) {
    var onScheme = function () { paintTheme(); };
    if (darkMQ.addEventListener) darkMQ.addEventListener("change", onScheme);
    else if (darkMQ.addListener) darkMQ.addListener(onScheme);
  }


  /* ---- 文案：三套，按「文案风格」切 ----
   *
   * 情绪向那套逐句抄自原型稿的 tipOf()：阈值属于「显示哪句话」，不属于
   * 「算多少钱」，所以它留在这边。另外两套是同一件事换个说法——
   * 严谨只陈述事实，静默干脆不说话（tip 整行在 CSS 里按 [data-copy="silent"] 隐藏）。
   *
   * 三张表的键必须**逐一对齐**：少一个键就会掉进 `|| labels.on` 那个兜底，
   * 而兜底文案冒到静默档里，正是这个功能最容易出的丑。
   */
  var COPY = {
    emotional: {
      labels: {
        on: "在岗", break: "午休中", ot: "加班中",
        unpaid: "义务加班", leave: "请假", done: "已下班", paused: "已暂停"
      },
      tip: function (d, s) {
        if (d.paused) return "暂停中，先忙你的";
        if (s === "break") {
          return d.lunch_remain_sec > 0 ? "吃饱再算，钱又不会跑" : "午休结束了，该上工了";
        }
        if (s === "leave") return "不赚钱的日子，也是日子";
        if (s === "done") return "今日收工，明天见";
        if (s === "ot") return "今天这钱，是拿命换的";
        if (s === "unpaid") {
          if (d.shadow_src === "break") {
            if (d.lunch_remain_sec <= 60) return "午休到点了，马上切回在岗";
            if (d.shadow_sec < 300) return "午饭还没吃，先垫一口";
            if (d.shadow_sec < 1800) return "午休本来就没加班费，只能自己记";
            if (d.shadow_sec < 3600) return "午休没了，钱也没多一分";
            return "中午不歇，下午是要还的";
          }
          if (d.shadow_sec < 600) return "刚过点，现在还来得及走";
          if (d.shadow_sec < 1800) return "这钱公司不给，但账得记着";
          if (d.shadow_sec < 3600) return "白干半小时，够买一顿饭";
          return "再坐下去，白干的就不是钱了";
        }
        /* 收尾 / 最后一段按「距下班还有多久」切，不按进度百分比切。
           规格 02-B 写死了「收尾 = 距下班 < 60 分钟」，而百分比在不同日工时下
           对应的剩余时间并不一样：9 小时排班的 90% 是 54 分钟，6 小时排班
           只有 36 分钟——调过工时的人会听到「收尾阶段」时天还没黑。
           所以先按剩余时间判完这两档，剩下的才轮到进度分带。 */
        if (d.remain_sec <= 600) return "最后一段，撑住";
        if (d.remain_sec <= 3600) return "收尾阶段，别硬熬";
        var r = d.progress;
        if (r < 0.05) return "打卡成功，先稳住";
        if (r < 0.20) return "开局不错，钱在流";
        if (r < 0.40) return "稳住，别急着看表";
        if (r < 0.60) return "过半了，够买份午饭";
        return "手别停，钱别停";
      }
    },

    /* 严谨：一句话说清现在是什么时段、计不计薪，不调侃、不催。 */
    plain: {
      labels: {
        on: "工作时段", break: "午休", ot: "加班", 
        unpaid: "无薪加班", leave: "请假", done: "已下班", paused: "已暂停"
      },
      tip: function (d, s) {
        if (d.paused) return "计时已暂停";
        if (s === "break") {
          return d.lunch_remain_sec > 0 ? "午休时段，不计薪" : "午休结束，恢复计薪";
        }
        if (s === "leave") return "今日请假，不计薪";
        if (s === "done") return "今日工时已计满";
        if (s === "ot") return "加班时段，按设置的口径计薪";
        if (s === "unpaid") {
          return d.shadow_src === "break" ? "午休加班，不计薪，仅记录时长" : "加班不计薪，仅记录时长";
        }
        return "工作时段，正常计薪";
      }
    },

    /* 静默：一个字都不说。tip 整行不显示，标签也压到最短。 */
    silent: {
      labels: {
        on: "在岗", break: "午休", ot: "加班",
        unpaid: "无薪加班", leave: "请假", done: "已下班", paused: "已暂停"
      },
      tip: function () { return ""; }
    }
  };

  function copyOf(d) { return COPY[d.copy_style] || COPY.emotional; }

  /* ---- 格式化：整数 → 字符串，唯一的「换算」是除以 1000 / 1e6 ---- */

  function pad(n) { return n < 10 ? "0" + n : "" + n; }

  /* 天数：配置里存的是「天 × 100」的整数，这里只除以 100 再加一位小数。 */
  function days(centi) {
    return (centi / 100).toFixed(1);
  }

  function hms(sec) {
    sec = Math.max(0, Math.floor(sec || 0));
    return pad(Math.floor(sec / 3600)) + ":" + pad(Math.floor(sec % 3600 / 60)) + ":" + pad(sec % 60);
  }

  /* 金额：三位小数。第三位就是要它一直跳——那是这个产品存在的理由。 */
  function money3(milli) {
    return (milli / 1000).toFixed(3);
  }

  /* 月合计要千分位：「本月已赚 ¥7,588.97」比 ¥7588.97 好读。 */
  function money2(milli) {
    return (milli / 1000).toLocaleString("zh-CN", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2
    });
  }

  function render(d) {
    /* 休息日：整块盖住，一个数字都不给。
       给 0 会被当成「今天赚了 0 元」，给满勤又会虚增一天工资。 */
    el.restMask.hidden = !d.rest;
    el.hud.dataset.masked = d.hide_money ? "1" : "0";
    el.hud.dataset.paused = d.paused ? "1" : "0";
    el.hud.dataset.thru = d.click_through ? "1" : "0";

    var s = d.state;
    /* 这一帧用哪套文案。静默档的 tip 是空串，CSS 侧按 [data-copy="silent"]
       把整行收掉——留一个 16px 的空行在那儿，比说话还吵。 */
    var copy = copyOf(d);
    el.hud.dataset.copy = d.copy_style || "emotional";

    /* 外观三件套：强调色、不透明度、信息密度。
       改成 CSS 变量而不是在这里拼颜色——色相一致就够，具体怎么调是样式的事。
       `accent_rgb` 由 Rust 一起给：rgba() 需要三元组，而「#35E07F → 53,224,127」
       是解析，不是算术，但放在后端可以保证前端一行转换都没有。 */
    el.hud.style.setProperty("--user-accent", d.accent);
    el.hud.style.setProperty("--user-accent-rgb", d.accent_rgb);
    el.hud.style.setProperty("--user-alpha", (d.opacity / 100).toFixed(2));
     /* 这里列的是**要藏起来**的那几块，不是要显示的。
       反过来写过一次：标记写「显示」、CSS 却按「有标记就藏」匹配，
       结果四个开关全开的时候版面一片空白——那种错只有真机截图能看出来。 */
    el.hud.dataset.off = [
      d.density_metrics ? "" : "m",
      d.density_bar ? "" : "b",
      d.density_tip ? "" : "t",
      d.density_shadow ? "" : "s"
    ].filter(Boolean).join(" ");

    /* 主题与数字字体。两个都只是 dataset，具体长什么样全在 CSS 里——
       换皮肤不该走到这个文件里的任何一行数字。 */
    if (d.theme !== wantTheme) {
      wantTheme = d.theme || "dark";
      paintTheme();
    }
    el.hud.dataset.font = d.num_font || "sf";

    if (!d.rest) {
      el.amount.textContent = money3(d.amount_milli);
      el.perSec.textContent = "¥" + (d.per_sec_micro / 1e6).toFixed(4);
      el.hourly.textContent = "¥" + (d.hourly_milli / 1000).toFixed(2);
      el.daily.textContent = "¥" + (d.day_milli / 1000).toFixed(2);

      el.worked.textContent = hms(d.worked_sec);
      el.pct.textContent = Math.round(d.progress * 100) + "%";
      el.barFill.style.width = (d.progress * 100).toFixed(2) + "%";

      /* 三列指标默认是「钱的三档」，请假那天换成「假的三个数」——
         原型的请假态卡就是这么排的（类型 / 剩余 / 本月）。 */
      if (s === "leave") {
        el.k1.textContent = "类型";
        el.k2.textContent = "剩余";
        el.k3.textContent = "本月";
        el.perSec.textContent = d.leave_kind;
        el.hourly.textContent = days(d.leave_left_centi) + " 天";
        el.daily.textContent = "-" + d.leave_month_days + " 天";
      } else {
        el.k1.textContent = "每秒进账";
        el.k2.textContent = "时薪";
        el.k3.textContent = "日薪";
      }

      /* 右槽的语义随状态变。义务加班时绝不能写「距下班」——
         那会让人以为还在正常上班（这条是原型阶段踩过的坑）。
         已下班 / 加班时也不写「距下班 00:00:00」：那读起来像卡住了，
         改成「已过 / 已加班」，同一个数（超出下班点的秒数）换个说法。 */
      if (s === "unpaid") {
        el.remainLabel.textContent = "已白干";
        el.remain.textContent = hms(d.shadow_sec);
      } else if (s === "break") {
        el.remainLabel.textContent = "午休剩余";
        el.remain.textContent = hms(d.lunch_remain_sec);
      } else if (s === "ot") {
        el.remainLabel.textContent = "已加班";
        el.remain.textContent = hms(d.ot_sec);
      } else if (s === "done") {
        el.remainLabel.textContent = "已过";
        el.remain.textContent = hms(d.ot_sec);
      } else if (s === "leave") {
        /* 请假没有「距下班」可言，改报余额——这是这一天唯一还在动的数 */
        el.remainLabel.textContent = "余额";
        el.remain.textContent = days(d.leave_left_centi) + " 天";
      } else {
        el.remainLabel.textContent = "距下班";
        el.remain.textContent = hms(d.remain_sec);
      }

      el.monthEarn.textContent = "¥" + money2(d.month_earn_milli);
      el.monthLeft.textContent = d.month_left_days;
    }

    /* 白干条：只要今天记过就一直留着（原型稿的规矩），
       但主账一分都不会因为它变多。
       请假当天不该出现——那天根本没上班，谈不上白干。 */
    var hasShadow = d.shadow_milli >= 5 && d.density_shadow && s !== "leave";
    el.hud.classList.toggle("has-shadow", hasShadow);
    if (hasShadow) {
      el.shadowAmt.textContent = "¥" + money2(d.shadow_milli);
    }

    el.tip.textContent = copy.tip(d, s);

    /* 快捷动作：只在「现在确实能做点什么」的状态出现。
       在岗、请假不显示，避免误触。 */
    var act = (s === "break" || s === "done") ? "start"
            : (s === "ot" || s === "unpaid") ? "stop" : "";
    el.hud.dataset.act = act;
    el.actStartTxt.textContent =
      (s === "break" && d.lunch_remain_sec === 0) ? "开始下午班" : "我在加班";

    el.stateLabel.textContent = d.paused ? copy.labels.paused : (copy.labels[s] || copy.labels.on);
    el.hud.dataset.state = s;

    /* 尺寸档位。三档是三套版式（紧凑砍内容、展开加内容），
       所以只用一个 class 开关，不做宽度计算——宽度写在 CSS 里。 */
    var sz = "size-" + (d.size || "normal");
    if (!el.hud.classList.contains(sz)) {
      SIZES.forEach(function (c) { el.hud.classList.remove(c); });
      el.hud.classList.add(sz);
    }

    /* 展开态那块等价物。数字全来自 Rust，这里只负责拼字。 */
    el.eqName.textContent = d.eq_name;
    el.eqCups.textContent = (d.eq_cups_x10 / 10).toFixed(1) + " 杯";
    el.eqGoal.textContent = d.eq_goal_cups.toLocaleString("zh-CN");
    el.eqLeft.textContent = (d.eq_left_cups_x10 / 10).toFixed(1) + " 杯";
    el.eqDay.textContent = d.eq_day_cups;
    el.eqGoalBar.style.width = (d.eq_goal_pct * 100).toFixed(1) + "%";
    el.eqDayBar.style.width = (d.eq_day_pct * 100).toFixed(1) + "%";

    fit();
  }

  var lastW = 0;
  var lastH = 0;
  var shown = false;

  /* 内容尺寸变了就让窗口跟着变（宽和高都要传）。
     不做的话：白干条一出现就被窗口下沿切掉；切到紧凑档时窗口还留着 302 宽，
     右边那 66px 透明区域会吃掉桌面上的点击——那是可用性问题，不是审美。

     宽度必须和高度一起报：三档的宽度是 CSS 定的（236 / 302 / 322），
     高度是内容撑的，只有量一次才知道。 */
  function fit() {
    var r = el.hud.getBoundingClientRect();
    var w = Math.ceil(r.width);
    var h = Math.ceil(r.height);
    if (!w || !h) return;
    if (Math.abs(w - lastW) < 0.5 && Math.abs(h - lastH) < 0.5) return;
    lastW = w;
    lastH = h;
    if (invoke) invoke("resize_hud", { width: w, height: h }).catch(function () {});
    /* 第一帧量完就把窗口显示出来（配置里它是藏着的）。
       顺序很关键：先 resize 再 show，否则会先闪一下 242 高再跳成 258。 */
    if (!shown) {
      shown = true;
      if (invoke) invoke("ready").catch(function () {});
    }
  }

  /* 拖动。

     `data-tauri-drag-region` 只在**事件目标自己**带这个属性时才生效，
     而这块内容几乎每个像素上都压着子元素（金额那串数字、`<b>`、`<i>`、图标）——
     用户在数字上按住一拖，目标就是那个 `<span>`，属性不在它身上，于是纹丝不动。
     实测就是这样：边缘的空白能拖，最想拖的那块拖不动，结论就是「拖不动」。

     所以这里自己接一次：整块 HUD 都能按，除了按钮（穿透 ◎、我在加班、结束加班）。
     属性还留着——它不影响这条路径，去掉反而少一层兜底。 */
  var win = (function () {
    var t = window.__TAURI__;
    var w = t && t.window;
    if (!w) return null;
    if (w.getCurrentWindow) return w.getCurrentWindow();
    if (w.getCurrent) return w.getCurrent();   // 2.0 早期的名字
    return null;
  })();

  document.addEventListener("mousedown", function (e) {
    if (!win || e.button !== 0) return;
    if (e.target.closest("button")) return;
    e.preventDefault();          // 别让这次按下变成选文字
    win.startDragging().catch(function () {});
  }, true);

  /* 右键。
   *
   * 不拦的话弹出来的是 WebView2 自己的**网页菜单**——「重新加载」「检查」
   * 那一套。这东西出现在一个桌面小挂件上，用户第一反应是「怎么是个网页」。
   * 原型稿第 1379 行定的是自家菜单（设置 / 切换尺寸 / 暂停计时 / 今日明细 / 退出），
   * 这里拦掉默认动作，把鼠标屏幕坐标交给 Rust 弹自绘深色菜单。
   *
   * `preventDefault` 必须真的调：WebView2 的默认菜单是 DOM 默认动作的后果，
   * 拦掉这个事件它就不弹了。 */
  document.addEventListener("contextmenu", function (e) {
    e.preventDefault();
    if (invoke) invoke("hud_menu").catch(function () {});
  }, true);

  function act(start) {
    if (!invoke) return;
    invoke("act", { start: start })
      .then(render)
      .catch(function () {});
  }

  el.actStart.addEventListener("click", function (e) { e.stopPropagation(); act(true); });
  el.actStop.addEventListener("click", function (e) { e.stopPropagation(); act(false); });
  el.thruBtn.addEventListener("click", function (e) {
    e.stopPropagation();
    if (invoke) invoke("set_click_through", { on: true }).catch(function () {});
  });

  /* ---- 三档尺寸循环（原型 01-D「缩放」） ----
     三条入口共用后端那一个 cycle_size：双击标题区、窗口上滚轮、托盘菜单。
     档位存在配置里，所以切完重开还是那一档。 */
  var sizeBusy = false;

  function cycleSize(dir) {
    if (!invoke || sizeBusy) return;
    sizeBusy = true;
    invoke("cycle_size", { dir: dir })
      .then(render)
      .catch(function () {})
      .then(function () { sizeBusy = false; });
  }

  /* 拖动区（data-tauri-drag-region）在 Tauri 2 里会把双击解释成「最大化」。
     在一个 302px 的悬浮窗上双击变全屏，是很难解释的行为——所以在这里拦掉，
     顺手把「双击标题区切尺寸」做掉（原型 01-D）。
     必须用捕获阶段：Tauri 注入的监听在冒泡阶段，晚了。 */
  document.addEventListener("dblclick", function (e) {
    e.preventDefault();
    e.stopPropagation();
    if (e.target.closest && e.target.closest(".hud-head")) cycleSize(1);
  }, true);

  /* 滚轮也切档，但必须节流：一次滚轮会连发十几个 wheel 事件，
     不拦的话滚一下能从紧凑直接转到展开还多绕一圈。
     这个窗口没有可滚动的内容，所以滚轮不存在别的语义。 */
  var lastWheel = 0;
  document.addEventListener("wheel", function (e) {
    var t = Date.now();
    if (t - lastWheel < 300) return;
    lastWheel = t;
    cycleSize(e.deltaY > 0 ? 1 : -1);
  }, { passive: true });

  /* ---- 主循环 ---- */

  var busy = false;

  function loop() {
    if (busy || !invoke) return;
    busy = true;
    invoke("tick")
      .then(function (d) {
        el.hud.dataset.dead = "0";
        render(d);
      })
      .catch(function () { dead(); })
      .then(function () { busy = false; });
  }

  /* 后端不在时不显示旧数字，也不假装在跑：给「—」。
     留着上一帧的数字比报错更糟——用户会以为钱还在涨。 */
  function dead() {
    el.hud.dataset.dead = "1";
    el.amount.textContent = "—";
    el.stateLabel.textContent = "未就绪";
    el.tip.textContent = "引擎没起来，金额暂时不显示";
    fit();
  }

  if (!invoke) {
    dead();
    return;
  }
  loop();
  /* 10 Hz：第三位小数每 73 毫秒动一次，10 Hz 看着就是连续在跳。
     提到 60 Hz 只是白白多烧 CPU。 */
  setInterval(loop, 100);
  window.addEventListener("resize", fit);
})();
