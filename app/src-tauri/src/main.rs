// 释放构建下不弹控制台窗口；debug 下保留，方便看 panic。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    paypulse_app::run()
}
