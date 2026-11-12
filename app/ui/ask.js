/* 「时区变了」询问窗的前端。
 *
 * 这个文件里**没有一行算术**——四个时刻（当地 09:00 / 北京 08:00 / 北京 09:00 /
 * 当地 10:00）全部由 Rust 算好送过来。理由和 settings.js 一样：
 * 「UTC+9 的 09:00 换成北京几点」正是最容易写反的那一步，而写反了，
 * 用户会照着一句错话做选择，然后一整天的账都按错的那边算。
 *
 * 唯一的判断是「点了哪个按钮」，两个都是命令，不是本地状态。
 */
(function () {
  "use strict";

  document.addEventListener("contextmenu", function (e) { e.preventDefault(); }, true);

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  var listen = window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen;

  var el = {};
  ["closeBtn", "sub", "tzNow", "tzCompany", "vLocal", "vCompany",
   "pickLocal", "pickCompany"].forEach(function (id) {
    el[id] = document.getElementById(id);
  });

  /* 画一屏。字段全部来自 Rust 的 AskView，这里只负责往格子里塞字。 */
  function fill(v) {
    if (!v) return;
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

  function answer(mode) {
    if (!invoke) return;
    /* 先禁用按钮再发命令：落盘 + 关窗这段时间里，用户可能又点一下另一颗。
       两次命令的先后顺序无法保证，最后写进去的可能是他本来没选的那个。 */
    el.pickLocal.disabled = true;
    el.pickCompany.disabled = true;
    invoke("ask_answer", { mode: mode }).catch(function (e) {
      console.error("ask_answer 失败", e);
      el.pickLocal.disabled = false;
      el.pickCompany.disabled = false;
    });
  }

  el.pickLocal.addEventListener("click", function () { answer("local"); });
  el.pickCompany.addEventListener("click", function () { answer("company"); });

  /* × 等于「保持现状」：不选也是答案，按本地时间继续，并记下这个偏移，
     免得下一拍又弹回来。落盘那一步在 Rust 的 CloseRequested 里做—— */
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
})();
