//! 一个最小的日志文件：出问题时能查，平时几乎不写。
//!
//! ## 为什么不能只靠 `eprintln!`
//!
//! 这是 GUI 程序（`windows-subsystem`），没有控制台。`eprintln!` 写进的那两个句柄
//! 在双击启动时是空的——**一个字都留不下来**。之前「快捷键没反应」「通知没弹」
//! 这类问题全靠猜，就是因为失败日志被扔进了虚空。
//!
//! ## 为什么不用 log + tracing
//!
//! 它们要拉一整套门面 + 订阅者 + 过滤器，为的只是写几行固定格式的文本。
//! 这里只有四种事件（启动、快捷键、自启、通知），十几行写完。
//!
//! ## 写在哪
//!
//! `%APPDATA%\com.bydota.paypulse\paypulse.log`，和 `settings.json` 同一个目录——
//! 卸载时「删除应用数据」一并清掉，不会在用户机器上留垃圾。
//! 单个文件超过 256 KB 就从头覆盖，不滚动、不压缩：这东西只在出问题时才有用。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 由外壳在启动时告诉日志该写哪儿。不调也不影响功能（只是没有日志）。
///
/// 顺手把目录建好：首次启动时 `app_config_dir()` 的目录还不存在，
/// 而 `w` 里 `OpenOptions::create(true)` 只建文件、不建父目录——
/// 不先建目录，启动那几行日志会静默丢掉，出事时正好没日志可查。
pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let _ = PATH.set(Some(dir.join("paypulse.log")));
}

/// 记一行。失败就算了——日志写不进去不该让任何功能跟着坏。
pub fn w(msg: &str) {
    let Some(Some(p)) = PATH.get() else { return };
    // 256 KB 上限：超了就从空的开始写，避免无限长
    if std::fs::metadata(p).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(p);
    }
    let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) else {
        return;
    };
    let ts = chrono::Local::now().format("%m-%d %H:%M:%S");
    let _ = writeln!(f, "[{ts}] {msg}");
}
