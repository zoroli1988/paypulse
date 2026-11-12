//! 首次启动的四步引导窗口。
//!
//! ## 为什么单独开一扇窗，而不是设置页顶部的一条横幅
//!
//! 上一版做成了设置窗口里的一条绿色横幅（`.welcome`），等于把原型的「03 首次引导」
//! 整章丢了。原型要的是四步独立窗口 560×640，每步只问一件事：
//!
//!   1. 你的工资 —— 填月薪、选口径，当场看到「日薪 / 时薪 / 每秒」
//!   2. 上班节奏 —— 排班形态 + 上下班 + 午休，当场看到计薪工时
//!   3. 它出现在这里 —— 拖动预览定位置、选尺寸、开机自启
//!   4. 可以开工了 —— 汇总 + 快捷键 + 「数据只存本机」的提醒
//!
//! 那三条原则是：①所有字段预填人均值（用户只做「确认」不做「填写」）
//! ②不要求注册登录 ③第 3 步给视觉预览。横幅一条都做不到——它只是把用户
//! 丢进七组设置里自己找月薪在哪。
//!
//! ## 窗口生命周期
//!
//! 和 menu / detail / toast / settings 一样：启动时 `prewarm()` 建好藏起来，
//! 用的时候只 `show()`；`CloseRequested` 一律拦成 `hide()`，**永不销毁**。
//!
//! 这条不是洁癖：懒创建的 WebView2 窗口 `show()` 出来是空白的（首帧还没渲染），
//! 而且同名标签在旧窗口没清干净时重建会把主线程卡死——v0.1.1 修的「点设置假死」
//! 就是这个问题（见 settings_win::build 的注释）。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "onboard";

/// 原型「02 状态与流程」的窗口规格表写的是 560×640，但「03 首次引导」
/// 里那四张高保真稿是按内容撑开的——实测四步里最高的第 4 步约 470 逻辑高。
///
/// 按 640 做出来，内容全挤在上半屏、中间空掉近 200px，和原型的观感完全不是
/// 一回事（原型的四张图像是「一屏能看完的一件事」，不是一扇空窗口）。
/// 所以这里取内容高度 + 一点余量：500 让最高的第 4 步也还剩 48px 呼吸。
///
/// 要严格按规格表的 640，把这两个数改回去就行——版式不会坏，只是会空。
const ONBOARD_W: f64 = 560.0;
const ONBOARD_H: f64 = 500.0;

/// 打开引导窗。首启时由 `setup()` 自动调，也可以在托盘/设置里手动重看。
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    let win = if let Some(w) = app.get_webview_window(LABEL) {
        w
    } else {
        // 正常情况启动时就预热好了，这里只该拿到现成的。
        // 万一没建好（比如预热失败），退回去现建一个——内容可能空白，但至少能用。
        crate::log::w("引导窗口：没预热，现建（内容可能空白）");
        build(app)?
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

pub fn close(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

/// 启动时把引导窗建好藏着。
///
/// 只在 fresh 时才有必要预热（非首启永远不弹），但这里无条件预热：
/// 一是「首启」这个状态在 `setup()` 里才知道，等知道的时候再建就晚了
/// （第一帧是空白）；二是用户可能从设置里点「重看引导」。
pub fn prewarm(app: &AppHandle) {
    match build(app) {
        Ok(_w) => crate::log::w("引导窗口已预热（藏起）"),
        Err(e) => crate::log::w(&format!("引导窗口预热失败：{e}")),
    }
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("onboard.html".into()))
        .title("首次引导 · 牛马薪动了！")
        .inner_size(ONBOARD_W, ONBOARD_H)
        // 和其他窗口一致：去掉系统标题栏（Win11 原生标题栏是浅色的，压在深色窗上
        // 像贴了张纸），窗口自己画圆角和阴影。没有标题栏就没有可抓的边框，
        // 所以尺寸锁死——「能拉」只是个拉不动的谎。
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .center()
        // 建的时候就藏着：配合 prewarm 让页面先加载，打开时只 show。
        .visible(false)
        .focused(false)
        .build()?;

    // 关闭 = 隐藏，不销毁。理由见模块注释最后一段。
    let me = w.clone();
    w.on_window_event(move |e| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
            api.prevent_close();
            let _ = me.hide();
        }
    });

    Ok(w)
}
