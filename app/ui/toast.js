/* 通知条。内容全部由 Rust 推过来——这一行 JS 里没有算术，也没有时间格式化。

   图标用字符而不是 SVG：一共三种语气、三个符号，为它们各写一段 path
   不值得。选中文字符在两种字体下都能居中，比 SVG 还稳。 */
(function () {
  "use strict";

  var tauri = window.__TAURI__;
  /* 这扇窗不提供右键菜单。不拦的话 WebView2 会弹它自己的网页菜单
     （「重新加载」「检查」那一套）——一个桌面小工具上冒出这个很莫名其妙。
     悬浮窗是例外：那边右键要弹自家的菜单，处理在 hud.js。 */
  document.addEventListener("contextmenu", function (e) { e.preventDefault(); }, true);

  var invoke = tauri && tauri.core && tauri.core.invoke;
  var listen = tauri && tauri.event && tauri.event.listen;

  var t = document.getElementById("t");
  var ic = document.getElementById("ic");
  var ti = document.getElementById("ti");
  var tb = document.getElementById("tb");

  var GLYPH = { ok: "✓", warn: "!", info: "i" };
  var FALLBACK = "¥";

  function render(p) {
    var tone = p.tone || "info";
    ti.textContent = p.title || "";
    tb.textContent = p.body || "";
    t.dataset.tone = tone;
    ic.textContent = GLYPH[tone] || FALLBACK;
    /* 重播一次入场动画。不重播的话，连续两条通知看起来像一条没变过。 */
    t.style.animation = "none";
    void t.offsetWidth;
    t.style.animation = "";
  }

  if (listen) {
    listen("toast", function (e) {
      if (e && e.payload) render(e.payload);
    });
  }

  /* 窗口是「第一次要弹的时候」才建的，`emit` 到达时这一页往往还没跑起来，
     那一发就丢了——表现是窗口弹出来但整块透明，看起来像「没弹」。
     所以页面起来后自己捞一次最近那条；之后的新通知仍然走上面的事件。 */
  if (invoke) {
    invoke("toast_pull")
      .then(function (p) {
        if (p) render(p);
      })
      .catch(function () {});
  }

  /* 点一下就关。整块都是热区——通知条上不该出现「关闭按钮在哪里」这种问题。 */
  document.addEventListener("click", function () {
    if (invoke) invoke("toast_dismiss").catch(function () {});
  });
})();
