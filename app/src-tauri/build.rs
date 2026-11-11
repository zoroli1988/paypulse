//! 构建脚本。
//!
//! 除了 Tauri 的标准步骤，这里还多做一件事：**给测试产物也嵌一份 manifest**。
//!
//! 为什么必须做：`tao` / `muda` 这些窗口库会导入 `comctl32.dll` 的
//! `SetWindowSubclass`、`TaskDialogIndirect`、`RemoveWindowSubclass`、
//! `DefSubclassProc`，这几个函数**只存在于 comctl32 v6**，而 v6 只有在
//! 程序嵌了 `Microsoft.Windows.Common-Controls 6.0.0.0` 清单时才被激活；
//! 否则加载到的是 5.82，缺导出。
//!
//! `tauri_build::build()` 只给 bin 嵌，不给 test 嵌。于是 `cargo test` 的
//! 测试 exe 一启动就被加载器打死，报 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND`
//! ——连一个测试函数都执行不到，看起来像代码坏了，其实只是少一份清单。
//!
//! 只在 `windows-gnu` 上做：MSVC 目标由 rustc 自己带默认清单。

use std::path::PathBuf;
use std::process::Command;

/// 清单内容与 `tauri_build` 自己生成的那份**完全一致**。
///
/// 这不是抄错，是刻意的：bin 同时链接 tauri 的 `libresource.a` 和这份对象，
/// 两者内容一样，所以无论加载器最后挑了哪一份，行为都不会有任何差别。
/// （DPI 感知不在清单里，是 Tauri 运行时调 `SetProcessDpiAwarenessContext` 做的。）
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    tauri_build::build();

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        embed_test_manifest();
    }
}

fn embed_test_manifest() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = out_dir.join("test.manifest");
    let rc = out_dir.join("test-manifest.rc");
    let obj = out_dir.join("test-manifest.o");

    if std::fs::write(&manifest, MANIFEST).is_err() {
        println!("cargo:warning=写 test.manifest 失败，测试产物将缺少 comctl32 v6 清单");
        return;
    }
    // 资源脚本：1 = 资源 id（加载器只认 CREATEPROCESS_MANIFEST_RESOURCE_ID = 1），
    // 24 = RT_MANIFEST
    let rc_src = format!("1 24 \"{}\"\n", manifest.display().to_string().replace('\\', "/"));
    if std::fs::write(&rc, rc_src).is_err() {
        println!("cargo:warning=写 test-manifest.rc 失败");
        return;
    }

    let windres = std::env::var("PAYPULSE_WINDRES").unwrap_or_else(|_| "windres".to_string());
    let status = Command::new(&windres)
        .arg("-O")
        .arg("coff")
        // 语言标成中立（0）。tauri 那份是 1033，资源 id 又都得是 1，
        // 语言是唯一还能区分的地方——不区分就会被 lld 判成
        // "duplicate resource: type MANIFEST (ID 24)/name ID 1/language 1033"。
        .arg("--lang")
        .arg("0")
        .arg("-i")
        .arg(&rc)
        .arg("-o")
        .arg(&obj)
        .status();

    match status {
        Ok(s) if s.success() => {
            // 注意：这里只能用通用形式。`rustc-link-arg-tests` 只作用于
            // `tests/` 目录下的集成测试，管不到 lib 自带的单元测试 harness
            // （Cargo 会直接报 "does not have a test target"）。
            // 通用形式对所有「可链接目标」生效，rlib 不可链接所以不受影响。
            println!("cargo:rustc-link-arg={}", obj.display());
        }
        _ => {
            // 没有 windres 就降级：测试仍然能编，只是跑不起来。
            // 这在 README 的「三个坑」一节里有说明。
            println!(
                "cargo:warning=找不到可用的 {windres}，跳过测试清单嵌入（cargo test 会报 0xc0000139）"
            );
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PAYPULSE_WINDRES");
}
