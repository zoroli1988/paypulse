//! `as` 的替身：把 dlltool 传给汇编器的参数转给 MinGW 的 gcc。
//!
//! 为什么需要它：Windows GNU 工具链上，`windows-link` 这类 crate 用
//! `#[link(kind = "raw-dylib")]`，rustc 会在编译期调 `dlltool` 生成导入库，
//! 而 `dlltool` 又要调 `as` 汇编中间产物。rustup 的 `rust-mingw` 组件里
//! **只给了 dlltool 和 ld，没有 as**，于是报 `CreateProcess` 失败。
//!
//! 唯一的差别是 gcc 不认 `--64`（`-m64` 才行），所以这里滤掉它再转发。
//! 这是构建环境的问题，不是产品代码——它也永远不该进安装包。

use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--64" && a != "-64")
        .collect();

    let backend =
        std::env::var("PAYPULSE_AS_BACKEND").unwrap_or_else(|_| "x86_64-w64-mingw32-gcc".to_string());

    let status = Command::new(&backend).args(&args).status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("as-shim: 起不来后端 {backend}: {e}");
            std::process::exit(1);
        }
    }
}
