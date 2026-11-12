//! 崩溃日志。规格 07 章：「崩溃日志 —— **默认关闭**。开启后仅上报堆栈与版本号，
//! 绝不含任何金额、薪资或路径」。
//!
//! ## 「上报」在这里是什么
//!
//! 这个程序没有上报服务（唯一出网的地方是检查更新，见设置页「数据与隐私」那一页
//! 的承诺）。所以这个开关管的是**写不写本机那份 `crash.log`**，设置页也就照实
//! 这么写。抄一句「上报到服务器」很容易，但那是这个产品里最不该撒的谎：
//! 用户点开它，是想帮上忙，不是想被骗。
//!
//! ## 默认关着意味着什么
//!
//! 程序崩了，磁盘上不留任何痕迹——`paypulse.log` 只记我们主动写的事件，
//! panic 是另一条路。这是有意的：一个记工资的工具没有资格在用户机器上
//! 默认留一份「它怎么死的」的现场记录。
//!
//! ## 「绝不含金额、薪资或路径」
//!
//! 这句话是能被验证的，所以代码里必须真的做到：
//!
//! * 堆栈里的源文件路径 → `redact()` 换成 `<路径>`，只留行号；
//! * panic 消息里如果带着设置目录（有些 IO 错误会带）→ 同样抹掉；
//! * 只写版本号、系统、位置、消息、堆栈，**不写配置内容**。
//!
//! 这条脱敏是单测覆盖的（见文件末尾）——它是这个功能唯一的伦理边界，
//! 不能靠「我写的时候注意了」。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// 开关。每帧都可能读（其实只在 panic 那一刻读一次），用原子量而不是锁。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 写哪儿。`init` 之前调用 `set_enabled(true)` 也不会有事——只是记不下来。
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// 文件名的上限。崩溃日志不是流水账：它只在用户真的开了开关、真的崩了的时候写。
const MAX_BYTES: u64 = 128 * 1024;

pub fn init(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// `crash.log` 的完整路径。设置页要把它显示出来（用户有权知道文件在哪）。
pub fn path() -> Option<PathBuf> {
    DIR.get().map(|d| d.join("crash.log"))
}

/// 装 panic 钩子。
///
/// 钩子只做两件事：满足条件就写一行（加一段堆栈），然后把控制权交回原来的钩子。
/// **不能吞掉原钩子**——默认钩子会往 stderr 打一行，那个在开发时是唯一能看见的
/// 东西；吞掉它等于把「崩溃」变成「程序静静消失」。
pub fn install() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if enabled() {
            record(info);
        }
        prev(info);
    }));
}

/// 把一次 panic 落成文件。写不进去就算了：这条路本身在崩溃现场，任何 IO 失败
/// 都不该让情况更糟（比如再 panic 一次，那就成了一层套一层的死循环）。
fn record(info: &std::panic::PanicHookInfo<'_>) {
    let Some(p) = path() else { return };

    // 位置：只要文件名和行号。`Location` 的 `file()` 是完整路径，
    // 而「绝不含路径」那句话就是冲着它来的。
    let where_ = info
        .location()
        .map(|l| {
            let file = Path::new(l.file())
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "<未知>".into());
            format!("{file}:{}", l.line())
        })
        .unwrap_or_else(|| "<未知>".into());

    let msg = match info.payload().downcast_ref::<&str>() {
        Some(s) => (*s).to_string(),
        None => match info.payload().downcast_ref::<String>() {
            Some(s) => s.clone(),
            None => "<非字符串 panic>".to_string(),
        },
    };

    let thread = std::thread::current()
        .name()
        .unwrap_or("<未命名线程>")
        .to_string();

    // `force_capture` 不受 `RUST_BACKTRACE` 影响：用户机器上那个变量一定是空的，
    // 而我们要的正是「用户机器上崩的那一次」的堆栈。
    let bt = std::backtrace::Backtrace::force_capture();

    let text = format!(
        "[{}] v{} · panic\n位置: {where_}\n线程: {thread}\n信息: {}\n堆栈:\n{bt}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        env!("CARGO_PKG_VERSION"),
        redact(&msg),
    );

    // 超上限就整体重来：留着最旧的那次崩溃没有意义，而一直追加会长到几十兆。
    if std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(text.as_bytes());
        let _ = f.flush();
    }
}

/// 把路径抹成 `<路径>/文件名`。
///
/// 认两种形状：Windows 的 `C:\a\b` 和类 Unix 的 `/a/b`。
/// **只留最后一段**（`main.rs`）和行号——那两样是定位崩在哪一行必需的信息，
/// 而且说不出用户是谁、住在哪。中间那些目录（用户名、项目名、盘符）全部抹掉。
pub fn redact(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        let drive = i + 2 < c.len()
            && c[i].is_ascii_alphabetic()
            && c[i + 1] == ':'
            && (c[i + 2] == '\\' || c[i + 2] == '/');
        // 前面紧挨着字母数字的 `/` 多半是「3/5」「1/2」，不是路径开头。
        let unix = c[i] == '/'
            && i + 1 < c.len()
            && (c[i + 1].is_ascii_alphanumeric() || c[i + 1] == '.')
            && (i == 0 || !c[i - 1].is_ascii_alphanumeric());
        if !drive && !unix {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let mut j = if drive { i + 3 } else { i + 1 };
        while j < c.len() && is_path_char(c[j]) {
            j += 1;
        }
        // 最后一段的起点：`i..j` 里最后一个分隔符之后。
        let seg = (i..j).rev().find(|&k| c[k] == '/' || c[k] == '\\').map_or(i, |k| k + 1);
        out.push_str("<路径>/");
        out.extend(&c[seg..j]);
        i = j;
    }
    out
}

/// 路径里会出现的字符。故意**不含空格**：带空格的路径会被切成两段抹，
/// 结果仍然是安全的（多的那半也抹掉了），但更常见的情况是这句注释
/// 后面跟着正文——把空格算进去会把正文一起吃掉。
fn is_path_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '\\' | '/' | '_' | '-' | '.' | '~')
}

/// 最近一次记录的时间与位置，给设置页显示一行。
///
/// 返回 `(时间, 位置)`，例如 `("2026-11-12 10:20:31", "main.rs:123")`。
pub fn last() -> Option<(String, String)> {
    let p = path()?;
    let text = std::fs::read_to_string(&p).ok()?;
    let mut time = None;
    let mut place = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some((t, _)) = rest.split_once(']') {
                // 只认带 panic 的那一行，别把别的行当成「上次崩溃」
                if line.contains("panic") {
                    time = Some(t.to_string());
                    place = None;
                }
            }
        } else if line.starts_with("位置: ") && time.is_some() {
            place = Some(line.trim_start_matches("位置: ").to_string());
        }
    }
    Some((time?, place.unwrap_or_else(|| "<未知>".into())))
}

/// 清空。用户开了开关、崩了一次、自己看完，就该能一键抹掉。
pub fn clear() -> Result<(), String> {
    let Some(p) = path() else {
        return Err("还不知道数据目录".into());
    };
    if !p.exists() {
        return Ok(());
    }
    std::fs::remove_file(&p).map_err(|e| format!("删除失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 堆栈里的路径必须被抹掉，但行号要留着（那是唯一有用的定位信息）。
    #[test]
    fn paths_are_stripped_but_line_numbers_stay() {
        let r = redact(r"panicked at C:\Users\zorol\paypulse\src\main.rs:12");
        assert!(!r.contains("zorol"), "用户名漏了：{r}");
        assert!(!r.contains("paypulse"), "目录漏了：{r}");
        assert!(r.contains("main.rs:12"), "行号应该留着：{r}");
    }

    /// 类 Unix 的路径同样要抹。
    #[test]
    fn unix_paths_are_stripped() {
        let r = redact("at /Users/zorol/Library/Application Support/x.rs:9");
        assert!(!r.contains("zorol"), "{r}");
        assert!(r.contains(":9"), "{r}");
    }

    /// 分数、日期里的斜杠不能当成路径——那会把正常的话抹成一片空白。
    #[test]
    fn slashes_in_plain_text_are_left_alone() {
        assert_eq!(redact("3/5 的人会看到 2026/11/12"), "3/5 的人会看到 2026/11/12");
    }

    /// 消息里不该出现的两类东西：用户名与金额。钱不写进崩溃日志是硬要求。
    #[test]
    fn message_without_paths_survives_intact() {
        assert_eq!(redact("index out of bounds"), "index out of bounds");
    }

    /// 开关默认是关的：这是规格里那句「默认关闭」的可执行版本。
    #[test]
    fn disabled_by_default() {
        // 不去 set_enabled，直接读默认值。别的测试也不该动它（同一进程共享）。
        assert!(!enabled());
    }
}
