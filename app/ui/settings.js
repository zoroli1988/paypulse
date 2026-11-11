/* 设置窗口前端。
 *
 * 四条规矩，和前几个文件一致：
 *   1. **这里不做算术。** 日薪、时薪、每秒进账、计薪工时、年终奖摊分全由 Rust 算好送过来，
 *      这边只做 `toFixed()` 这类格式化。改一个月薪要同时改四处数字，
 *      让前端各算一遍迟早有一处对不上。
 *   2. **也不做日期算术。** 日历格子、某天是谁、下个假日还有几天，都是 Rust 排好送来的；
 *      连「点哪一天」回传的都是 `YYYY-MM-DD` 字符串，不是自己拼的。
 *   3. **没有「保存」。** 每个控件 change 完就发一次补丁，返回的完整视图立刻回填。
 *   4. **回填时跳过正在输入的那个框。** 否则用户打到一半小数点会被格式化冲掉。
 */
(function () {
  "use strict";

  /* 这扇窗不提供右键菜单。不拦的话 WebView2 会弹它自己的网页菜单
     （「重新加载」「检查」那一套）——一个桌面小工具上冒出这个很莫名其妙。
     悬浮窗是例外：那边右键要弹自家的菜单，处理在 hud.js。 */
  document.addEventListener("contextmenu", function (e) { e.preventDefault(); }, true);

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  var el = {};
  [
    "nav", "body", "closeBtn", "fMonth", "fMonthLabel", "fDays", "fHours", "fOt",
    "tStart", "tEnd", "tL1", "tL2", "schedCalc",
    "dDaily", "dHourly", "dSec", "dSched", "otBlk", "otTitle", "otDesc",
    "szRow", "thGrid", "ftRow", "cpChips", "cpPreview", "acRow", "acVal", "opRng", "opVal",
    "fEqPrice", "fEqGoal", "fEqDay", "eqLive", "eqChips",
    "leaveChips", "fLeaveLeft", "swLeaveToday",
    "swThru", "dataPath", "kVer", "kDir", "verLine",
    "grossSeg", "bonusChips", "bonusFields", "bonusOut",
    "wsCards", "wkGrid", "wkHint", "calGrid", "calMonth", "calHol", "calRest",
    "calMk", "calNext", "calMode", "fTz", "swCross",
    "hkShow", "hkPassthru", "hkClock", "hkExtra", "keyHint",
    "swBoot", "swSilent", "snapRng", "snapVal", "mmChips", "swIdle", "fIdleMin",
    "idleVal", "idleNow", "segClose",
    "swEnd", "endChips", "swLunch", "lunchChips", "swSit", "segSit",
    "swDnd", "dndFrom", "dndTo",
    "clrToday", "clrMonth", "clrAll", "clrBox", "clrWhat", "clrWord", "clrGo", "clrCancel",
    "welcome", "welcomeGo", "updBody", "btnCheck", "updChan", "updSub"
  ].forEach(function (id) { el[id] = document.getElementById(id); });

  var view = null;
  /* 日历当前的标记模式：放假 / 补班。点格子时读它。 */
  var calMode = "holiday";

  /* ---- 取值 / 回填 ---- */

  function txt(id, s) {
    var n = el[id];
    if (n && document.activeElement !== n) n.textContent = s;
  }

  function val(id, s) {
    var n = el[id];
    if (n && document.activeElement !== n) n.value = s;
  }

  function money(v, digits) {
    return "¥" + v.toFixed(digits);
  }

  function setSwitch(node, on) {
    if (!node) return;
    node.classList.toggle("on", !!on);
  }

  function markChosen(row, attr, on) {
    if (!row) return;
    Array.prototype.forEach.call(row.children, function (c) {
      c.classList.toggle("on", c.getAttribute(attr) === on);
    });
  }

  /* 文案风格的预览句：三种说法摆在一起才看得出差别。
     静默档没有句子，就照实写「不说话」——这一格本来就是拿来对比的。 */
  var COPY_SAMPLE = {
    emotional: "过半了，够买份午饭",
    plain: "工作时段，正常计薪",
    silent: "（不说话，只留数字）"
  };

  function paintCopyPreview(v) {
    if (el.cpPreview) el.cpPreview.textContent = COPY_SAMPLE[v] || COPY_SAMPLE.emotional;
  }

  /* 弹性那张卡是开关，不是形态：亮着 = 按当天第一次活动算起点。 */
  function markFlex(on) {
    var card = el.wsCards.querySelector('[data-w="flex"]');
    if (!card) return;
    card.classList.toggle("on", on);
    var m = card.querySelector(".ws-m");
    if (m) m.textContent = on ? "按首次活动 · 已开启" : "按首次活动 · 已关闭";
  }

  function markColor(hex) {
    Array.prototype.forEach.call(el.acRow.children, function (c) {
      var on = c.getAttribute("data-ac").toUpperCase() === String(hex).toUpperCase();
      c.classList.toggle("on", on);
    });
    el.acVal.textContent = hex;
  }

  function markDensity(d) {
    var map = { metrics: d.metrics, bar: d.bar, tip: d.tip, shadow: d.shadow };
    Array.prototype.forEach.call(document.querySelectorAll("[data-den]"), function (n) {
      setSwitch(n, map[n.getAttribute("data-den")]);
    });
  }

  function markEq(name) {
    Array.prototype.forEach.call(el.eqChips.children, function (c) {
      c.classList.toggle("on", c.getAttribute("data-eq") === name);
    });
  }

  function fill(v) {
    view = v;

    /* 口径只改称呼：填「税前」时标题就是「税前月薪」。 */
    el.fMonthLabel.textContent = v.gross ? "税前月薪" : "税后月薪";
    markChosen(el.grossSeg, "data-v", v.gross ? "1" : "0");

    val("fMonth", v.month_yuan.toFixed(2));
    val("fDays", String(v.pay_days_centi));
    val("fHours", v.daily_hours.toFixed(1));
    el.fOt.value = v.ot_paid ? "paid" : "shadow";

    val("tStart", v.sched.start);
    val("tEnd", v.sched.end);
    val("tL1", v.sched.lunch1);
    val("tL2", v.sched.lunch2);
    el.schedCalc.innerHTML =
      "在岗 " + v.sched.span_h.toFixed(1) + " h − 午休 " + v.sched.lunch_h.toFixed(1) +
      " h = <b>计薪 " + v.sched.paid_h.toFixed(1) + " h</b><span>已同步到「薪资与口径」</span>";

    el.dDaily.textContent = money(v.day_yuan, 2);
    el.dHourly.textContent = money(v.hourly_yuan, 2);
    el.dSec.textContent = money(v.per_sec_yuan, 4);
    el.dSched.textContent = v.daily_hours.toFixed(1) + " h";

    el.otTitle.textContent = v.ot_paid ? "当前：按 1.5× 计入月薪" : "当前：不计加班费";
    el.otDesc.textContent = v.ot_paid
      ? "加班时长乘以 1.5 倍时薪，直接加进今日已赚与月合计。选这一档的前提是你真能拿到。"
      : "加班时段不计入月薪，但会单独折算一笔「白干价值」。这笔钱不占月薪、不进合计、不参与任何汇总。";
    el.otBlk.className = "blk " + (v.ot_paid ? "info" : "amber");

    /* ---- 年终奖 ---- */
    markChosen(el.bonusChips, "data-bm", v.bonus.mode);
    renderBonusFields(v);
    renderBonusOut(v);

    /* ---- 排班 ---- */
    markChosen(el.wsCards, "data-w", v.week.form);
    markFlex(!!v.week.flex);
    markWeekDays(v.week.days);
    el.wkHint.textContent = v.week.form === "custom"
      ? "当前是自定义排班：点某一天单独切换「班 / 休」"
      : "点某一天可单独切换「班 / 休」（改完就不再是上面那种形态）";

    renderCalendar(v.week);
    el.fTz.value = v.tz_fixed ? "fix" : "sys";
    setSwitch(el.swCross, v.sched.cross_midnight);

    /* ---- 外观 ---- */
    markChosen(el.szRow, "data-sz", v.size);
    markChosen(el.thGrid, "data-th", v.theme);
    markChosen(el.ftRow, "data-f", v.num_font);
    markChosen(el.cpChips, "data-cp", v.copy_style);
    paintCopyPreview(v.copy_style);
    markColor(v.accent);
    el.opRng.value = v.opacity;
    el.opVal.textContent = v.opacity + "%";
    markDensity(v.density);

    /* ---- 行为 ---- */
    paintHotkeys(v.hotkey);
    setSwitch(el.swBoot, v.autostart);
    setSwitch(el.swSilent, v.silent_start);
    el.snapRng.value = v.snap_px;
    el.snapVal.textContent = v.snap_px === 0 ? "关" : String(v.snap_px);
    markChosen(el.mmChips, "data-mm", v.monitor_primary ? "primary" : "remember");
    setSwitch(el.swThru, v.click_through);
    setSwitch(el.swIdle, v.idle_detect);
    el.fIdleMin.value = String(v.idle_min);
    el.idleVal.textContent = String(v.idle_min);
    el.idleNow.textContent = String(v.idle_now_sec);
    markChosen(el.segClose, "data-v", v.close_action);

    /* ---- 通知 ---- */
    setSwitch(el.swEnd, v.notify.end !== "off");
    markChosen(el.endChips, "data-v", v.notify.end === "off" ? "" : v.notify.end);
    setSwitch(el.swLunch, v.notify.lunch);
    markChosen(el.lunchChips, "data-v", String(v.notify.lunch_before));
    setSwitch(el.swSit, v.notify.sit);
    markChosen(el.segSit, "data-v", String(v.notify.sit_min));
    setSwitch(el.swDnd, v.notify.dnd);
    val("dndFrom", v.notify.dnd_from);
    val("dndTo", v.notify.dnd_to);

    /* ---- 等价物 / 请假 ---- */
    val("fEqPrice", v.eq_price_yuan.toFixed(2));
    val("fEqGoal", String(v.eq_goal_cups));
    val("fEqDay", String(v.eq_day_cups));
    markEq(v.eq_name);
    /* 折合杯数在 Rust 的 snapshot_at 里除过一次，这里直接拿那个数——不在第二处再除 */
    el.eqLive.textContent = "今天 ≈ " + (v.today_cups_x10 / 10).toFixed(1) + " 杯" + v.eq_name;

    markChosen(el.leaveChips, "data-lk", v.leave_kind);
    val("fLeaveLeft", v.leave_left_days.toFixed(1));
    setSwitch(el.swLeaveToday, v.today_leave);

    el.dataPath.textContent = v.data_dir;
    el.kDir.textContent = v.data_dir;
    txt("kVer", v.version);
    txt("verLine", "PayPulse · 版本 " + v.version);
  }

  /* ---- 年终奖的两个动态块 ---- */

  function renderBonusFields(v) {
    var m = v.bonus.mode;
    var html = "";
    if (m === "none") {
      el.bonusFields.innerHTML =
        '<div class="blk-d">当前不计入。点上面的「自定义 xx 薪」可以填<b style="color:#A78BFA">任意月数</b>——' +
        "13 薪填 1.0，15 薪填 3.0，12.5 薪填 0.5，月数不必是整数。</div>";
      return;
    }
    html += '<div class="bonus-f">';
    if (m === "months") {
      html += '<label>年终奖 = 月薪 ×</label>' +
        '<input class="inp mono" id="bMonths" type="number" step="0.5" min="0" max="12" ' +
        'value="' + v.bonus.months.toFixed(1) + '" style="width:76px;height:28px" />' +
        '<span class="u">个月（1.0 = 13 薪）</span>';
    } else if (m === "amount") {
      html += '<label>年终奖金额</label>' +
        '<input class="inp mono" id="bAmount" type="number" step="1000" min="0" ' +
        'value="' + v.bonus.amount_yuan.toFixed(0) + '" style="width:110px;height:28px" />' +
        '<span class="u">元 / 年（红包、项目奖都算这里）</span>';
    } else {
      html += '<label>占年薪比例</label>' +
        '<input class="inp mono" id="bRate" type="number" step="0.5" min="0" max="500" ' +
        'value="' + v.bonus.rate_pct.toFixed(1) + '" style="width:76px;height:28px" />' +
        '<span class="u">% （月薪 × 12 × 比例）</span>';
    }
    html += "</div>";
    if (m === "months") {
      /* 常用值做成快捷 chip。标签写死，不在前端算「12 + n」——
         那几个数就是 12.5 / 13 / 13.5 / 14 / 15 / 16 薪，写出来比算出来省事。 */
      var quick = [
        ["0.5", "12.5 薪"], ["1", "13 薪"], ["1.5", "13.5 薪"],
        ["2", "14 薪"], ["3", "15 薪"], ["4", "16 薪"]
      ];
      html += '<div class="chips" id="bQuick" style="margin-top:9px">' +
        quick.map(function (q) {
          var on = Math.abs(v.bonus.months - parseFloat(q[0])) < 1e-6 ? " on" : "";
          return '<span class="chip' + on + '" data-mv="' + q[0] + '">' + q[1] + "</span>";
        }).join("") +
        '<span class="u" style="align-self:center;font-size:10px;padding-left:4px">这些都是常用值，不是上限</span></div>';
    }
    el.bonusFields.innerHTML = html;
  }

  function renderBonusOut(v) {
    var b = v.bonus;
    if (b.mode === "none" || b.total_yuan <= 0) {
      el.bonusOut.innerHTML =
        "年终奖<b>不计入任何折算</b>，仅在年报里作为独立条目记录。时薪仍按每日 " +
        v.daily_hours.toFixed(1) + " h 折算。";
      return;
    }
    var tag = b.mode === "months"
      ? "（月薪 × " + b.months.toFixed(1) + " 个月 ＝ " + b.annual_months.toFixed(1) + " 薪）"
      : b.mode === "amount" ? "（自定义金额）" : "（年薪 " + b.rate_pct.toFixed(1) + "%）";
    el.bonusOut.innerHTML =
      "年终奖 <b>¥" + money2(b.total_yuan) + "</b><span style=\"color:#7E8792\">" + tag + "</span>　→　" +
      "摊入月薪 <b>+¥" + money2(b.per_month_yuan) + "</b><br>" +
      "时薪（含年终奖）<b>¥" + v.hourly_yuan.toFixed(2) + "</b>" +
      "<span style=\"color:#7E8792\">　只在「摊入 12 个月」这一种口径下成立</span>";
  }

  function money2(v) {
    return v.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  /* ---- 每周排班 ---- */

  function markWeekDays(days) {
    Array.prototype.forEach.call(el.wkGrid.children, function (c) {
      var on = !!days[parseInt(c.getAttribute("data-d"), 10)];
      c.classList.toggle("on", on);
      c.querySelector("span").textContent = on ? "班" : "休";
    });
  }

  /* ---- 日历 ---- */

  var CELL_CLASS = { work: "d", rest: "d rest", holiday: "d hol", makeup: "d mk" };

  function renderCalendar(w) {
    el.calMonth.textContent = w.month;
    el.calHol.textContent = String(w.holidays);
    el.calRest.textContent = String(w.rest);
    el.calMk.textContent = String(w.makeup);

    var html = "";
    ["一", "二", "三", "四", "五", "六", "日"].forEach(function (h) {
      html += '<div class="ch">' + h + "</div>";
    });
    for (var i = 0; i < (w.lead || 0); i++) html += '<div class="d mute"></div>';
    (w.cells || []).forEach(function (c) {
      var cls = CELL_CLASS[c.kind] || "d";
      if (c.today) cls += " today";
      html += '<div class="' + cls + '" data-iso="' + c.iso + '" data-kind="' + c.kind +
        '" title="' + c.iso + '">' + c.d + "</div>";
    });
    el.calGrid.innerHTML = html;
    el.calNext.textContent = w.next_holiday;
  }

  /* ---- 快捷键 ---- */

  var HK_KEYS = [
    ["hkShow", "hk_show", "show"],
    ["hkPassthru", "hk_passthru", "passthru"],
    ["hkClock", "hk_clock", "clock"],
    ["hkExtra", "hk_extra", "extra"]
  ];

  function paintHotkeys(h) {
    HK_KEYS.forEach(function (row) {
      var node = el[row[0]];
      if (!node) return;
      node.setAttribute("data-spec", h[row[2]] || "");
      paintKeys(node, h[row[2]] || "");
      node.classList.toggle("dead", !h.enabled);
    });
  }

  function paintKeys(node, spec) {
    node.innerHTML = "";
    if (!spec) {
      var e = document.createElement("span");
      e.className = "rec-txt";
      e.textContent = "点这里录";
      node.appendChild(e);
      return;
    }
    spec.split("+").forEach(function (k, i) {
      if (i) {
        var plus = document.createElement("i");
        plus.textContent = "+";
        node.appendChild(plus);
      }
      var kbd = document.createElement("kbd");
      kbd.textContent = k;
      node.appendChild(kbd);
    });
  }

  var recording = null;

  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") {
      if (recording) {
        recording.classList.remove("rec");
        recording = null;
        el.keyHint.textContent = "点方框 → 按下组合键；Esc 取消";
        e.preventDefault();
        return;
      }
      close();
      return;
    }
    if (!recording) return;

    e.preventDefault();
    e.stopPropagation();

    /* 只按修饰键时先不动：等用户按到那个「主键」再提交。
       否则录 Ctrl+Shift+P 会在按下 Shift 的那一瞬间就把「Ctrl+Shift」当成结果。 */
    var k = e.key;
    if (k === "Control" || k === "Shift" || k === "Alt" || k === "Meta") return;

    var parts = [];
    if (e.ctrlKey) parts.push("Ctrl");
    if (e.shiftKey) parts.push("Shift");
    if (e.altKey) parts.push("Alt");
    if (e.metaKey) parts.push("Win");
    if (!parts.length) {
      /* 没有修饰键的组合会抢走整个系统的那个键，直接拒绝并说明原因 */
      el.keyHint.textContent = "必须带 Ctrl / Alt / Shift / Win 之一";
      return;
    }
    parts.push(keyName(k));
    var patch = {};
    patch[recording.getAttribute("data-patch")] = parts.join("+");
    recording.classList.remove("rec");
    recording = null;
    el.keyHint.textContent = "点方框 → 按下组合键；Esc 取消";
    apply(patch);
  }, true);

  function keyName(k) {
    if (k.length === 1) return k.toUpperCase();
    var map = {
      Enter: "Enter", " ": "Space", Tab: "Tab", Backspace: "Backspace",
      Delete: "Delete", Insert: "Insert", Home: "Home", End: "End",
      PageUp: "PageUp", PageDown: "PageDown",
      ArrowLeft: "Left", ArrowUp: "Up", ArrowRight: "Right", ArrowDown: "Down"
    };
    return map[k] || k;
  }

  /* ---- 发补丁 ---- */

  var busy = false;
  var queued = null;

  function apply(patch) {
    if (!invoke) return;
    if (busy) { queued = patch; return; }
    busy = true;
    invoke("apply_settings", { patch: patch })
      .then(function (v) { fill(v); })
      .catch(function (e) { console.error("保存失败", e); })
      .then(function () {
        busy = false;
        if (queued) { var p = queued; queued = null; apply(p); }
        // 任何一次落盘之后，首启引导就完成使命了：
        // 用户可能先改月薪（change 就写盘了）再想起点「开始」，
        // 也可能一次都没点。这条判断两种顺序都对。
        if (el.welcome && !el.welcome.hidden) {
          invoke("first_run").then(function (on) {
            if (!on) el.welcome.hidden = true;
          }).catch(function () {});
        }
      });
  }

  function density() {
    var d = { metrics: false, bar: false, tip: false, shadow: false };
    Array.prototype.forEach.call(document.querySelectorAll("[data-den]"), function (n) {
      d[n.getAttribute("data-den")] = n.classList.contains("on");
    });
    return d;
  }

  /* ---- 分组切换 ---- */

  function selectGroup(g) {
    Array.prototype.forEach.call(el.nav.querySelectorAll(".i"), function (n) {
      n.classList.toggle("on", n.getAttribute("data-g") === g);
    });
    Array.prototype.forEach.call(document.querySelectorAll(".pane"), function (p) {
      p.classList.toggle("on", p.getAttribute("data-g") === g);
    });
    el.body.scrollTop = 0;
  }

  el.nav.addEventListener("click", function (e) {
    var item = e.target.closest(".i");
    if (item) selectGroup(item.getAttribute("data-g"));
  });

  /* 支持 `settings.html?g=look` 直接落到某一组。
     截图探针要靠它逐组拍；顺手也让人能把某一页发给别人看。 */
  var want = new URLSearchParams(location.search).get("g");
  if (want) selectGroup(want);

  /* ---- 关闭 ---- */

  function close() {
    if (window.__TAURI__ && window.__TAURI__.window) {
      window.__TAURI__.window.getCurrentWindow().close();
    }
  }
  el.closeBtn.addEventListener("click", close);

  /* ---- 拖动标题栏 ----

     和悬浮窗同一个坑：`data-tauri-drag-region` 只在**事件目标自己**带这个属性时才生效，
     而标题栏 .bar 里压着三个 mac 圆点、标题文字、关闭按钮——在文字或圆点上按住一拖，
     目标就是那个 `<span>`/`<i>`，属性不在它身上，于是纹丝不动。
     所以自己接一次：只在 .bar 区域内按下就 startDragging，关闭按钮除外。
     属性还留着，不影响这条路径，去掉反而少一层兜底。 */
  (function () {
    var w = null;
    var t = window.__TAURI__;
    if (t && t.window) {
      if (t.window.getCurrentWindow) w = t.window.getCurrentWindow();
      else if (t.window.getCurrent) w = t.window.getCurrent();
    }
    var bar = document.querySelector(".bar");
    if (bar && w) {
      bar.addEventListener("mousedown", function (e) {
        if (e.button !== 0) return;
        if (e.target.closest("button")) return;   // 关闭按钮不放行
        e.preventDefault();                        // 别把按下变成选文字
        w.startDragging().catch(function () {});
      });
    }
  })();

  /* ---- 绑定：薪资与口径 ---- */

  el.fMonth.addEventListener("change", function () {
    var v = parseFloat(el.fMonth.value);
    if (isFinite(v)) apply({ month_yuan: v });
  });

  el.fDays.addEventListener("change", function () {
    apply({ pay_days_centi: parseInt(el.fDays.value, 10) });
  });

  el.fHours.addEventListener("change", function () {
    var v = parseFloat(el.fHours.value);
    if (isFinite(v)) apply({ daily_hours: v });
  });

  el.fOt.addEventListener("change", function () {
    apply({ ot_paid: el.fOt.value === "paid" });
  });

  el.grossSeg.addEventListener("click", function (e) {
    var b = e.target.closest("button");
    if (b) apply({ gross: b.getAttribute("data-v") === "1" });
  });

  /* 年终奖的输入框是动态生成的，用委托接 change */
  el.bonusFields.addEventListener("change", function (e) {
    var t = e.target;
    if (!t.id) return;
    var v = parseFloat(t.value);
    if (!isFinite(v)) return;
    if (t.id === "bMonths") apply({ bonus_months: v });
    else if (t.id === "bAmount") apply({ bonus_amount_yuan: v });
    else if (t.id === "bRate") apply({ bonus_rate_pct: v });
  });

  /* ---- 绑定：作息与工作日 ---- */

  /* 作息四个时刻：任何一个改动都把四个一起发——
     只发一个的话，后端拿着「新的上班 + 旧的午休」会先夹一次，
     用户会看到午休被莫名改成默认值。 */
  function pushSched() {
    apply({
      sched: {
        start: el.tStart.value,
        end: el.tEnd.value,
        lunch1: el.tL1.value,
        lunch2: el.tL2.value
      }
    });
  }
  [el.tStart, el.tEnd, el.tL1, el.tL2].forEach(function (n) {
    n.addEventListener("change", pushSched);
  });

  el.swCross.addEventListener("click", function () {
    apply({ cross_midnight: !(view && view.sched.cross_midnight) });
  });

  el.fTz.addEventListener("change", function () {
    apply({ tz_fixed: el.fTz.value === "fix" });
  });

  el.calMode.addEventListener("click", function (e) {
    var b = e.target.closest("button");
    if (!b) return;
    calMode = b.getAttribute("data-v");
    markChosen(el.calMode, "data-v", calMode);
  });

  /* ---- 绑定：外观 ---- */

  el.szRow.addEventListener("click", function (e) {
    var item = e.target.closest(".sz");
    if (item) apply({ size: item.getAttribute("data-sz") });
  });

  el.thGrid.addEventListener("click", function (e) {
    var item = e.target.closest(".th");
    if (item) apply({ theme: item.getAttribute("data-th") });
  });

  el.ftRow.addEventListener("click", function (e) {
    var item = e.target.closest(".chip");
    if (item) apply({ num_font: item.getAttribute("data-f") });
  });

  el.cpChips.addEventListener("click", function (e) {
    var item = e.target.closest(".chip");
    if (!item) return;
    var v = item.getAttribute("data-cp");
    // 预览句当场换掉，不等补丁回来——这是「点了有没有反应」最小的那个证明
    paintCopyPreview(v);
    apply({ copy_style: v });
  });

  el.acRow.addEventListener("click", function (e) {
    var item = e.target.closest(".ac");
    if (item) apply({ accent: item.getAttribute("data-ac") });
  });

  // 拖动时只改数字，松手才写盘——不然一次拖动会打十几个补丁
  el.opRng.addEventListener("input", function () {
    el.opVal.textContent = el.opRng.value + "%";
  });
  el.opRng.addEventListener("change", function () {
    apply({ opacity: parseInt(el.opRng.value, 10) });
  });

  /* ---- 绑定：行为 ---- */

  el.wsCards.addEventListener("click", function (e) {
    var card = e.target.closest(".ws");
    if (!card || card.classList.contains("off")) return;
    var w = card.getAttribute("data-w");
    // 弹性那张卡不是「第四种形态」，是个开关：它跟哪几天上班互不相干，
    // 双休也能弹性。所以点它发的是 flex_start，不是 work_form。
    if (w === "flex") {
      apply({ flex_start: !(view && view.week.flex) });
      return;
    }
    apply({ work_form: w });
  });

  el.wkGrid.addEventListener("click", function (e) {
    var c = e.target.closest(".wd");
    if (!c || !view) return;
    var days = view.week.days.slice();
    var i = parseInt(c.getAttribute("data-d"), 10);
    days[i] = !days[i];
    apply({ week_days: days });
  });

  el.calGrid.addEventListener("click", function (e) {
    var c = e.target.closest("[data-iso]");
    if (!c) return;
    var iso = c.getAttribute("data-iso");
    var kind = c.getAttribute("data-kind");
    var patch = {};
    if (calMode === "holiday") {
      patch.holiday = kind === "holiday" ? { del: iso } : { add: iso };
    } else {
      patch.makeup = kind === "makeup" ? { del: iso } : { add: iso };
    }
    apply(patch);
  });

  HK_KEYS.forEach(function (row) {
    var node = el[row[0]];
    if (!node) return;
    node.setAttribute("data-patch", row[1]);
    node.addEventListener("click", function () {
      if (recording) recording.classList.remove("rec");
      recording = node;
      node.classList.add("rec");
      el.keyHint.textContent = "现在按下组合键…（Esc 取消）";
    });
  });

  el.swBoot.addEventListener("click", function () {
    apply({ autostart: !(view && view.autostart) });
  });
  el.swSilent.addEventListener("click", function () {
    apply({ silent_start: !(view && view.silent_start) });
  });
  el.swThru.addEventListener("click", function () {
    apply({ click_through: !(view && view.click_through) });
  });
  el.swIdle.addEventListener("click", function () {
    apply({ idle_detect: !(view && view.idle_detect) });
  });
  el.fIdleMin.addEventListener("change", function () {
    apply({ idle_min: parseInt(el.fIdleMin.value, 10) });
  });

  el.snapRng.addEventListener("input", function () {
    el.snapVal.textContent = el.snapRng.value === "0" ? "关" : el.snapRng.value;
  });
  el.snapRng.addEventListener("change", function () {
    apply({ snap_px: parseInt(el.snapRng.value, 10) });
  });

  el.mmChips.addEventListener("click", function (e) {
    var item = e.target.closest(".chip");
    if (item) apply({ monitor_primary: item.getAttribute("data-mm") === "primary" });
  });

  el.segClose.addEventListener("click", function (e) {
    var b = e.target.closest("button");
    if (b) apply({ close_action: b.getAttribute("data-v") });
  });

  /* ---- 绑定：通知 ---- */

  el.swEnd.addEventListener("click", function () {
    var on = view && view.notify.end !== "off";
    /* 关掉就是 off；打开时恢复成「只报金额」——那是默认值，不是记忆值 */
    apply({ notify_end: on ? "off" : "amount" });
  });

  el.endChips.addEventListener("click", function (e) {
    var c = e.target.closest(".chip");
    if (c) apply({ notify_end: c.getAttribute("data-v") });
  });

  el.swLunch.addEventListener("click", function () {
    apply({ notify_lunch: !(view && view.notify.lunch) });
  });
  el.lunchChips.addEventListener("click", function (e) {
    var c = e.target.closest(".chip");
    if (!c) return;
    /* 点「提前 N 分钟」顺手把开关打开：点了却没反应是最气人的那种交互 */
    apply({ notify_lunch_before: parseInt(c.getAttribute("data-v"), 10), notify_lunch: true });
  });

  el.swSit.addEventListener("click", function () {
    apply({ notify_sit: !(view && view.notify.sit) });
  });
  el.segSit.addEventListener("click", function (e) {
    var b = e.target.closest("button");
    if (b) apply({ notify_sit_min: parseInt(b.getAttribute("data-v"), 10), notify_sit: true });
  });

  el.swDnd.addEventListener("click", function () {
    apply({ notify_dnd: !(view && view.notify.dnd) });
  });
  el.dndFrom.addEventListener("change", function () {
    apply({ notify_dnd_from: el.dndFrom.value });
  });
  el.dndTo.addEventListener("change", function () {
    apply({ notify_dnd_to: el.dndTo.value });
  });

  /* ---- 绑定：清除数据 ---- */

  var pendingClear = null;

  function askClear(kind, what) {
    pendingClear = kind;
    el.clrWhat.textContent = what;
    el.clrWord.value = "";
    el.clrBox.hidden = false;
    el.clrWord.focus();
  }

  el.clrToday.addEventListener("click", function () {
    askClear("today", "清除今天的暂停与午休加班记录（钱按当前作息重算）");
  });
  el.clrMonth.addEventListener("click", function () {
    askClear("month", "清除本月记录，含本月标过的请假");
  });
  el.clrAll.addEventListener("click", function () {
    askClear("all", "清空全部数据：薪资、排班、请假、记录。外观设置会保留");
  });
  el.clrCancel.addEventListener("click", function () {
    pendingClear = null;
    el.clrBox.hidden = true;
  });
  el.clrGo.addEventListener("click", function () {
    /* 这一关只在前端：后端不做二次确认，但也只认 today / month / all 三个值。
       写成大写比较，省得用户输 delete 就被当成取消。 */
    if (!pendingClear) return;
    if (el.clrWord.value.trim().toUpperCase() !== "DELETE") {
      el.clrWord.value = "";
      el.clrWord.placeholder = "要大写 DELETE";
      return;
    }
    var p = { clear: pendingClear };
    pendingClear = null;
    el.clrBox.hidden = true;
    apply(p);
  });

  /* ---- 绑定：其他 ---- */

  document.addEventListener("click", function (e) {
    var sw = e.target.closest("[data-den]");
    if (sw) {
      sw.classList.toggle("on");
      apply({ density: density() });
      return;
    }
    var chip = e.target.closest("#eqChips .chip");
    if (chip) {
      apply({
        eq_name: chip.getAttribute("data-eq"),
        eq_price_yuan: parseFloat(chip.getAttribute("data-price"))
      });
      return;
    }
    var lk = e.target.closest("#leaveChips .chip");
    if (lk) apply({ leave_kind: lk.getAttribute("data-lk") });
  });

  document.addEventListener("click", function (e) {
    var bm = e.target.closest("#bonusChips .chip");
    if (bm) apply({ bonus_mode: bm.getAttribute("data-bm") });
    var mv = e.target.closest("#bQuick .chip");
    if (mv) apply({ bonus_months: parseFloat(mv.getAttribute("data-mv")) });
  });

  el.fEqPrice.addEventListener("change", function () {
    var v = parseFloat(el.fEqPrice.value);
    if (isFinite(v)) apply({ eq_price_yuan: v });
  });
  el.fEqGoal.addEventListener("change", function () {
    var v = parseInt(el.fEqGoal.value, 10);
    if (isFinite(v)) apply({ eq_goal_cups: v });
  });
  el.fEqDay.addEventListener("change", function () {
    var v = parseInt(el.fEqDay.value, 10);
    if (isFinite(v)) apply({ eq_day_cups: v });
  });

  el.fLeaveLeft.addEventListener("change", function () {
    var v = parseFloat(el.fLeaveLeft.value);
    if (isFinite(v)) apply({ leave_left_days: v });
  });

  // 「今天请假」是开关语义，不读自己的 class——以服务端返回的状态为准
  el.swLeaveToday.addEventListener("click", function () {
    apply({ today_leave: !(view && view.today_leave) });
  });

  /* ---- 启动 ---- */

  if (!invoke) {
    document.body.innerHTML =
      '<div style="padding:40px;color:#7A838F;font-size:13px">后端没起来，设置读不到。</div>';
    return;
  }
  invoke("get_settings").then(fill).catch(function (e) {
    console.error("读设置失败", e);
  });

  /* 首启引导。`first_run` 读的是「启动那一刻配置文件在不在」——
     所以它只会在新装那一次为真，升级上来的老用户不会被弹一次。
     点「开始」发一个空补丁：空补丁也会走一遍「合并 → 校验 → 原子写」，
     正好把配置落下来，Rust 那边顺手让悬浮窗露脸。 */
  invoke("first_run")
    .then(function (on) {
      if (!on || !el.welcome) return;
      el.welcome.hidden = false;
      // 光标直接落在月薪上，并且全选：用户想干的第一件事就是把它替换掉，
      // 而不是先找地方点一下。光标放这儿，「填两个数」才不是一句空话。
      // 延一帧：`fill()` 可能还在路上，它会把 value 重写一遍、顺手清掉选区。
      setTimeout(function () {
        var m = document.getElementById("fMonth");
        if (m) {
          m.focus();
          m.select();
        }
      }, 80);
    })
    .catch(function () {});

  if (el.welcomeGo) {
    el.welcomeGo.addEventListener("click", function () {
      // 光标还在某个输入框里时，change 事件可能还没发出去；先让它失焦
      if (document.activeElement && document.activeElement.blur) {
        document.activeElement.blur();
      }
      apply({});
      el.welcome.hidden = true;
      // 悬停焦点回到第一组，让用户接着看自己填的那些数
      var pay = document.querySelector('.nav .i[data-g="pay"]');
      if (pay) pay.click();
    });
  }

  /* ---- 检查更新 ----

     真状态机（原型 07 章 + 05-E）：点「检查更新」走 解析地址 → 拉清单 →
     验签 → 比版本。网络请求在前端（fetch，WebView2 自带 HTTP 栈），
     验签和版本比较在 Rust（update.rs），前端只负责展示。

     清单地址：GitHub Releases 里放的 manifest 文件。
     后端 release.yml 现在只发一份 manifest（channel=stable），
     所以稳定版/测试版暂时都指向同一个 `latest/download/manifest.json`。
     等以后真的分通道（beta manifest 单独发），再把 beta 的 URL 换成
     对应的资产名。 */
  var UPDATE_MANIFEST_URL = "https://github.com/zoroli1988/paypulse/releases/latest/download/manifest.json";

  function updShow(html) {
    if (el.updBody) el.updBody.innerHTML = html;
  }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  function updIdle(msg) {
    updShow(
      '<div class="upd-idle"><span class="dot" style="background:#35E07F"></span>' +
      '<b>' + esc(el.kVer ? el.kVer.textContent : "?") + '</b> ' +
      esc(msg || "已是最新版本") +
      '<span style="margin-left:auto;font-size:10px;color:#5F6874">上次检查 刚才</span></div>'
    );
  }

  function updError(msg) {
    updShow(
      '<div class="upd-idle err"><span class="dot" style="background:#FF8A8A"></span>' +
      '<b>检查失败</b> ' + esc(msg) +
      '<span style="margin-left:auto;font-size:10px;color:#5F6874">上次检查 刚才</span></div>'
    );
  }

  function updChecking() {
    updShow(
      '<div style="display:flex;align-items:center;gap:9px;font-size:11px;color:#B4BAC2;margin-bottom:9px">' +
      '<span class="spin"></span>正在连接更新服务…</div>' +
      '<div class="chk">' +
      '<div class="wait" data-s="1">解析更新服务地址</div>' +
      '<div class="wait" data-s="2">拉取清单 manifest</div>' +
      '<div class="wait" data-s="3">校验 Ed25519 签名</div>' +
      '<div class="wait" data-s="4">比对语义化版本</div>' +
      '</div>'
    );
  }
  function tickSteps(i) {
    var steps = document.querySelectorAll("#updBody .chk div");
    for (var k = 0; k < i && k < steps.length; k++) steps[k].className = "ok";
  }

  function updAvailable(r) {
    var mb = (r.size / 1024 / 1024).toFixed(1);
    updShow(
      '<div class="upd-idle" style="background:rgba(53,224,127,.09);border-color:rgba(53,224,127,.32)">' +
      '<span class="dot" style="background:#35E07F"></span><b>' + esc(r.latest) + '</b> 有新版本可用' +
      '<span style="margin-left:auto;font-size:10px;color:#5F6874">' + mb + ' MB</span></div>' +
      (r.forced
        ? '<div class="upd-idle err" style="margin-top:6px"><span class="dot" style="background:#FF8A8A"></span>' +
          '<b>必须更新</b> 当前版本已停用，请先更新后再继续使用</div>'
        : "") +
      '<div style="display:flex;gap:8px;margin-top:10px">' +
      '<button class="btn btn-p" id="btnDl" style="height:28px;font-size:11px;padding:0 14px">下载新版本</button>' +
      (r.forced ? "" : '<button class="btn btn-g" id="btnLater" style="height:28px;font-size:11px;padding:0 14px">稍后提醒</button>') +
      '</div>'
    );
    var dl = document.getElementById("btnDl");
    if (dl) dl.addEventListener("click", function () {
      // 下载走系统默认浏览器 / 下载器，不在这里自己接管二进制下载
      window.open(r.url, "_blank");
    });
    var later = document.getElementById("btnLater");
    if (later) later.addEventListener("click", function () { updIdle(); });
  }

  if (el.btnCheck) {
    el.btnCheck.addEventListener("click", function () {
      var btn = this;
      btn.disabled = true;
      btn.textContent = "检查中…";
      updChecking();
      var seq = [350, 700, 1050];
      seq.forEach(function (t, i) { setTimeout(function () { tickSteps(i + 1); }, t); });

      // 通道目前只发一份 manifest（后端 release.yml 里 channel=stable），
      // 稳定版/测试版走同一个地址；等 beta 分开发时再按通道拼 URL。
      var url = UPDATE_MANIFEST_URL;

      fetch(url, { cache: "no-store" })
        .then(function (resp) {
          if (!resp.ok) throw new Error("HTTP " + resp.status);
          return resp.text();
        })
        .then(function (text) {
          tickSteps(3); // 拉清单完成
          return invoke("check_update", { manifestJson: text });
        })
        .then(function (r) {
          tickSteps(4); // 验签 + 比版本（Rust 侧一起做完）
          setTimeout(function () {
            btn.textContent = "检查更新";
            btn.disabled = false;
            if (r.state === "latest") updIdle(r.message);
            else if (r.state === "available") updAvailable(r);
            else updError(r.message || "未知错误");
            if (el.updSub) el.updSub.textContent = "刚刚检查 · 通道 " + (el.updChan ? el.updChan.value : "稳定版");
          }, 350);
        })
        .catch(function (e) {
          btn.textContent = "检查更新";
          btn.disabled = false;
          updError(e && e.message ? e.message : "网络错误，请稍后重试");
        });
    });
  }

  if (el.updChan) {
    el.updChan.addEventListener("change", function () {
      if (el.updSub) el.updSub.textContent = "上次检查：从未 · 通道 " + this.value;
    });
  }
})();
