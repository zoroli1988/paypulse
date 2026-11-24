use std::path::Path;
use std::time::SystemTime;

fn newer(a: &Path, b: &Path) -> bool {
    let t = |p: &Path| p.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    t(a) > t(b)
}

fn main() {
    // payload 是编译期 include_bytes! 进去的，缺了会得到一句
    // "couldn't read payload/paypulse.exe" ——对第一次 clone 的人毫无指向性。
    // 这里提前拦住，直接告诉他跑哪条命令。
    let exe = Path::new("payload/paypulse.exe");
    let packed = Path::new("payload/paypulse.lzma");

    if !exe.exists() {
        panic!(
            "\n\n缺安装内容：{}\n\n先跑一次：\n    bash installer/build.sh --app\n\n\
             它会编译主程序、拷到 payload/、再编安装器。\n\
             （单独编主程序用：cd app && npx tauri build --no-bundle）\n",
            exe.display()
        );
    }

    // 压缩这一步放在 build.sh 里做（用 python 的 lzma，能压到 34%），
    // 这里只负责「不许用旧的」。
    //
    // 为什么要这道闸：主程序换了、压缩包还是上一次的，编出来的安装包装的
    // 是上一版 exe——而且一切都编译通过，只有用户装完才发现版本没变。
    // cargo 只看 rerun-if-changed 指的那个文件，它不会替你比这两个的时间。
    if !packed.exists() || newer(exe, packed) {
        panic!(
            "\n\npayload 没压过，或者比主程序旧：{}\n\n\
             跑一次 bash installer/build.sh 就会重新生成（它同时会重编安装器）。\n\
             只重压、不重编主程序：bash installer/build.sh  （不带 --app）\n",
            packed.display()
        );
    }

    println!("cargo:rerun-if-changed=payload/paypulse.lzma");
    println!("cargo:rerun-if-changed=payload/paypulse.exe");

    // 界面资源是 tauri::generate_context!() 在编译期从磁盘读进去、直接嵌进
    // 二进制的。这个读取发生在宏展开时，rustc 的 dep-info 看不见它，cargo
    // 自然也就不知道「改了 html」和「要重编 main.rs」之间有因果关系——
    // 结果就是改完文案 cargo build 一路绿灯，跑起来还是上一版界面，而且
    // 没有任何报错提示。这里把它挂进跟踪列表，改 html 就会触发重编。
    //
    // 盯的是整个 ui/ 目录而不是单独一个文件：frontendDist 指的是目录，
    // 以后往里放 css/字体/图片同样会被带上（cargo 对目录是递归扫描的）。
    println!("cargo:rerun-if-changed=ui");

    // 同一类坑的第二处：**权限**也是编译期从磁盘读进去的。
    // tauri_build 读 capabilities/*.json 和 tauri.conf.json，把 ACL 直接编进
    // 二进制；这两个文件改了，rustc 的 dep-info 一样看不见。后果比改 html 更
    // 阴——界面照常显示，只是某个按钮点了没反应（比如拖标题栏、点关闭），
    // 而代码里那两行调用明明写着。踩过一次：`core:default` 并不包含
    // start_dragging / close，窗口就既拖不动也关不掉，只能 Alt+F4。
    println!("cargo:rerun-if-changed=capabilities");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!(
        "cargo:warning=payload {:.2} MB（压缩后），源自 {:.2} MB 的主程序",
        packed.metadata().map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0,
        exe.metadata().map(|m| m.len()).unwrap_or(0) as f64 / 1048576.0,
    );

    tauri_build::build()
}
