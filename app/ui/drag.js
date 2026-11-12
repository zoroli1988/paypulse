/* 无边框窗口的「抓住标题栏拖窗口」。设置 / 询问 / 引导三扇窗共用这一份。
 *
 * ## 为什么不用 `data-tauri-drag-region`
 *
 * Tauri 那条内置路径只认「按下的目标元素**自己**带这个属性」
 * （`drag.js` 里那句 `el === composedPath[0]`）。标题栏 `.bar` 里压着三个圆点、
 * 一段标题文字、一个关闭按钮，于是：
 *
 *   - 按在 `.bar` 的空白处 → 目标就是 `.bar`，属性在，内置路径生效；
 *   - 按在标题文字上 → 目标是那个 `<span>`，属性不在它身上，内置路径不生效。
 *
 * 早先为了救第二种情况，在 `.bar` 上又接了一个 `mousedown` 自己调 `startDragging`。
 * 结果反而把第一种情况弄坏了：空白处的目标既命中内置路径、又冒泡到 `.bar`
 * 触发我们那个监听，同一次按下发起**两次**拖动。Windows 这边 `start_dragging`
 * 一进去先 `ReleaseCapture()`，第二次调用把第一次刚开始的拖动直接掐断——
 * 表现就是「按在空白处拖不动，按在文字上反而能拖」。
 *
 * 所以现在只有一条路：去掉元素上的 `data-tauri-drag-region`，统一由这里接管。
 * 顺带解决另一个隐患——内置路径对标题栏双击会发 `internal_toggle_maximize`，
 * 而这几扇窗都是 `resizable(false)` 的固定尺寸窗，被最大化会很难看。
 *
 * ## 用法
 *
 *   <script src="drag.js"></script>
 *   <script>bindBarDrag();</script>
 *
 * 想换选择器就 `bindBarDrag("#titlebar")`。
 */
(function () {
  "use strict";

  /* 窗口对象在几个 Tauri 版本里叫法不一样，能拿到哪个用哪个。 */
  function currentWindow() {
    var t = window.__TAURI__;
    if (!t || !t.window) return null;
    if (t.window.getCurrentWindow) return t.window.getCurrentWindow();
    if (t.window.getCurrent) return t.window.getCurrent();
    return null;
  }

  window.bindBarDrag = function (selector) {
    var bar = document.querySelector(selector || ".bar");
    var w = currentWindow();
    if (!bar || !w) return;

    bar.addEventListener("mousedown", function (e) {
      if (e.button !== 0) return;              // 只认左键
      if (e.detail > 2) return;                // 三击及以后不当拖动
      if (e.target.closest("button, a, input, select, textarea")) return;

      // 拦下默认行为，否则按住会变成「框选标题文字」。
      // stopPropagation 是给页面自己的其它 mousedown 监听留清净：
      // 拖动这件事一旦判定成立，就不该再有人来分一杯羹。
      e.preventDefault();
      e.stopPropagation();

      w.startDragging().catch(function () {});
    });
  };
})();
