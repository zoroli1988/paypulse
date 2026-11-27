//! 牛马薪动了 · 安装程序
//!
//! 为什么不用 NSIS：方案 8.2 的原型是「一张深色卡片，按钮原地变进度条，
//! 装完在同一个窗口里变成『开始使用』」。NSIS 的按钮是原生 Win32 控件，
//! 上不了色也圆不了角，进度掌握在它自己的 INSTFILES 页手里——那套皮怎么
//! 刷都刷不出原型。这里用 Tauri 自己画，界面就是 installer/ui/index.html。
//!
//! 安装器自身的行为约定：
//!   · 无参数        → 显示界面，等人点「安装」
//!   · --uninstall   → 直接走卸载（不建窗口），供「应用和功能」调用
//!
//! 装哪儿：默认 %LOCALAPPDATA%\牛马薪动了，界面上点「更改」可以换到别的盘。
//! 选目录用的是系统自己的 IFileOpenDialog（FOS_PICKFOLDERS），也就是资源管理器
//! 里「选择文件夹」那一个——不是 SHBrowseForFolder 那个 XP 味的树。
//!
//! 主程序是压着进来的：build.sh 用 LZMA 把 payload 从 5.3 MB 压到 1.8 MB，
//! 装的时候在这里解开再落盘。9.8 MB 的安装包有近 4 MB 是靠这一步省下来的。
//!
//! 手写 extern 的理由和 app/src-tauri/src/platform.rs 一样：windows-sys 的
//! raw-dylib 在 windows-gnu 工具链上要 dlltool，本机没有，一引就编不过。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Emitter};

const APP_NAME: &str = "牛马薪动了";
/// 版本只有一个来源：Cargo.toml。以前这里写死过一次，改版本时漏掉，
/// 结果安装包文件名是 0.1.7、注册表里写的是 0.1.6。
const APP_VER: &str = env!("CARGO_PKG_VERSION");
const MAIN_EXE: &str = "paypulse.exe";
const UNINST_EXE: &str = "uninstall.exe";

/// 主程序本体，已经在 build.sh 里压过（LZMA，34%）。编译期嵌进安装器，
/// 所以整个安装包就是一个 exe，不落地临时文件。
const PAYLOAD: &[u8] = include_bytes!("../payload/paypulse.lzma");

/// 解开的体积。给 Vec 预分配用，省掉几次翻倍扩容时的拷贝。
const PAYLOAD_RAW: usize = 5_600_000;

#[derive(Clone, serde::Serialize)]
struct Progress {
    pct: f32,
    step: String,
}

fn emit(app: &AppHandle, pct: f32, step: &str) {
    let _ = app.emit("install://progress", Progress { pct, step: step.to_string() });
}

// ---------------------------------------------------------------- 路径

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
}

/// 默认装到 %LOCALAPPDATA%\牛马薪动了。
///
/// 选这个而不是 Program Files，是因为 Program Files 要管理员权限，
/// 而这个软件单文件、只写自己的目录，抬权没有任何收益——只会多一个 UAC 弹窗。
fn install_dir() -> PathBuf {
    local_appdata().join(APP_NAME)
}

// ------------------------------------------------- WebView2 的数据目录

/// 安装器自己的 WebView2 数据目录名（在 %TEMP% 下）。
const WEBVIEW_TMP: &str = "paypulse-setup";

/// 老版本的数据目录：WebView2 默认按 tauri.conf.json 的 identifier 建在
/// `%LOCALAPPDATA%\com.bydota.paypulse.setup\EBWebView`，实测 113 MB。
/// 卸载要把它一起清掉，否则用户卸完了硬盘里还躺着一百多兆。
const LEGACY_WEBVIEW_DIR: &str = "com.bydota.paypulse.setup";

/// 安装器用的 WebView2 数据目录：`%TEMP%\paypulse-setup`。
///
/// 为什么不放 %LOCALAPPDATA%：这是个一次性程序，用户装完就再也不会打开它，
/// 但 WebView2 会在 %LOCALAPPDATA% 下留一整套 Chromium profile（113 MB，
/// 334 个文件）。留着没任何用处，删了也不心疼——所以干脆放临时目录，
/// 用完就扔；万一没删干净，系统清理临时文件时也会收走。
fn webview_data_dir() -> PathBuf {
    std::env::temp_dir().join(WEBVIEW_TMP)
}

/// 把 WebView2 领到临时目录去，并清掉上一次的残留。
///
/// 必须在 Tauri 建窗口**之前**调用：数据目录是 WebView2 环境创建时定的，
/// 窗口建完再设就没用了。走环境变量 `WEBVIEW2_USER_DATA_FOLDER` 而不是
/// tauri.conf 的 `dataDirectory`，是因为后者只能是相对 `localDataDir()`
/// 的路径（tauri-utils config.rs:2273），搬不到 %TEMP%。
///
/// 每次开跑都先清空：这个目录只服务于本次安装，带着上一次的 profile 启动
/// 没有任何好处（Chromium 还可能因为上次被强杀而走崩溃恢复，反而更慢）。
#[cfg(windows)]
fn prepare_webview_dir() {
    let dir = webview_data_dir();
    let _ = std::fs::remove_dir_all(&dir);
    // 万一 %TEMP% 不可写（组策略、权限被人改过），别把 WebView2 指到一个建不
    // 出来的地方——那样整个安装器会起不来。这种情况下退回 WebView2 的默认目录，
    // 多留点垃圾总比装不上强。
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir);
}

/// 起一个自己的副本来删数据目录。
///
/// 为什么不在这里直接删：WebView2 的浏览器进程是本进程的子进程，本进程还活着
/// 的时候那个目录一直有文件锁，删不掉。所以另起一个进程，等我们退出之后再删。
#[cfg(windows)]
fn spawn_webview_cleanup() {
    let Ok(me) = std::env::current_exe() else { return };
    let _ = Command::new(me)
        .arg("--cleanup-webview")
        .arg(std::process::id().to_string())
        .spawn();
}

/// `--cleanup-webview <父进程 pid>` 的实现。
#[cfg(windows)]
fn run_webview_cleanup(parent_pid: u32) {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;

    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, parent_pid);
        if h != 0 {
            // 最多等 20 秒。父进程正常退出是毫秒级的，这个上限只是防止
            // 自己变成僵尸——等不到就直接试着删，删不掉也无所谓。
            WaitForSingleObject(h, 20_000);
            CloseHandle(h);
        }
    }

    // 文件锁松开有一点点延迟，重试几次。删不掉也不再纠缠：东西在 %TEMP% 里，
    // 系统清理临时文件时会收走，不值得为它弹个错误框。
    let dir = webview_data_dir();
    for _ in 0..20 {
        if !dir.exists() {
            return;
        }
        if std::fs::remove_dir_all(&dir).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[tauri::command]
fn target_dir() -> String {
    install_dir().to_string_lossy().into_owned()
}

/// 界面把这个字符串原样带回来，所以进安装前先归一化：
/// 去掉两边空白和末尾分隔符（资源管理器选出来的路径有时带 `\`），
/// 直接选到盘根（`D:\`）时就补一层品牌目录——没人想让整个 D 盘变成安装目录。
/// 空字符串＝界面还没问到默认值，按默认目录算。
fn normalize_dir(raw: &str) -> Result<PathBuf, String> {
    let s = raw.trim().trim_end_matches(['\\', '/']);
    if s.is_empty() {
        return Ok(install_dir());
    }
    let p = PathBuf::from(s);
    if p.file_name().is_none() {
        return Ok(p.join(APP_NAME));
    }
    Ok(p)
}

/// 安装包自己的版本和体积。
///
/// 体积只能在运行时量：它在被编译出来之前根本不存在，界面里写死一个「约 4.2 MB」
/// 过一个版本就成了假话（原型上那句 4.2 MB 就是估的）。
#[derive(serde::Serialize)]
struct Meta {
    version: String,
    size: String,
}

#[tauri::command]
fn meta() -> Meta {
    let bytes = std::env::current_exe()
        .and_then(|p| std::fs::metadata(p))
        .map(|m| m.len())
        .unwrap_or(0);
    Meta {
        version: APP_VER.to_string(),
        size: format!("约 {:.1} MB", bytes as f64 / 1048576.0),
    }
}

/// 让用户挑安装目录。返回 None ＝ 他点了取消，界面保持原样。
///
/// 为什么丢到阻塞线程池里跑：这个对话框是模态的，会自己开消息循环，
/// 在异步运行时的工作线程上占着一个线程无所谓，但绝不能在主线程上等——
/// 主线程一卡，窗口在拖动对话框时会白一块。
#[tauri::command]
async fn pick_dir(window: tauri::WebviewWindow, current: String) -> Result<Option<String>, String> {
    let owner = window.hwnd().map(|h| h.0 as isize).unwrap_or(0);
    tauri::async_runtime::spawn_blocking(move || picker::pick_folder(owner, &current))
        .await
        .map_err(|e| format!("打开选目录对话框失败：{e}"))?
}

// ---------------------------------------------------------------- 安装

/// 把压着的 payload 解开。1.8 MB → 5.3 MB 大约 0.1 秒，
/// 所以干脆整块解到内存再落盘，不做流式——少一层中间文件。
fn unpack() -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(PAYLOAD_RAW);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(PAYLOAD), &mut out)
        .map_err(|e| format!("安装内容解压失败：{e}"))?;
    Ok(out)
}

#[tauri::command]
async fn install(app: AppHandle, dir: String) -> Result<(), String> {
    let dir = normalize_dir(&dir)?;

    // 先收旧版本，再动新目录。顺序不能反：老进程占着主程序的固定名互斥量，
    // 不先把它带走，装完点图标也不会有任何反应（见 legacy 模块的说明）。
    #[cfg(windows)]
    {
        emit(&app, 2.0, "正在检查旧版本…");
        let olds = legacy::find(&dir);
        let was_running = is_running(MAIN_EXE);
        if !olds.is_empty() {
            emit(&app, 5.0, "正在清理旧版本…");
        }
        if !olds.is_empty() || was_running {
            legacy::sweep(&olds, &dir.join(MAIN_EXE));
        }
    }

    emit(&app, 8.0, "正在创建目录…");
    std::fs::create_dir_all(&dir).map_err(|e| {
        // 选到 C:\Program Files 之类的地方会拿到拒绝访问。说清楚下一步：
        // 用户不会想到「这是 UAC」，他只会觉得安装器坏了。
        format!("{} 写不进去（{e}）。点「更改」换到别的盘，或换到用户目录。", dir.display())
    })?;

    emit(&app, 12.0, "正在解开安装内容…");
    let bytes = unpack()?;

    emit(&app, 40.0, "正在写入主程序…");
    let exe = dir.join(MAIN_EXE);
    // 覆盖安装时主程序可能还在跑。这时候 write 会拿到 ERROR_SHARING_VIOLATION(32)，
    // 直接抛「拒绝访问」会让人以为是权限问题，所以单独给一句能照做的提示。
    if let Err(e) = std::fs::write(&exe, &bytes) {
        return Err(if e.raw_os_error() == Some(32) {
            format!("{APP_NAME} 正在运行，请先在托盘退出，再点一次安装。")
        } else {
            format!("写主程序失败：{e}")
        });
    }
    drop(bytes);

    emit(&app, 62.0, "正在写入运行时组件…");
    copy_webview_loader(&dir)?;

    emit(&app, 70.0, "正在创建快捷方式…");
    create_shortcuts(&dir)?;

    emit(&app, 84.0, "正在登记卸载信息…");
    // 把安装器自己复制成 uninstall.exe：「应用和功能」里的卸载项要有个 exe 可指，
    // 而用户下载的那个 setup 很可能装完就删了。
    let me = std::env::current_exe().map_err(|e| format!("定位不了安装器自身：{e}"))?;
    let uninst = dir.join(UNINST_EXE);
    if me != uninst {
        std::fs::copy(&me, &uninst).map_err(|e| format!("写卸载程序失败：{e}"))?;
    }
    write_uninstall_key(&dir)?;

    emit(&app, 96.0, "正在收尾…");
    emit(&app, 100.0, "装好了");
    Ok(())
}

/// 把 WebView2 的加载器一起装进去。
///
/// 为什么需要：用 msvc 工具链时，WebView2Loader 是静态链进 exe 的，装完一个文件就够；
/// 而用 windows-gnu 时 webview2-com-sys 只能动态链，exe 旁边必须有
/// WebView2Loader.dll，否则一启动就是「由于找不到 WebView2Loader.dll，无法继续执行代码」。
/// 主程序和安装器自己都受这条约束（安装器同理，所以它旁边也得放一个）。
///
/// 所以这里做成「隔壁有就跟着装」：msvc 编出来的包里根本没有这个文件，整段自动跳过；
/// gnu 编出来的包把 dll 放在 setup 旁边（installer/build.sh 会一起放进 dist），
/// 装的时候一并拷进安装目录，主程序才起得来。
///
/// 不塞进 payload 里：它是运行时依赖，不是「安装内容」，混进压缩包会让
/// 「解开的内容 = 主程序」这条简单关系多一个特例。
fn copy_webview_loader(dir: &Path) -> Result<(), String> {
    const DLL: &str = "WebView2Loader.dll";
    let me = std::env::current_exe().map_err(|e| format!("定位不了安装器自身：{e}"))?;
    let Some(src_dir) = me.parent() else { return Ok(()) };
    let src = src_dir.join(DLL);
    if !src.exists() {
        // msvc 编的包里没有这个 dll（loader 静态链进 exe 了）。但旧版是 gnu 编的，
        // 当时装进目录的这个文件还在——现在的 exe 根本不读它，160 KB 纯属死重量。
        let stale = dir.join(DLL);
        if stale.exists() {
            let _ = std::fs::remove_file(stale);
        }
        return Ok(());
    }
    std::fs::copy(&src, dir.join(DLL)).map_err(|e| format!("写 {DLL} 失败：{e}"))?;
    Ok(())
}

#[tauri::command]
fn launch(dir: String) -> Result<(), String> {    let dir = normalize_dir(&dir)?;
    let exe = dir.join(MAIN_EXE);
    if !exe.exists() {
        return Err(format!("找不到 {}，安装可能没完成。", exe.display()));
    }
    // 不传任何参数：主程序判首启靠的是「settings.json 在不在」（见 app/src-tauri
    // lib.rs 里 `fresh` 字段的注释），全新装完自然没有配置文件，它自己会进引导。
    // 覆盖安装时文件还在，就正常起——正是想要的行为。
    Command::new(&exe)
        .spawn()
        .map_err(|e| format!("启动失败：{e}"))?;
    Ok(())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}
// ---------------------------------------------------------------- 快捷方式

/// 用 WScript.Shell 建 .lnk。
///
/// 为什么不手写 IShellLink：那要 CoCreateInstance + 四五个 COM 方法 + IPersistFile
/// 的 vtable，unsafe 代码量比这里整个文件还多，且一旦接口顺序写错就是崩溃而不是报错。
/// 建快捷方式是一次性动作，多起一个 PowerShell 的代价可以忽略（隐藏窗口，无闪框）。
fn create_shortcuts(dir: &Path) -> Result<(), String> {
    let target = dir.join(MAIN_EXE);
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$t='{t}'
$d='{d}'
$w=New-Object -ComObject WScript.Shell
$list=@(
  (Join-Path ([Environment]::GetFolderPath('Desktop')) '{n}.lnk'),
  (Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\{n}.lnk')
)
foreach($p in $list){{
  $s=$w.CreateShortcut($p)
  $s.TargetPath=$t
  $s.WorkingDirectory=$d
  $s.IconLocation="$t,0"
  $s.Description='把月薪摊成每一秒'
  $s.Save()
}}"#,
        t = target.display(),
        d = dir.display(),
        n = APP_NAME
    );

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| format!("调不起 PowerShell：{e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("建快捷方式失败：{}", msg.trim()));
    }
    Ok(())
}

// ---------------------------------------------------------------- 选目录
mod picker;


// ---------------------------------------------------------------- 注册表

#[cfg(windows)]
mod reg {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    pub type Hkey = *mut core::ffi::c_void;

    pub const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as usize as Hkey;
    pub const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002u32 as usize as Hkey;
    const KEY_WRITE: u32 = 0x20006;
    /// KEY_READ。只读地看一眼卸载表，不需要写权限（HKLM 也读得到）。
    const KEY_READ: u32 = 0x20019;
    const KEY_SET_VALUE: u32 = 0x0002;
    pub const REG_SZ: u32 = 1;
    pub const REG_DWORD: u32 = 4;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            sa: *const core::ffi::c_void,
            out: *mut Hkey,
            disposition: *mut u32,
        ) -> i32;

        fn RegSetKeyValueW(
            hkey: Hkey,
            sub_key: *const u16,
            value_name: *const u16,
            ty: u32,
            data: *const core::ffi::c_void,
            cb: u32,
        ) -> i32;

        fn RegCloseKey(hkey: Hkey) -> i32;

        fn RegDeleteTreeW(hkey: Hkey, sub_key: *const u16) -> i32;

        fn RegOpenKeyExW(
            hkey: Hkey,
            sub_key: *const u16,
            options: u32,
            sam: u32,
            out: *mut Hkey,
        ) -> i32;

        fn RegEnumKeyExW(
            hkey: Hkey,
            index: u32,
            name: *mut u16,
            name_len: *mut u32,
            reserved: *const u32,
            class: *mut u16,
            class_len: *mut u32,
            last_write: *mut core::ffi::c_void,
        ) -> i32;

        fn RegQueryValueExW(
            hkey: Hkey,
            name: *const u16,
            reserved: *const u32,
            ty: *mut u32,
            data: *mut u8,
            cb: *mut u32,
        ) -> i32;

        fn RegDeleteValueW(hkey: Hkey, name: *const u16) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain([0]).collect()
    }

    /// 建键并返回句柄。返回值要 RegCloseKey。
    pub fn create(sub: &str) -> Result<Hkey, String> {
        let w = wide(sub);
        let mut h: Hkey = std::ptr::null_mut();
        let mut disp = 0u32;
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                w.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut h,
                &mut disp,
            )
        };
        if rc != 0 {
            return Err(format!("建注册表项失败（错误码 {rc}）"));
        }
        Ok(h)
    }

    pub fn set_sz(h: Hkey, name: &str, val: &str) -> Result<(), String> {
        let n = wide(name);
        let v: Vec<u16> = OsStr::new(val).encode_wide().chain([0]).collect();
        let rc = unsafe {
            RegSetKeyValueW(
                h,
                std::ptr::null(),
                n.as_ptr(),
                REG_SZ,
                v.as_ptr() as *const _,
                (v.len() * 2) as u32,
            )
        };
        if rc != 0 {
            return Err(format!("写 {name} 失败（错误码 {rc}）"));
        }
        Ok(())
    }

    pub fn set_dword(h: Hkey, name: &str, val: u32) -> Result<(), String> {
        let n = wide(name);
        let rc = unsafe {
            RegSetKeyValueW(
                h,
                std::ptr::null(),
                n.as_ptr(),
                REG_DWORD,
                &val as *const u32 as *const _,
                4,
            )
        };
        if rc != 0 {
            return Err(format!("写 {name} 失败（错误码 {rc}）"));
        }
        Ok(())
    }

    pub fn close(h: Hkey) {
        unsafe {
            RegCloseKey(h);
        }
    }

    pub fn delete_tree(sub: &str) -> Result<(), String> {
        delete_tree_at(HKEY_CURRENT_USER, sub)
    }

    pub fn delete_tree_at(root: Hkey, sub: &str) -> Result<(), String> {
        let w = wide(sub);
        let rc = unsafe { RegDeleteTreeW(root, w.as_ptr()) };
        // 2 = ERROR_FILE_NOT_FOUND：本来就没有，算删成功
        if rc != 0 && rc != 2 {
            return Err(format!("删注册表项失败（错误码 {rc}）"));
        }
        Ok(())
    }

    /// 只读打开一个子键。打不开就返回 None——卸载表里没有这一项。
    pub fn open_read(root: Hkey, sub: &str) -> Option<Hkey> {
        open_with(root, sub, KEY_READ)
    }

    /// 读 + 删值用的句柄。
    ///
    /// 注意不能拿 KEY_READ 的句柄去 RegDeleteValueW：注册表会返回
    /// ERROR_ACCESS_DENIED(5)，而且是个「静默失败」——值还在，代码不报错。
    pub fn open_rw(root: Hkey, sub: &str) -> Option<Hkey> {
        open_with(root, sub, KEY_READ | KEY_SET_VALUE)
    }

    fn open_with(root: Hkey, sub: &str, sam: u32) -> Option<Hkey> {
        let w = wide(sub);
        let mut h: Hkey = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(root, w.as_ptr(), 0, sam, &mut h) };
        if rc != 0 || h.is_null() {
            return None;
        }
        Some(h)
    }

    /// 列出子键名。卸载表就是个「一家一个子键」的平表，靠这个枚举。
    pub fn subkeys(h: Hkey) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0u32;
        loop {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    h,
                    i,
                    buf.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            // 259 = ERROR_NO_MORE_ITEMS，枚举到头了。其它错误也当结束，
            // 少认出一个旧安装最多是留个空目录，比死循环好。
            if rc != 0 {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
            i += 1;
        }
        out
    }

    /// 读一个字符串值。类型不是 REG_SZ / REG_EXPAND_SZ 时按字符串尽力解释——
    /// 卸载表里我们关心的那几个值（DisplayName、InstallLocation、UninstallString）
    /// 都是字符串。
    pub fn get_sz(h: Hkey, name: &str) -> Option<String> {
        let n = wide(name);
        let mut ty = 0u32;
        let mut cb = 0u32;
        let rc = unsafe {
            RegQueryValueExW(
                h,
                n.as_ptr(),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut cb,
            )
        };
        if rc != 0 || cb == 0 || cb > 64 * 1024 {
            return None;
        }
        let mut buf = vec![0u8; cb as usize];
        let rc = unsafe {
            RegQueryValueExW(h, n.as_ptr(), std::ptr::null(), &mut ty, buf.as_mut_ptr(), &mut cb)
        };
        if rc != 0 {
            return None;
        }
        let units: Vec<u16> = buf[..cb as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
        Some(String::from_utf16_lossy(&units[..end]))
    }

    /// 删一个值。删不掉（权限、不存在）不报错——调用方都在做「尽力清理」。
    pub fn delete_value(h: Hkey, name: &str) -> bool {
        let n = wide(name);
        unsafe { RegDeleteValueW(h, n.as_ptr()) == 0 }
    }
}

// ---------------------------------------------------------------- 旧版本清理

/// 0.1.x 用的是 NSIS 安装器：装到 `%LOCALAPPDATA%\paypulse`，卸载项
/// `Uninstall\paypulse`，桌面图标 `paypulse.lnk`，开机自启 `Run\paypulse`。
/// 0.1.7 换成这个自制安装器后默认装到 `%LOCALAPPDATA%\牛马薪动了`——
/// 两套并存留下的不是「多几个文件」，而是一个致命的死局：
///
/// 主程序的单实例互斥量是**固定名**（`Local\com.bydota.paypulse.single-instance`，
/// 见 app/src-tauri/src/single.rs）。老版本还在托盘里活着，新装好的程序一启动
/// 就认定「已经有一个在跑」，把老窗口叫到前面然后自己退出。用户看到的是：
/// 升级完了、点了新图标、屏幕上什么都没发生。
///
/// 所以装之前必须把老版本收走。判据不看目录名（理论上能改），看卸载表里的
/// DisplayName + InstallLocation；表里查不到时再退回一条已知路径，
/// 防止注册表被清理工具扫过、文件却还在。
#[cfg(windows)]
mod legacy {
    use super::{is_running, local_appdata, reg, MAIN_EXE};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    const UNINST: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINST_WOW: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub struct Old {
        dir: PathBuf,
        /// 卸载表里的 UninstallString，可能带引号、也可能带参数。
        uninstall: Option<String>,
    }

    /// 32 位 NSIS 安装器写 HKCU 会被 WOW64 重定向到 `WOW6432Node`，
    /// 64 位的这个安装器写 HKCU 则落在不带 WOW 的那条路径上。
    /// 两条都要看，否则「旧版装过、新版看不见」。
    fn uninstall_roots() -> [(&'static str, reg::Hkey); 4] {
        [
            (UNINST, reg::HKEY_CURRENT_USER),
            (UNINST_WOW, reg::HKEY_CURRENT_USER),
            (UNINST, reg::HKEY_LOCAL_MACHINE),
            (UNINST_WOW, reg::HKEY_LOCAL_MACHINE),
        ]
    }

    /// 卸载表里跟这个产品有关的 DisplayName：0.1.x 是 `paypulse`，
    /// 改名之后是 `牛马薪动了`。
    fn is_ours(display: &str) -> bool {
        let low = display.trim().to_lowercase();
        low == "paypulse" || low.contains("牛马薪动了")
    }

    fn norm(p: &Path) -> String {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    }

    fn same(a: &Path, b: &Path) -> bool {
        norm(a) == norm(b)
    }

    /// NSIS 写 InstallLocation 时是带引号的（`"$INSTDIR"`）。
    fn unquote(s: &str) -> String {
        let t = s.trim();
        let t = t.strip_prefix('"').unwrap_or(t);
        t.strip_suffix('"').unwrap_or(t).trim().to_string()
    }

    /// `"C:\a b\uninstall.exe" --uninstall` → `C:\a b\uninstall.exe`
    fn exe_of(cmd: &str) -> Option<PathBuf> {
        let t = cmd.trim();
        if let Some(rest) = t.strip_prefix('"') {
            let end = rest.find('"')?;
            return Some(PathBuf::from(&rest[..end]));
        }
        // 没引号：截到 .exe 为止，路径里的空格才不会把它截断
        let low = t.to_lowercase();
        let cut = low.find(".exe").map(|i| i + 4).unwrap_or(t.len());
        let p = PathBuf::from(t[..cut].trim());
        if p.as_os_str().is_empty() {
            None
        } else {
            Some(p)
        }
    }

    /// 扫出所有该收走的旧安装（不含 target 自己）。
    pub fn find(target: &Path) -> Vec<Old> {
        let mut out: Vec<Old> = Vec::new();

        for (sub, root) in uninstall_roots() {
            let Some(h) = reg::open_read(root, sub) else { continue };
            for key in reg::subkeys(h) {
                let Some(k) = reg::open_read(h, &key) else { continue };
                let name = reg::get_sz(k, "DisplayName").unwrap_or_default();
                if !is_ours(&name) {
                    reg::close(k);
                    continue;
                }
                let loc = reg::get_sz(k, "InstallLocation").unwrap_or_default();
                let uninstall = reg::get_sz(k, "QuietUninstallString")
                    .or_else(|| reg::get_sz(k, "UninstallString"));
                reg::close(k);

                let loc = unquote(&loc);
                let dir = if loc.is_empty() {
                    uninstall
                        .as_deref()
                        .and_then(exe_of)
                        .and_then(|p| p.parent().map(Path::to_path_buf))
                } else {
                    Some(PathBuf::from(loc))
                };
                let Some(dir) = dir else { continue };

                // 同一个目录：那是「重装 / 覆盖安装」，不是旧安装，别去卸它。
                if same(&dir, target) || out.iter().any(|o| same(&o.dir, &dir)) {
                    continue;
                }
                out.push(Old { dir, uninstall });
            }
            reg::close(h);
        }

        // 兜底：卸载项被人清过、目录还躺着。0.1.x 的安装器把「选择安装目录」
        // 那一页删了，路径是写死的，所以这条已知路径足够可靠。
        let guess = local_appdata().join("paypulse");
        if guess.join(MAIN_EXE).exists()
            && !same(&guess, target)
            && !out.iter().any(|o| same(&o.dir, &guess))
        {
            out.push(Old { dir: guess, uninstall: None });
        }

        out
    }

    /// 结束还在托盘里的 paypulse.exe。
    ///
    /// 这一步不能省：互斥量是固定名，老进程不走，新程序起来就自己退了。
    /// 两种场景都要杀——升级（旧目录的进程）和覆盖安装（同一目录的旧进程，
    /// 不杀的话 exe 正被占用，写文件会拿 ERROR_SHARING_VIOLATION）。
    pub fn close_running() {
        if !is_running(MAIN_EXE) {
            return;
        }
        // /F 是强杀。主程序每次状态变化都落盘，丢的最多是最后几秒；
        // 但让用户「先自己去托盘退出再装」在升级路径上是走不通的——
        // 用户根本不知道旧图标还活着。
        let _ = Command::new("taskkill").args(["/F", "/IM", MAIN_EXE]).output();
        let t0 = Instant::now();
        while is_running(MAIN_EXE) && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 收走旧安装，并清掉那些不跟着目录走的东西（图标、自启）。返回收走了几个。
    pub fn sweep(olds: &[Old], target_exe: &Path) -> usize {
        close_running();
        for old in olds {
            retire(old);
        }
        clean_shortcuts();
        clean_run(target_exe);
        olds.len()
    }

    fn retire(old: &Old) {
        // 先让旧卸载器干：NSIS 自己知道它还注册过什么（开始菜单文件夹、
        // AppUserModelID、文件关联），这些从外面猜不全。
        //
        // `/S` 完全静默、不弹页面；旧 NSIS 安装器那个「同时删除应用数据」的
        // 勾选框在 passive 模式下整页被跳过，所以用户的历史账不会被连带删掉
        // ——升级要的正是这个。
        if let Some(exe) = old.uninstall.as_deref().and_then(exe_of) {
            if exe.exists() {
                if let Ok(mut child) = Command::new(&exe).arg("/S").spawn() {
                    let t0 = Instant::now();
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) | Err(_) => break,
                            Ok(None) => {
                                if t0.elapsed() > Duration::from_secs(20) {
                                    // 老卸载器卡住了（极少见）：杀掉，自己收尾。
                                    let _ = child.kill();
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(80));
                            }
                        }
                    }
                }
            }
        }

        // 卸载器不在、起不来、或者没删干净：自己补刀。
        // 删不掉的残留不会影响新版本运行（新目录是另一个），所以不报错。
        for _ in 0..4 {
            match std::fs::remove_dir_all(&old.dir) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }

        // 卸载项：卸载器活下来时自己会删，死了就得我们删。
        for (sub, root) in uninstall_roots() {
            let _ = reg::delete_tree_at(root, &format!(r"{sub}\paypulse"));
        }
        // MANUPRODUCTKEY = Software\BYDOTA\paypulse，指向旧目录。
        let _ = reg::delete_tree(r"Software\BYDOTA\paypulse");
    }

    /// 清掉旧图标。`牛马薪动了.lnk` 随后会被 create_shortcuts 重建，
    /// 先删后建没有副作用；而 `paypulse.lnk` 没人再建了，留着就是死链。
    fn clean_shortcuts() {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        let desktop = home.join("Desktop");
        let programs = home.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
        for n in ["paypulse.lnk", "牛马薪动了.lnk"] {
            let _ = std::fs::remove_file(desktop.join(n));
            let _ = std::fs::remove_file(programs.join(n));
        }
        // NSIS 版会建同名开始菜单文件夹（$SMPROGRAMS\paypulse\），新安装器不建。
        let _ = std::fs::remove_dir_all(programs.join("paypulse"));
        let _ = std::fs::remove_dir_all(programs.join("牛马薪动了"));
    }

    /// 清掉指向旧目录的开机自启。
    ///
    /// 不去判断值名（`paypulse` / `PayPulse` / `牛马薪动了` 三种都可能存在），
    /// 而是看它指向哪儿：指向即将装好的 exe 就留着——那是用户在新版设置里
    /// 自己打开的，替他关掉就是丢设置；指向别处一律删。
    fn clean_run(target_exe: &Path) {
        let Some(h) = reg::open_rw(reg::HKEY_CURRENT_USER, RUN) else { return };
        for name in ["PayPulse", "牛马薪动了"] {
            if let Some(v) = reg::get_sz(h, name) {
                let to_target = exe_of(&v).map_or(false, |p| same(&p, target_exe));
                if !to_target {
                    reg::delete_value(h, name);
                }
            }
        }
        reg::close(h);
    }
}

/// 「应用和功能」里那一行，全靠这几个值。少写 DisplayName 就查不到，
/// 少写 UninstallString 就点不动卸载。
#[cfg(windows)]
fn write_uninstall_key(dir: &Path) -> Result<(), String> {
    let sub = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_NAME}");
    let h = reg::create(&sub)?;

    let exe = dir.join(MAIN_EXE);
    let uninst = dir.join(UNINST_EXE);
    let r = (|| -> Result<(), String> {
        reg::set_sz(h, "DisplayName", APP_NAME)?;
        reg::set_sz(h, "DisplayVersion", APP_VER)?;
        reg::set_sz(h, "Publisher", "BYDOTA")?;
        reg::set_sz(h, "DisplayIcon", &format!("{},0", exe.display()))?;
        reg::set_sz(h, "InstallLocation", &dir.to_string_lossy())?;
        reg::set_sz(h, "UninstallString", &format!("\"{}\" --uninstall", uninst.display()))?;
        reg::set_sz(h, "QuietUninstallString", &format!("\"{}\" --uninstall /S", uninst.display()))?;
        // 1 = 不显示「修改」，2 = 不显示「修复」。这程序没什么可修的。
        reg::set_dword(h, "NoModify", 1)?;
        reg::set_dword(h, "NoRepair", 1)?;
        Ok(())
    })();
    reg::close(h);
    r
}

#[cfg(not(windows))]
fn write_uninstall_key(_dir: &Path) -> Result<(), String> {
    Err("安装器目前只支持 Windows".into())
}

// ---------------------------------------------------------------- 卸载

#[cfg(windows)]
mod msg {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
    }
    const MB_YESNO: u32 = 0x4;
    const MB_ICONQUESTION: u32 = 0x20;
    const IDYES: i32 = 6;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub fn confirm(text: &str, caption: &str) -> bool {
        let t = wide(text);
        let c = wide(caption);
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
    }

    pub fn info(text: &str, caption: &str) {
        let t = wide(text);
        let c = wide(caption);
        unsafe {
            MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x40 /* MB_ICONINFORMATION */);
        }
    }
}

/// 卸载目标目录。
///
/// 用户可以把程序装到别的地方（界面上的「更改」），所以不能再假设是默认目录。
/// 卸载器自己被拷在安装目录里、也被注册表指着，直接看「我是谁、我在哪儿」最可靠：
/// 自己叫 uninstall.exe 时，所在目录就是安装目录。
#[cfg(windows)]
fn installed_dir() -> PathBuf {
    if let Ok(me) = std::env::current_exe() {
        if me.file_name() == Some(std::ffi::OsStr::new(UNINST_EXE)) {
            if let Some(parent) = me.parent() {
                return parent.to_path_buf();
            }
        }
    }
    install_dir()
}

#[cfg(windows)]
fn run_uninstall(silent: bool) {
    let dir = installed_dir();

    // 卸载器住在安装目录里，而 Windows 不允许删除正在运行的程序——
    // 直接删目录会剩一个 uninstall.exe 和整个空壳。所以先把自己复制到 %TEMP%
    // 再让副本干活：副本不在安装目录里，才删得干净。
    // 用环境变量标记，避免副本再复制一次自己（无限套娃）。
    if std::env::var_os("PAYPULSE_UNINST_COPY").is_none() {
        if let Ok(me) = std::env::current_exe() {
            if me.starts_with(&dir) {
                let tmp = std::env::temp_dir().join("paypulse-uninstall.exe");
                if std::fs::copy(&me, &tmp).is_ok() {
                    let mut c = Command::new(&tmp);
                    c.arg("--uninstall");
                    if silent {
                        c.arg("/S");
                    }
                    c.env("PAYPULSE_UNINST_COPY", "1");
                    if c.spawn().is_ok() {
                        return;
                    }
                }
            }
        }
    }

    if !silent
        && !msg::confirm(
            &format!("要从这台电脑上卸载 {APP_NAME} 吗？\n\n安装目录：{}\n\n你的设置和历史数据会一起删掉。", dir.display()),
            &format!("卸载 {APP_NAME}"),
        )
    {
        return;
    }

    if !silent && is_running(MAIN_EXE) {
        // 主程序还在跑的话文件删不掉。静默卸载时直接结束进程反而可能丢掉
        // 还没落盘的当日账，所以这里两种情况都只提示，让用户自己退。
        msg::info(
            &format!("{APP_NAME} 正在运行。请先在托盘图标上退出，再运行卸载。"),
            &format!("卸载 {APP_NAME}"),
        );
        return;
    }

    let _ = reg::delete_tree(&format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_NAME}"));
    // 顺带清掉自启项：安装器不主动开自启，但主程序里那个开关写的是同一个位置，
    // 只删主目录不删它，用户重装后会得到一个指向空目录的开机项。
    let _ = reg::delete_tree(r"Software\Microsoft\Windows\CurrentVersion\Run\PayPulse");

    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    let links = [
        home.join("Desktop").join(format!("{APP_NAME}.lnk")),
        home.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs")
            .join(format!("{APP_NAME}.lnk")),
    ];
    for l in links {
        let _ = std::fs::remove_file(l);
    }

    let err = std::fs::remove_dir_all(&dir).err();

    // WebView2 的数据目录不在安装目录里，卸载只删安装目录的话，用户卸完了
    // 硬盘里还躺着 113 MB（334 个文件）。两个位置都要清：
    //   · %TEMP%\paypulse-setup —— 当前版本用的；
    //   · %LOCALAPPDATA%\com.bydota.paypulse.setup —— 老版本（以及这台机器上
    //     已经装过的那些人）留下的，identifier 决定的名字。
    let _ = std::fs::remove_dir_all(webview_data_dir());
    let _ = std::fs::remove_dir_all(local_appdata().join(LEGACY_WEBVIEW_DIR));

    if silent {
        return;
    }
    match err {
        None => msg::info(&format!("{APP_NAME} 已卸载。"), &format!("卸载 {APP_NAME}")),
        Some(e) => msg::info(
            &format!("卸载没做干净：{e}\n\n可以先重启电脑再删一次 {}", dir.display()),
            &format!("卸载 {APP_NAME}"),
        ),
    }
}

#[cfg(windows)]
fn is_running(exe: &str) -> bool {
    // tasklist 的输出会因为语言而本地化，但「有这一行 / 没这一行」是稳的。
    Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {exe}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(exe))
        .unwrap_or(false)
}

// ---------------------------------------------------------------- 入口

/// 上一个窗口过程。子类化之后要把不关心的消息原样还给它。
#[cfg(windows)]
static PREV_WNDPROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 窗口类背景刷是不是已经换过。CreateSolidBrush 出来的刷子归系统管、不回收，
/// 每调一次漏一个 GDI 对象——decorate_window 现在会被调两次（建窗后一次、
/// 露脸后一次），所以这里要有个「只干一次」的闸。
#[cfg(windows)]
static CLASS_BRUSH_SET: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 把非客户区削成零。
///
/// 光把 style 换成 WS_POPUP 还不够：winit 在 WM_NCCALCSIZE 里仍然会留一圈
/// （实测左右 7、上下 4 逻辑像素），那是它给「拖边框改大小」预留的命中区。
/// 后果是客户区比窗口小一圈，卡片四周挂着一道边——浅色主题下是白边，
/// 深色下也能看出卡片没贴边。
///
/// 这张卡片要求严丝合缝，所以在这里直接回 0：客户区 = 整个窗口。
#[cfg(windows)]
unsafe extern "system" fn no_frame_proc(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize {
    use std::sync::atomic::Ordering;

    const WM_NCCALCSIZE: u32 = 0x0083;
    if msg == WM_NCCALCSIZE && wp != 0 {
        return 0;
    }

    let prev = PREV_WNDPROC.load(Ordering::Relaxed);
    if prev == 0 {
        return 0;
    }
    #[link(name = "user32")]
    extern "system" {
        fn CallWindowProcW(
            proc: isize,
            hwnd: isize,
            msg: u32,
            wp: usize,
            lp: isize,
        ) -> isize;
    }
    CallWindowProcW(prev, hwnd, msg, wp, lp)
}

/// 把「带边框的窗口」改造成方案里那张卡片。
///
/// 干四件事：摘掉 caption/边框、子类化窗口过程把非客户区削成零、把窗口类的
/// 背景刷换成卡片色（消白屏）、让 DWM 上圆角并把 1px 边框染成卡片色。
///
/// **这个函数是幂等的，允许重复调用**——实测摘边框和 winit 显示窗口之间有
/// 时序竞争：`visible: false` 下建窗后摘一次，`show()` 那一下 winit 有可能
/// 把原来的 style 又写回去（连开三次，三次 style 位都不一样，实测
/// 0x14CB0000 里还留着 WS_CAPTION）。几何上被 no_frame_proc 兜住了看不出
/// 来，但样式位不该挂着 caption，所以露脸之后再来一遍。重复调用的安全性
/// 靠两个闸：子类化只做一次（否则 prev 会指回自己 → 无限递归），
/// 背景刷只建一次（GDI 对象不回收）。
fn decorate_window(window: &tauri::WebviewWindow) {
    type SetWindowAttribute =
        unsafe extern "system" fn(isize, u32, *const core::ffi::c_void, u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
        fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8)
            -> *const core::ffi::c_void;
    }

    #[link(name = "user32")]
    extern "system" {
        fn SetClassLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn InvalidateRect(hwnd: isize, rect: *const core::ffi::c_void, erase: i32) -> i32;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateSolidBrush(color: u32) -> *mut core::ffi::c_void;
    }

    // DWM 的边框颜色：Windows 11 给无边框窗口画的那一圈 1px 线。
    // 它跟的是系统主题，不是我们的界面——浅色主题下就是一条白边，
    // 圆角处还有抗锯齿混色，看起来像卡片外面贴了张白纸。
    const DWMWA_BORDER_COLOR: u32 = 34;
    // GetClassLongPtr 里「背景刷」的槽位（GCLP_HBRBACKGROUND）。
    const GCLP_HBRBACKGROUND: i32 = -10;

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;

    // COLORREF 是 0x00BBGGRR。直接写 0x0B0E13 会得到 R=0x13 G=0x0E B=0x0B，
    // 一片偏紫的黑。这里必须把 #0B0E13 反过来拼。
    const CARD_BG: u32 = 0x0013_0E0B;

    // 窗口比内容大出来的那 14×8，是非客户区：`decorations: false` 并没有把
    // WS_CAPTION / WS_DLGFRAME 摘干净（实测 style 里还留着 0x00C00000），
    // 于是 DWM 按系统色在四周填空，浅色主题下就是一条白边。
    // 换成 WS_POPUP 之后窗口矩形 = 客户区矩形，这条缝从根上没有了。
    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const GWLP_WNDPROC: i32 = -4;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_EX_WINDOWEDGE: u32 = 0x0000_0100;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    let Ok(h) = window.hwnd() else { return };
    let hwnd = h.0 as isize;

    unsafe {
        // ① 摘掉 caption / 边框，让窗口矩形等于客户区矩形。
        //    SWP_FRAMECHANGED 是必需的：它在改完 style 后强制重算非客户区，
        //    否则新样式要等到窗口下次被移动或改尺寸才认，白边会一直留着。
        //    注意别把 WS_VISIBLE 写进来——窗口现在还是藏着的（visible: false），
        //    这一位决定了它什么时候露脸，得留给 show_me。
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let style = (style & !WS_CAPTION) | WS_POPUP | WS_CLIPSIBLINGS;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);

        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_WINDOWEDGE) as isize);

        // ② 子类化窗口过程，把非客户区削成零（见 no_frame_proc）。
        //    必须排在 style 之后、SWP_FRAMECHANGED 之前，否则这一次重算
        //    还是老过程在管，白留一圈。
        //    只做一次：第二次进来时 prev 会取回我们自己的 no_frame_proc，
        //    存进去之后 CallWindowProcW 就变成自己调自己了。
        if PREV_WNDPROC.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, no_frame_proc as *const () as isize);
            PREV_WNDPROC.store(prev, std::sync::atomic::Ordering::Relaxed);
        }
        // 换成 popup 后不再按 434×308 摆，但也不能直接设 420×300 像素：
        // 客户区该不该等于窗口，交给上面那个 WM_NCCALCSIZE 决定；
        // 宽高本身交给 winit 按逻辑像素换算（见函数末尾的 set_size），
        // 这里只负责让系统把非客户区重算一次。
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );


        // ② 窗口类的背景刷换成卡片色。网页还没画出来的那几百毫秒，
        //    铺满客户区的就是这把刷子；默认是系统的白色。
        //    只换一次：CreateSolidBrush 出来的刷子由系统持有、不回收，
        //    重复调用就是在漏 GDI 对象。
        if !CLASS_BRUSH_SET.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let brush = CreateSolidBrush(CARD_BG);
            if !brush.is_null() {
                SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush as isize);
                // 换刷子不会自动重画已经画过的像素，得推它一把，否则要等到
                // 下一次系统觉得这块区域脏了才生效。
                InvalidateRect(hwnd, std::ptr::null(), 1);
            }
        }

        // 圆角 + 边框色走 DWM。这里不用 early return：下面还有一次 set_size
        // 不能因为「这台系统没有 DWM」被跳过。
        let dll: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(dll.as_ptr());
        if !module.is_null() {
            let proc = GetProcAddress(module, b"DwmSetWindowAttribute\0".as_ptr());
            if !proc.is_null() {
                let set_attr: SetWindowAttribute = std::mem::transmute(proc);

                // 圆角交给系统，CSS 圆角只能圆掉卡片内部，窗口四角还是方的。
                // Win11 生效；Win10 上这个属性不存在，调用返回失败值，忽略即可。
                let pref: i32 = DWMWCP_ROUND;
                set_attr(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &pref as *const i32 as *const _,
                    std::mem::size_of::<i32>() as u32,
                );

                // 那圈 1px 边框染成卡片色 = 等于让它消失。
                let border = CARD_BG;
                set_attr(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    &border as *const u32 as *const _,
                    std::mem::size_of::<u32>() as u32,
                );
            }
        }
    }

    // 420×300 是逻辑像素（方案里定的），物理尺寸让 winit 按当前 DPI 换算：
    // 125% 的屏幕上应该是 525×375。这里不能自己拿 GetDpiForWindow 手算——
    // setup 这个时间点上它还可能返回 96，算出来就是 420×300 物理像素，
    // 那 CSS 视口只剩 336×240，界面底部那两个按钮直接被裁掉。
    let _ = window.set_size(tauri::LogicalSize::new(420.0, 300.0));
}

/// 立刻把窗口显示出来，绕开 tao 的事件队列。
///
/// `WebviewWindow::show()` 并不直接调系统 API，它只是往事件循环里投一个
/// 事件；而建 WebView2 恰好也是在事件循环里**同步**做的（3~19 秒）。
/// 所以 setup 里调 show() 之后，它要排在 WebView2 建完才能执行——
/// 实测和原来「等界面就绪再露脸」一样慢（5.2 s），改了等于没改。
///
/// 在我们自己的线程上直接 ShowWindow 是同步生效的，这才是「200ms 露脸」。
#[cfg(windows)]
fn show_now(window: &tauri::WebviewWindow) {
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    }
    const SW_SHOW: i32 = 5;

    if let Ok(h) = window.hwnd() {
        unsafe {
            ShowWindow(h.0 as isize, SW_SHOW);
        }
    }
}

/// 系统认为这个窗口现在可见吗（`IsWindowVisible`，不看有没有被别的窗口盖住）。
#[cfg(windows)]
fn is_visible(window: &tauri::WebviewWindow) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn IsWindowVisible(hwnd: isize) -> i32;
    }
    match window.hwnd() {
        Ok(h) => unsafe { IsWindowVisible(h.0 as isize) != 0 },
        Err(_) => false,
    }
}

/// 界面把第一帧画完之后的收尾：再要一次尺寸、把边框再修一遍、把焦点拿过来。
///
/// 窗口不是在这里露脸的：占位卡片在 main() 里就显示了，真窗口在 setup 里接管，
/// 用户不会经历「双击之后什么都没发生」那一段（见 splash 模块的说明）。
///
/// 界面那边有 1.5 秒兜底：脚本万一在这里出岔子，这一步也不会被跳过。
#[tauri::command]
fn show_me(window: tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        trace("show_me 被界面调用");
        // 兜底：正常路径上 setup 已经交接并销毁了卡片，走到这里它是 0。
        // 万一 setup 那条路提前退出了（比如拿不到窗口），这里补一刀，
        // 免得一块「正在准备…」的卡片永远挂在屏幕上。
        splash::dismiss();
    }

    // 尺寸必须在这里再要一次。窗口在建好时是对的（125% 缩放下 525×375 物理
    // = 420×300 CSS），但过一会儿会被按「物理 = 逻辑」重置成 420×300，
    // 那样 CSS 视口只剩 336×240，界面底部的按钮直接被裁掉。
    // 走 LogicalSize 让 winit 自己按当前 DPI 换算，别手写像素。
    let _ = window.set_size(tauri::LogicalSize::new(420.0, 300.0));
    let _ = window.show();
    let _ = window.set_focus();

    // 露脸之后再修一次边框。show() 这一下 winit 有几率把 style 写回带
    // caption 的版本（见 decorate_window 的注释），几何被 no_frame_proc
    // 兜住了看不出来，但样式位留着 caption 就是没修干净。函数是幂等的。
    #[cfg(windows)]
    decorate_window(&window);
}

/// 「已经有一个安装器在跑」时要被拎到前面的那个窗口。
#[cfg(windows)]
static FOUND_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// EnumWindows 的回调：找一个标题里带产品名的可见窗口。
///
/// 按「标题含产品名」找，而不是按 `FindWindowW` 精确匹配 "牛马薪动了 · 安装"：
/// 那个字符串在 tauri.conf.json 里还有一份，写死在这儿就是第二处真相，
/// 改标题时必漏。产品名本身已经在 APP_NAME 里，只有一处。
#[cfg(windows)]
unsafe extern "system" fn find_ours(hwnd: isize, _lp: isize) -> i32 {
    use std::sync::atomic::Ordering;

    #[link(name = "user32")]
    extern "system" {
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn GetWindowTextLengthW(hwnd: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
    }

    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let n = GetWindowTextLengthW(hwnd);
    if n <= 0 {
        return 1;
    }
    let mut buf = vec![0u16; n as usize + 1];
    GetWindowTextW(hwnd, buf.as_mut_ptr(), n + 1);
    if String::from_utf16_lossy(&buf[..n as usize]).contains(APP_NAME) {
        FOUND_WINDOW.store(hwnd, Ordering::Relaxed);
        return 0; // 找到就停，不用继续枚举
    }
    1
}

/// 只允许一个安装器实例。
///
/// 这条是「露脸慢」的副作用，不是洁癖：以前双击后有几个秒完全没反应，用户会
/// 再点一次，于是两个安装器同时跑。两边都往 `%LOCALAPPDATA%\牛马薪动了` 写
/// 文件，而 `fs::copy` 不是原子写——A 装完立刻 `launch()` 的时候，B 可能正把
/// 同一个 exe 重写到一半，那次启动就失败。
///
/// 做法是命名互斥体（`Local\` 前缀 = 只在本登录会话内，不跨用户）。已经有
/// 一个在跑时不弹窗、不报错，直接把那个窗口提到前面然后自己退出——
/// 用户第二次双击，观感等价于「切回刚才那个窗口」。
///
/// 返回 false 表示「别继续，已经有人在干了」。
#[cfg(windows)]
fn claim_single_instance() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attrs: *const core::ffi::c_void,
            owner: i32,
            name: *const u16,
        ) -> *mut core::ffi::c_void;
        fn GetLastError() -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: unsafe extern "system" fn(isize, isize) -> i32, lp: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    let name: Vec<u16> = "Local\\com.bydota.paypulse.setup.instance\0"
        .encode_utf16()
        .collect();

    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            // 极端情况下建不出来（句柄或内存耗尽）：宁可多开一个，
            // 也不能让用户双击了却起不来。
            return true;
        }
        // 句柄故意不关：它要一直开着直到本进程退出，那个「有人在跑」的
        // 标记才是有效的。进程结束时系统会收回去。
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }

        FOUND_WINDOW.store(0, std::sync::atomic::Ordering::Relaxed);
        EnumWindows(find_ours, 0);
        let w = FOUND_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
        if w != 0 {
            ShowWindow(w, SW_RESTORE);
            SetForegroundWindow(w);
        }
        false
    }
}

/// 启动时序探针：只有设了环境变量 `PAYPULSE_SETUP_TRACE` 才写。
///
/// 「双击之后几秒没反应」这个问题只靠猜是修不掉的——建窗、WebView2 起进程、
/// 页面脚本执行、show 真正生效，每一步都可能吃掉几秒，而它们的耗时在这台
/// 机器上还会从 3 s 波动到 19 s。所以留一个开关，把每一段的毫秒打到
/// `%TEMP%\paypulse-setup-trace.log`，下次再遇到慢就有据可查。
///
/// 平时这个函数只多一次环境变量查询，编译进 release 也无所谓。
#[cfg(windows)]
fn trace(msg: &str) {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }

    static T0: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

    if std::env::var_os("PAYPULSE_SETUP_TRACE").is_none() {
        return;
    }
    let now = unsafe { GetTickCount64() };
    let t0 = *T0.get_or_init(|| now);
    let line = format!("{:>6} ms  {}\n", now - t0, msg);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("paypulse-setup-trace.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

// ------------------------------------------------------- 占位卡片（纯 Win32）

/// 在 Tauri 的窗口能出现之前，先给用户一块「我在干活」的卡片。
///
/// 为什么要这么绕：Tauri 的执行顺序是「先把 tauri.conf.json 里的窗口连同
/// WebView2 一起建好，再调 setup」。WebView2 从起进程到能画第一帧实测要
/// 3~19 秒（这台机器四次采样：3.7 / 4.6 / 19.5 / 3.9 s），这段时间主线程一直
/// 卡在建窗里——setup 里写的 show() 要等它建完才轮得到执行（实测 setup 直到
/// 15.2 秒才被调用，等于什么都没改）。
///
/// 所以只能抢在 Tauri 前面，用纯 Win32 直接开一块卡片。它跟 tao/wry 完全无关：
/// 自己注册窗口类、自己画、自己销毁，Tauri 那边一行都不用动。
/// 之所以能画出来，是因为 wry 建 WebView2 时走的是 `wait_with_pump`
/// （webview2-com 0.39.1 src/lib.rs:60，GetMessage + DispatchMessage 循环），
/// 主线程在等的同时仍在派发消息，WM_PAINT 不会饿死。
#[cfg(windows)]
mod splash {
    use crate::APP_NAME;
    use std::sync::atomic::{AtomicIsize, Ordering};

    /// 占位窗口的句柄。0 = 没有（还没建，或已经交接完）。
    static HWND: AtomicIsize = AtomicIsize::new(0);
    /// 字号缓存，只 CreateFont 一次。
    static FONT: AtomicIsize = AtomicIsize::new(0);

    /// COLORREF 是 0x00BBGGRR，跟界面里的 #0B0E13 / #6B7280 是反着拼的。
    const CARD_BG: u32 = 0x0013_0E0B;
    const TEXT_COLOR: u32 = 0x0080_726B;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        wnd_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
        cls_extra: i32,
        wnd_extra: i32,
        instance: isize,
        icon: isize,
        cursor: isize,
        background: isize,
        menu_name: *const u16,
        class_name: *const u16,
        icon_sm: isize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct Paint {
        hdc: isize,
        erase: i32,
        rc: Rect,
        restore: i32,
        inc: i32,
        reserved: [u8; 32],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(wc: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const core::ffi::c_void,
        ) -> isize;
        fn DefWindowProcW(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize;
        fn DestroyWindow(hwnd: isize) -> i32;
        fn PostMessageW(hwnd: isize, msg: u32, wp: usize, lp: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn UpdateWindow(hwnd: isize) -> i32;
        fn BeginPaint(hwnd: isize, ps: *mut Paint) -> isize;
        fn EndPaint(hwnd: isize, ps: *const Paint) -> i32;
        fn GetClientRect(hwnd: isize, r: *mut Rect) -> i32;
        fn GetDpiForSystem() -> u32;
        fn DrawTextW(dc: isize, s: *const u16, n: i32, r: *mut Rect, fmt: u32) -> i32;
        fn SetTextColor(dc: isize, color: u32) -> u32;
        fn SetBkMode(dc: isize, mode: i32) -> i32;
        fn SelectObject(dc: isize, obj: isize) -> isize;
        fn SystemParametersInfoW(action: u32, param: u32, out: *mut core::ffi::c_void, win_ini: u32) -> i32;
        fn LoadCursorW(instance: isize, name: *const u16) -> isize;
        fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn GetWindowRect(hwnd: isize, r: *mut Rect) -> i32;
        fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
        fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *const core::ffi::c_void;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateSolidBrush(color: u32) -> isize;
        #[allow(clippy::too_many_arguments)]
        fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike_out: u32,
            charset: u32,
            out_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch: u32,
            face: *const u16,
        ) -> isize;
    }

    const WM_PAINT: u32 = 0x000F;
    const SW_SHOW: i32 = 5;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;

    /// 往给定的 DC 上画卡片内容（背景靠类背景刷，这里只画那行字）。
    ///
    /// 单独拎出来是因为它有两个调用点：正常的 WM_PAINT，以及 ShowWindow 之前
    /// 的预绘制（见 show 里的说明）。两处必须画得一模一样，否则露脸瞬间会跳一下。
    unsafe fn paint_into(hwnd: isize, dc: isize) {
        // 字号跟 DPI 走，不然高分屏上这行字小得看不清。
        let mut font = FONT.load(Ordering::Relaxed);
        if font == 0 {
            let dpi = GetDpiForSystem().max(96) as i32;
            let px = (13 * dpi + 48) / 96;
            let face: Vec<u16> = "Microsoft YaHei UI\0".encode_utf16().collect();
            font = CreateFontW(-px, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 0, 0, face.as_ptr());
            FONT.store(font, Ordering::Relaxed);
        }
        if font != 0 {
            SelectObject(dc, font);
        }
        SetTextColor(dc, TEXT_COLOR);
        SetBkMode(dc, 1); // TRANSPARENT
        let mut rc = Rect::default();
        GetClientRect(hwnd, &mut rc);
        let text: Vec<u16> = "正在准备…\0".encode_utf16().collect();
        DrawTextW(dc, text.as_ptr(), -1, &mut rc, 0x0001 | 0x0004 | 0x0020);
    }

    unsafe extern "system" fn proc(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize {
        if msg == WM_PAINT {
            let mut ps = Paint {
                hdc: 0,
                erase: 0,
                rc: Rect::default(),
                restore: 0,
                inc: 0,
                reserved: [0; 32],
            };
            let dc = BeginPaint(hwnd, &mut ps);
            if dc != 0 {
                paint_into(hwnd, dc);
                EndPaint(hwnd, &ps);
            }
            return 0;
        }
        // WM_ERASEBKGND 之类交给默认过程：类背景刷就是卡片色，白底不会露出来。
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    /// 把窗口四角磨圆、1px 边框染成卡片色——和真窗口在 decorate_window 里做的一样，
    /// 否则交接的那一下能看出「方的变圆的」。
    unsafe fn round(hwnd: isize) {
        use std::sync::atomic::AtomicBool;
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::Relaxed) {
            return;
        }
        let dll: Vec<u16> = "dwmapi.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() {
            return;
        }
        let p = GetProcAddress(module, b"DwmSetWindowAttribute\0".as_ptr());
        if p.is_null() {
            return;
        }
        type SetAttr = unsafe extern "system" fn(isize, u32, *const core::ffi::c_void, u32) -> i32;
        let set: SetAttr = std::mem::transmute(p);
        let pref: i32 = 2; // DWMWCP_ROUND
        set(hwnd, 33, &pref as *const i32 as *const _, 4);
        let border = CARD_BG;
        set(hwnd, 34, &border as *const u32 as *const _, 4);
    }

    /// 把进程标成 per-monitor DPI 感知。
    ///
    /// 不这么做的话，`GetDpiForSystem` 对「还没声明感知能力」的进程会返回 96，
    /// 125% 的屏幕上卡片是 420×300 物理像素，比 Tauri 那个 525×375 的真窗口小
    /// 一圈，交接时会看出跳一下。
    ///
    /// tao 自己也会做同样的事，但它在 `EventLoop::new` 里（tao 0.37.2
    /// platform_impl/windows/dpi.rs:20），比这里晚；同一个 API 第二次调用会
    /// 失败，而 tao 忽略返回值（它连 PER_MONITOR_AWARE 的老 API 都试），
    /// 所以提前设没有副作用。
    unsafe fn become_dpi_aware() {
        let dll: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(dll.as_ptr());
        if module.is_null() {
            return;
        }
        let p = GetProcAddress(module, b"SetProcessDpiAwarenessContext\0".as_ptr());
        if p.is_null() {
            return;
        }
        type SetCtx = unsafe extern "system" fn(isize) -> i32;
        let set: SetCtx = std::mem::transmute(p);
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 是个 -4 的伪句柄。
        set(-4);
    }

    /// 建一块 420×300 的卡片并显示。失败就算了——这块是锦上添花，
    /// 不能因为画不出来就让安装器起不来。
    pub fn show() {
        unsafe {
            become_dpi_aware();
            let instance = GetModuleHandleW(std::ptr::null());
            let class: Vec<u16> = "PayPulseSplash\0".encode_utf16().collect();
            let brush = CreateSolidBrush(CARD_BG);
            let mut wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                wnd_proc: Some(proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: 0,
                cursor: LoadCursorW(0, 32512 as *const u16), // IDC_ARROW
                background: brush,
                menu_name: std::ptr::null(),
                class_name: class.as_ptr(),
                icon_sm: 0,
            };
            if RegisterClassExW(&wc) == 0 {
                return;
            }

            // 420×300 是逻辑像素，物理尺寸按系统 DPI 换算（125% → 525×375）。
            let dpi = GetDpiForSystem().max(96) as i32;
            let w = (420 * dpi + 48) / 96;
            let h = (300 * dpi + 48) / 96;
            // 摆在工作区正中：跟 tauri.conf 里的 "center": true 落在同一处，
            // 交接时用户看不出换了个窗口。
            let mut work = Rect::default();
            SystemParametersInfoW(0x0030, 0, &mut work as *mut Rect as *mut _, 0); // SPI_GETWORKAREA
            let x = work.left + (work.right - work.left - w) / 2;
            let y = work.top + (work.bottom - work.top - h) / 2;

            // WS_POPUP：没有标题栏和边框。WS_EX_TOOLWINDOW：不占任务栏
            // （任务栏条目留给真窗口，免得交接时闪两下）。
            const WS_POPUP: u32 = 0x8000_0000;
            const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
            const WS_EX_TOPMOST: u32 = 0x0000_0008;
            let title: Vec<u16> = format!("{APP_NAME}…\0").encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                0,
                0,
                instance,
                std::ptr::null(),
            );
            if hwnd == 0 {
                return;
            }
            round(hwnd);
            HWND.store(hwnd, Ordering::Relaxed);
            // 先把内容画进 DC，再让窗口露脸。反过来的话，从 ShowWindow 到第一帧
            // WM_PAINT 之间 DWM 会拿一张白板顶着——深色界面上那一下白闪很明显
            // （实测截图：刚可见的那一刻整块 255,255,255）。
            let mut rc = Rect::default();
            if GetClientRect(hwnd, &mut rc) != 0 {
                let mut ps = Paint {
                    hdc: 0,
                    erase: 0,
                    rc: Rect::default(),
                    restore: 0,
                    inc: 0,
                    reserved: [0; 32],
                };
                let dc = BeginPaint(hwnd, &mut ps);
                if dc != 0 {
                    paint_into(hwnd, dc);
                    EndPaint(hwnd, &ps);
                }
            }
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
    }

    /// 真窗口起来了：把它挪到卡片刚才待的位置，然后销毁卡片。
    ///
    /// 顺序是「先挪、再显示、最后销毁」——反过来会在中间露一帧桌面。
    pub fn hand_off(window: &tauri::WebviewWindow) {
        let card = HWND.swap(0, Ordering::Relaxed);
        if card == 0 {
            return;
        }
        unsafe {
            if let Ok(h) = window.hwnd() {
                let mut r = Rect::default();
                if GetWindowRect(card, &mut r) != 0 {
                    SetWindowPos(
                        h.0 as isize,
                        0,
                        r.left,
                        r.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            DestroyWindow(card);
        }
    }

    /// 兜底：交接没走到（比如 setup 里提前返回）也要把卡片收掉。
    ///
    /// 注意这里可能是在 IPC 的工作线程上被调用（show_me 是 tauri command），
    /// 而 Windows 只允许窗口所属线程 DestroyWindow——跨线程调用会返回 0 且
    /// 什么都不做。所以失败时补一颗 WM_CLOSE：那个是投递到目标线程的消息队列，
    /// 跨线程是合法的，最终由窗口过程自己销毁自己。
    pub fn dismiss() {
        let card = HWND.swap(0, Ordering::Relaxed);
        if card == 0 {
            return;
        }
        unsafe {
            if DestroyWindow(card) == 0 {
                PostMessageW(card, 0x0010, 0, 0); // WM_CLOSE
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    trace("main 进入（T0）");

    let args: Vec<String> = std::env::args().collect();

    #[cfg(windows)]
    {
        if args.iter().any(|a| a == "--cleanup-webview") {
            // 自己人起的清道夫，要在互斥体之前处理，否则会被「已经有一个在跑」
            // 挡掉——那时候父进程还没退，正好是最需要它干活的时候。
            let pid = args
                .iter()
                .skip_while(|a| a.as_str() != "--cleanup-webview")
                .nth(1)
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            run_webview_cleanup(pid);
            return;
        }
        if args.iter().any(|a| a == "--uninstall") {
            run_uninstall(args.iter().any(|a| a == "/S"));
            return;
        }
        // 卸载那条路不加互斥：它是从「应用和功能」里被系统拉起来的，
        // 用户在安装器还开着时点卸载，应该让卸载跑，而不是把它挡掉。
        if !claim_single_instance() {
            return;
        }
        // WebView2 的坑位要在建窗之前铺好，见 prepare_webview_dir。
        prepare_webview_dir();
        // 抢在 Tauri 之前先把卡片显示出来。Tauri 要到 WebView2 建完（3~19 秒）
        // 才会走到 setup，这段时间屏幕上必须有东西——见 splash 模块的说明。
        splash::show();
        trace("占位卡片已显示");
    }

    let app = tauri::Builder::default()
        .on_page_load(|_, _| {
            #[cfg(windows)]
            trace("页面加载完成（on_page_load）");
        })
        .setup(|app| {
            #[cfg(windows)]
            {
                use tauri::Manager;
                trace("setup 进入");
                if let Some(w) = app.get_webview_window("main") {
                    // 先装修，再露脸。顺序不能反：decorate_window 会把窗口类的
                    // 背景刷换成卡片色，早于显示才能保证第一帧不是系统白。
                    decorate_window(&w);
                    trace("装饰完成");
                    // 等 WebView2 的这几秒一直是占位卡片在顶着（见 splash），
                    // 这里把它换成真窗口：先把真窗口挪到同一位置、显示，再销毁卡片。
                    show_now(&w);
                    splash::hand_off(&w);
                    // winit 会在显示这个动作里把 style 写回带 caption 的版本
                    // （见 decorate_window 的注释），而 hand_off 里的 SetWindowPos
                    // 又会触发一次重算。所以交接完立刻再修一遍：等 show_me 那趟
                    // 来回要 100 多毫秒，屏幕上会先闪一条浅蓝标题栏。
                    decorate_window(&w);
                    trace(&format!("真窗口已接管，IsWindowVisible={}", is_visible(&w)));
                } else {
                    // 拿不到窗口也要把卡片收掉，否则它会一直盖在屏幕上。
                    splash::dismiss();
                }
            }
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            install, launch, quit, target_dir, pick_dir, meta, show_me
        ])
        .build(tauri::generate_context!())
        .expect("安装器启动失败");

    app.run(|_, event| {
        // 不管是点「取消」、点关闭、还是装完自动退出，最后都会走到 Exit。
        // 清道夫只在这里起一次，别散落在各处。
        if let tauri::RunEvent::Exit = event {
            #[cfg(windows)]
            spawn_webview_cleanup();
        }
    });
}
