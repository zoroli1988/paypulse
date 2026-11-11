//! 预处理器替身：只实现 `gcc -E` 的「原样输出」。
//!
//! 为什么需要它：`windres` 编译 .rc 前要过一遍 C 预处理器，默认去调 `gcc -E`；
//! 而 rustup 的 `rust-mingw` 组件里的 gcc 是个空壳（旁边还放了 GCC-WARNING.txt）。
//! 于是 windres 报 "preprocessing failed"。
//!
//! 本项目生成的 resource.rc 里**没有 `#include`、没有宏**，只有 VERSIONINFO、
//! ICON 和一段 manifest 字符串，所以「输入等于输出」就是正确的预处理结果。
//!
//! 两个关键细节：
//!   1. windres 多数情况下**不给 -o**，它把预处理器的 stdout 抓走当下游输入。
//!      所以没有 -o 时必须写 stdout，写文件是给自己看的补充路径。
//!   2. 参数里混着 `-E`、`-xc-header`、`-DRC_INVOKED`、`-I<dir>` 这类纯开关，
//!      要跳过；但 `-I foo` 这种分开写的形式里，foo 是目录不是输入文件。
//!
//! 注意：这是构建环境补丁，不是产品代码。真正的 Windows 开发机上应当用
//! MSVC 工具链（`llvm-rc` / `rc.exe`），根本不需要这两个替身。

use std::io::Write;
use std::path::PathBuf;

/// 这些开关后面跟一个**非输入文件**的值，需要连值一起跳过。
const TAKES_VALUE: &[&str] = &[
    "-I",
    "-D",
    "-U",
    "-include",
    "-imacros",
    "-isystem",
    "-iquote",
    "-idirafter",
    "-x",
    "-MF",
    "-MT",
    "-MQ",
    "-arch",
    "-std",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if std::env::var_os("GCC_SHIM_DEBUG").is_some() {
        eprintln!("gcc-shim args: {args:?}");
    }

    let mut out: Option<PathBuf> = None;
    let mut inputs: Vec<PathBuf> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];

        // -o 的四种写法：`-o path` / `--output path` / `--output=path` / `-o=path`
        if a == "-o" || a == "--output" {
            if let Some(v) = args.get(i + 1) {
                out = Some(PathBuf::from(v));
            }
            i += 2;
            continue;
        }
        if let Some(v) = a
            .strip_prefix("--output=")
            .or_else(|| a.strip_prefix("-o="))
        {
            out = Some(PathBuf::from(v));
            i += 1;
            continue;
        }

        // 带值开关：值一起吃掉
        if TAKES_VALUE.contains(&a.as_str()) {
            i += 2;
            continue;
        }

        // 其余 `-` 开头的都是纯开关，跳过
        if a.starts_with('-') {
            i += 1;
            continue;
        }

        inputs.push(PathBuf::from(a));
        i += 1;
    }

    let Some(input) = inputs.last() else {
        eprintln!("gcc-shim: 没找到输入文件；args={args:?}");
        std::process::exit(1);
    };

    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("gcc-shim: 读 {} 失败: {e}", input.display());
            std::process::exit(1);
        }
    };

    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &bytes) {
                eprintln!("gcc-shim: 写 {} 失败: {e}", path.display());
                std::process::exit(1);
            }
        }
        None => {
            // 没有 -o：仿照真实预处理器的行为写 stdout
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            if let Err(e) = lock.write_all(&bytes).and_then(|_| lock.flush()) {
                eprintln!("gcc-shim: 写 stdout 失败: {e}");
                std::process::exit(1);
            }
        }
    }
}
