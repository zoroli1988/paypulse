/* 询问窗的前端。两个提问共用这扇窗，靠 Rust 传来的 kind 切屏。
 *
 * 这个文件里**没有一行算术**——
 *   时区那屏的四个时刻（当地 09:00 / 北京 08:00 / 北京 09:00 / 当地 10:00）、
 *   补记那屏的两个数（班次内多久、折多少钱），全部由 Rust 算好送过来。
 * 理由和 settings.js 一样：换算正是最容易写反的那一步，而写反了，
 * 用户会照着一句错话做选择，然后一整天的账都按错的那边算。
 *
 * 唯一的判断是「点了哪个按钮」，全是命令，不是本地状态。
 */
(function () {
  "use strict";

  document.addEventListener("contextmenu", function (e) { e.preventDefault(); }, true);

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  var listen = window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen;

  var el = {};
  ["barTitle", "closeBtn", "paneTz", "paneFill",
   "sub", "tzNow", "tzCompany", "vLocal", "vCompany", "pickLocal", "pickCompany",
   "fillSub", "fillCovered", "fillAmount", "pickSkip", "pickCount"
  ].forEach(function (id) {
    el[id] = document.getElementById(id);
  });

  /* 这一轮问的是哪件事。点按钮时要按它决定发哪个命令。 */
  var kind = "tz";
  /* 补记那屏要把「上次退出时刻」原样带回去（点「不算」时补暂停用）。
     Rust 给什么就传什么，前端不自己造时间戳。 */
  var fillFrom = 0;

  function pane(which) {
    kind = which;
    el.paneTz.hidden = which !== "tz";
    el.paneFill.hidden = which !== "fill";
    el.barTitle.textContent = which === "tz" ? "时区变了" : "好久不见";
    el.closeBtn.title = which === "tz" ? "关掉（按本地时间）" : "关掉（算进去）";
  }

  /* 画一屏。字段全部来自 Rust，这里只负责往格子里塞字。 */
  function fill(v) {
    if (!v) return;
    if (v.kind === "fill") {
      pane("fill");
      fillFrom = v.from_ts || 0;
      el.fillSub.textContent =
        "上次退出是 " + v.from_label + "，到现在 " + v.gap_label + "。";
      el.fillCovered.textContent = v.covered_label;
      el.fillAmount.textContent = v.amount_label;
      return;
    }
    pane("tz");
    el.tzNow.textContent = v.now_label;
    el.tzCompany.textContent = "UTC+8";
    el.sub.textContent =
      "系统时区是 " + v.now_label + "，比上次的 " + v.prev_label + " 差 " + v.diff_label + "。";
    el.vLocal.textContent = "当地 " + v.local_start + " = 北京 " + v.local_start_china;
    el.vCompany.textContent = "北京 " + v.company_start + " = 当地 " + v.company_start_local;
  }

  /* 事件比监听器先到是常态（窗口刚 show 出来时页面可能还在加载），
     所以除了监听，页面起来自己也去捞一次——两条路任一条通就够。 */
  if (listen) {
    listen("ask", function (e) { fill(e.payload); });
  }
  if (invoke) {
    invoke("ask_pull").then(fill).catch(function (e) {
      console.error("ask_pull 失败", e);
    });
  }

  var busy = false;

  /* 先禁用所有按钮再发命令：落盘 + 关窗这段时间里，用户可能又点一下另一颗。
     两次命令的先后顺序无法保证，最后写进去的可能是他本来没选的那个。 */
  function send(cmd, args) {
    if (!invoke || busy) return;
    busy = true;
    var all = [el.pickLocal, el.pickCompany, el.pickSkip, el.pickCount];
    all.forEach(function (b) { b.disabled = true; });
    invoke(cmd, args).catch(function (e) {
      console.error(cmd + " 失败", e);
      busy = false;
      all.forEach(function (b) { b.disabled = false; });
    });
  }

  el.pickLocal.addEventListener("click", function () { send("ask_answer", { mode: "local" }); });
  el.pickCompany.addEventListener("click", function () { send("ask_answer", { mode: "company" }); });

  el.pickSkip.addEventListener("click", function () {
    send("fill_answer", { mode: "skip", from: fillFrom });
  });
  el.pickCount.addEventListener("click", function () {
    send("fill_answer", { mode: "count", from: fillFrom });
  });

  /* × 等于「保持现状」：不选也是答案。
     时区那屏按本地继续并记下这个偏移；补记那屏就是「算进去」（不动金额）。
     两边都要记一笔，否则下一拍又弹回来。落脚点在 Rust 的 CloseRequested 里。 */
  el.closeBtn.addEventListener("click", function () {
    if (window.__TAURI__ && window.__TAURI__.window) {
      window.__TAURI__.window.getCurrentWindow().close();
    }
  });

  /* Esc = 关掉，和引导窗一致。这扇窗会抢焦点（要人做选择），
     键盘用户第一反应就是按 Esc——不接这一下，他会觉得窗卡住了。 */
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") el.closeBtn.click();
  });

  /* 标题栏拖动（含标题文字与圆点）。实现在 drag.js——
     之前这扇窗的 `.bar` 上挂着 `data-tauri-drag-region`，那条内置路径只认
     「按下的目标元素自己带属性」，于是按在标题文字上拖不动。 */
  if (window.bindBarDrag) window.bindBarDrag();
})();
