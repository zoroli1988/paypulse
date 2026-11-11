//! Windows 上必须亲自动手的三件事：开机自启、空闲检测、全局快捷键。
//!
//! 为什么不用现成的 tauri 插件：开机自启就是往 `HKCU\...\Run` 写一个字符串值，
//! 空闲检测是 `GetLastInputInfo`，全局快捷键是 `RegisterHotKey`——
//! 这三个 API 从 Win2000 起就没变过，为它们各引一个插件，换来的是
//! 又多三条版本约束、又多三个要跟着 tauri 升级的部件。
//!
//! 为什么连 `windows-sys` 也不引：那是几万个符号的绑定，而这个文件只用七个。
//! 更实际的原因是这个项目跑在 `x86_64-pc-windows-gnu` 上——`windows-sys` 0.60 起
//! 改用 `raw-dylib` 链接，编译期就要调 `dlltool`，而 rustup 的 GNU 工具链
//! 只带 `rust-lld`，不带 binutils。于是「依赖它」等于「构建不了」。
//! 手写七个 `extern` 之后，链接直接用工具链自带的 `libuser32.a` / `libadvapi32.a`，
//! 一行额外依赖都不需要。
//!
//! 三个函数都不做「失败就崩」：自启写不进去、快捷键被别的软件占了，
//! 都只该让那一个功能失效，不该让悬浮窗起不来。

#![cfg_attr(not(windows), allow(unused))]

/// 一个全局快捷键：修饰键 + 虚拟键码。
// 不再 `Copy`：`label` 是 String。注册成功的那条要原样塞回 `current`，
// 所以取 `next` 的所有权，不复制。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// 回调时带回来的编号（1 = 显隐，2 = 计时，3 = 额外收入，4 = 穿透）
    pub id: u32,
    pub mods: u32,
    pub vk: u32,
    /// 原始写法（`Ctrl+Shift+P`）。日志和冲突提示里要给人看的是这个，
    /// 而不是 `0x6+0x50`——后者只有我认得出，用户认不出。
    pub label: String,
}

/// 快捷键的动作编号。数字而不是字符串，是因为它要穿到另一个线程里。
///
/// 编号只在进程内用，前后加了新动作也不要改旧的值——`RegisterHotKey`
/// 已经注册的那些按编号分派，改号会让「按 P 出来的是穿透」。
pub const HK_SHOW: u32 = 1;
pub const HK_CLOCK: u32 = 2;
pub const HK_EXTRA: u32 = 3;
pub const HK_PASSTHRU: u32 = 4;

/// 把 `"Ctrl+Shift+P"` 解析成 `(MOD_CONTROL|MOD_SHIFT, 'P')`。
///
/// 认不出来就返回 `None`，调用方跳过这一个——用户手滑写了 `Ctrl+Shift+`
/// 不该让另外两个快捷键也不生效。
pub fn parse(spec: &str) -> Option<(u32, u32)> {
    // MOD_ALT=1, MOD_CONTROL=2, MOD_SHIFT=4, MOD_WIN=8
    let mut mods = 0u32;
    let mut vk = None;
    for raw in spec.split('+') {
        let part = raw.trim().to_ascii_lowercase();
        match part.as_str() {
            "" => return None,
            "ctrl" | "control" => mods |= 2,
            "alt" => mods |= 1,
            "shift" => mods |= 4,
            "win" | "super" | "meta" => mods |= 8,
            other => {
                if vk.is_some() {
                    return None; // 两个主键，不认
                }
                vk = Some(key_code(other)?);
            }
        }
    }
    // 一个没有修饰键的全局快捷键会抢走整个系统的这个按键，不给这个机会
    if mods == 0 {
        return None;
    }
    Some((mods, vk?))
}

/// 主键 → 虚拟键码。只认常见的：字母、数字、F1–F12、几个特殊键。
fn key_code(k: &str) -> Option<u32> {
    if k.len() == 1 {
        let c = k.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_uppercase() as u32); // VK_A..VK_Z 就是 'A'..'Z'
        }
        if c.is_ascii_digit() {
            return Some(c as u32); // VK_0..VK_9 就是 '0'..'9'
        }
    }
    Some(match k {
        "enter" | "return" => 0x0D,
        "space" => 0x20,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        _ => {
            let f = k.strip_prefix('f')?;
            let n: u32 = f.parse().ok()?;
            if (1..=12).contains(&n) {
                // VK_F1 = 0x70
                0x70 + (n - 1)
            } else {
                return None;
            }
        }
    })
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod imp {
    use super::Binding;
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex;

    // ---- 手写的 Win32 绑定（只有七个符号，见文件头的说明） ----

    /// 注册表键句柄。`HKEY_CURRENT_USER` 是个编译期常量句柄，值是 0x80000001。
    type Hkey = *mut c_void;
    /// 窗口句柄。`RegisterHotKey` 传 null 表示「绑到当前线程的消息队列」。
    type Hwnd = *mut c_void;

    const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as usize as Hkey;
    const REG_SZ: u32 = 1;
    /// 值不存在。删一个本来就不存在的启动项要算成功。
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const WM_HOTKEY: u32 = 0x0312;
    /// 自用唤醒消息。WM_APP 起的那一段是留给应用自己的。
    const WM_APP: u32 = 0x8000;

    /// 快捷键线程的线程号，用来给它塞唤醒消息。
    static THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// `LASTINPUTINFO`。就两个 u32，别处不用。
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }

    /// `MSG`。
    ///
    /// 只读 `message` 和 `wParam`，但**整个结构的大小必须对**：
    /// `PeekMessageW` 会往里写满 48 字节，这里少写一个字段就是一次栈溢出。
    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        /// `message` 之后的 4 字节对齐空洞。留着它，后面的字段才对得上。
        _pad: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
        lprivate: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount() -> u32;
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        // 用 RegSetKeyValueW / RegDeleteKeyValueW 而不是 RegCreateKeyExW：
        // 后者带一个 SECURITY_ATTRIBUTES 指针，写的是自己的 HKCU，权限我们根本不关心。
        fn RegSetKeyValueW(
            key: Hkey,
            sub_key: *const u16,
            value_name: *const u16,
            ty: u32,
            data: *const c_void,
            data_len: u32,
        ) -> i32;
        fn RegDeleteKeyValueW(key: Hkey, sub_key: *const u16, value_name: *const u16) -> i32;
    }

    /// `RECT`。左闭右开，右下是「不含」。
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    /// `MONITORINFO`。`cbSize` 必须先填对，否则 `GetMonitorInfoW` 直接返回 0。
    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    /// `MONITORFROMPOINT` 的 `dwFlags`：找不到屏就返回 null。
    const MONITOR_DEFAULTTONULL: u32 = 0;
    /// 找不到屏就退到最近的那块，绝不会返回 null。
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    /// 这块屏的**工作区**（物理像素，已经扣掉任务栏）。
    ///
    /// 为什么不能用屏幕本身：贴着屏幕底边放下，窗口下半截会被任务栏盖住——
    /// 用户看到的不是「贴住了」，而是「卡了一半进任务栏」。工作区才是
    /// 真正能看见的那块地方。
    pub fn work_area(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let mon = MonitorFromWindow(hwnd as Hwnd, MONITOR_DEFAULTTONEAREST);
            if mon.is_null() {
                return None;
            }
            let mut mi: MonitorInfo = std::mem::zeroed();
            mi.cb_size = std::mem::size_of::<MonitorInfo>() as u32;
            if GetMonitorInfoW(mon, &mut mi) == 0 {
                return None;
            }
            let r = mi.rc_work;
            if r.right <= r.left || r.bottom <= r.top {
                return None;
            }
            Some((r.left, r.top, r.right, r.bottom))
        }
    }

    /// 这个点上有没有屏。用来判断「窗口越出去的地方」是空的还是另一块屏。
    pub fn monitor_at(x: i32, y: i32) -> isize {
        unsafe {
            let pt = Point { x, y };
            MonitorFromPoint(pt, MONITOR_DEFAULTTONULL) as isize
        }
    }

    /// 窗口在哪块屏上。
    pub fn monitor_of(hwnd: isize) -> isize {
        unsafe { MonitorFromWindow(hwnd as Hwnd, MONITOR_DEFAULTTONEAREST) as isize }
    }

    /// 让屏幕上一块矩形重新画一遍。
    ///
    /// `RedrawWindow(NULL, …)` 是「重画桌面」的用法：句柄传空表示桌面窗口，
    /// 再带 `RDW_ALLCHILDREN`，落在这块矩形里的子窗口（任务栏就是其中一个）
    /// 会一起重画。透明窗口（`WS_EX_LAYERED`）被挪走之后，它原来压着的
    /// 那块地方没人保证会重画——任务栏自己懒得擦，屏幕上就留一片窗口的
    /// 残影。用户拖到任务栏下面时看到的就是这个。
    pub fn repaint_rect(left: i32, top: i32, right: i32, bottom: i32) {
        const RDW_INVALIDATE: u32 = 0x0001;
        const RDW_ERASE: u32 = 0x0004;
        const RDW_ALLCHILDREN: u32 = 0x0080;
        const RDW_UPDATENOW: u32 = 0x0100;
        unsafe {
            let rc = Rect {
                left,
                top,
                right,
                bottom,
            };
            RedrawWindow(
                std::ptr::null_mut(),
                &rc,
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Hwnd;
        fn MonitorFromPoint(pt: Point, flags: u32) -> Hwnd;
        fn GetMonitorInfoW(mon: Hwnd, info: *mut MonitorInfo) -> i32;
        fn RedrawWindow(hwnd: Hwnd, rc: *const Rect, rgn: *mut std::ffi::c_void, flags: u32) -> i32;
        fn RegisterHotKey(hwnd: Hwnd, id: i32, mods: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: Hwnd, id: i32) -> i32;
        /// 阻塞式取消息。`hWnd` 传 null = 取本线程的消息队列（含 WM_HOTKEY）。
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        /// 往**线程**队列塞一条消息，用来把阻塞在 `GetMessageW` 上的线程叫醒。
        fn PostThreadMessageW(thread_id: u32, msg: u32, wp: usize, lp: isize) -> i32;
        /// 鼠标的**屏幕物理坐标**。右键菜单要贴鼠标，用它最准：
        /// 不经过 WebView 的 CSS 像素换算，也不受前端 DPI 认知影响。
        fn GetCursorPos(pt: *mut Point) -> i32;
    }

    /// 鼠标当前屏幕物理坐标。
    pub fn cursor_pos() -> (i32, i32) {
        let mut pt = Point { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
        }
        (pt.x, pt.y)
    }

    /// `&str` → 以 0 结尾的 UTF-16。
    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain([0]).collect()
    }

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "PayPulse";

    /// 开 / 关开机自启。
    ///
    /// 只写这一处：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`。
    /// 不碰计划任务、不碰启动文件夹、不写服务——「不写入启动项以外的任何位置」
    /// 是设置页里那句承诺，删掉这个值就是干净的卸载。
    pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
        let sub = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        unsafe {
            if on {
                // 路径带空格时必须加引号，否则 Windows 会按空格切开当参数
                let cmd = format!("\"{}\"", exe.display());
                let data = wide(&cmd);
                let rc = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    sub.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    data.as_ptr() as *const c_void,
                    (data.len() * 2) as u32,
                );
                if rc == 0 {
                    Ok(())
                } else {
                    Err(format!("写启动项失败（{rc}）"))
                }
            } else {
                // 已经不存在也算成功：目的达到了
                let rc = RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr());
                if rc == 0 || rc == ERROR_FILE_NOT_FOUND {
                    Ok(())
                } else {
                    Err(format!("删启动项失败（{rc}）"))
                }
            }
        }
    }

    /// 距离最后一次键鼠活动过了多少秒。
    ///
    /// 用 `GetTickCount` 的 32 位差值 + `wrapping_sub`：49.7 天回绕一次，
    /// 回绕时相减仍然正确。这是这个 API 唯一一个不那么显然的地方。

    pub fn idle_seconds() -> u64 {
        unsafe {
            let mut info = LastInputInfo {
                cb_size: std::mem::size_of::<LastInputInfo>() as u32,
                dw_time: 0,
            };
            if GetLastInputInfo(&mut info) == 0 {
                return 0;
            }
            let now = GetTickCount();
            let diff = now.wrapping_sub(info.dw_time);
            (diff / 1000) as u64
        }
    }

    /// 快捷键线程的句柄：只管把新的绑定推过去。
    pub struct Hotkeys {
        tx: Sender<Vec<Binding>>,
    }

    // Sender 本身是 Send 的，但包成 struct 之后 Tauri 的 State 要求 Send + Sync。
    // mpsc::Sender 是 !Sync，所以这里用 Mutex 兜一层。只在改设置时发一次，不心疼。
    pub struct SharedHotkeys(pub Mutex<Hotkeys>);

    /// 起一个专用线程来收快捷键。
    ///
    /// 为什么必须是专用线程：`RegisterHotKey` 注册到**调用它的那个线程**上，
    /// `WM_HOTKEY` 也只投递到那个线程的消息队列。绑在主线程上就得去钩
    /// Tauri 的消息循环，而那是不公开的；自己在后台线程里建一个消息队列最省事。
    pub fn spawn_hotkeys<F, G>(on_fire: F, on_fail: G) -> Hotkeys
    where
        F: Fn(u32) + Send + 'static,
        G: Fn(u32, String) + Send + 'static,
    {
        let (tx, rx) = channel::<Vec<Binding>>();
        std::thread::Builder::new()
            .name("paypulse-hotkeys".into())
            .spawn(move || pump(rx, on_fire, on_fail))
            .ok();
        Hotkeys { tx }
    }

    impl Hotkeys {
        /// 换一批绑定。已经注册的会先全部注销。
        pub fn set(&self, list: Vec<Binding>) {
            let _ = self.tx.send(list);
            // 线程可能正阻塞在 GetMessage 上，捅一下让它醒过来取新绑定。
            // 线程还没起来时 TID 是 0，这一下会失败，但那时线程的第一轮
            // `try_recv` 也会自己捡到这条。
            let tid = THREAD_ID.load(std::sync::atomic::Ordering::SeqCst);
            if tid != 0 {
                unsafe {
                    PostThreadMessageW(tid, WM_APP, 0, 0);
                }
            }
        }
    }

    fn pump<F, G>(rx: Receiver<Vec<Binding>>, on_fire: F, on_fail: G)
    where
        F: Fn(u32),
        G: Fn(u32, String),
    {
        unsafe {
            THREAD_ID.store(GetCurrentThreadId(), std::sync::atomic::Ordering::SeqCst);
        }
        crate::log::w(&format!(
            "快捷键线程已启动，tid={}",
            THREAD_ID.load(std::sync::atomic::Ordering::SeqCst)
        ));
        let mut current: Vec<Binding> = Vec::new();
        loop {
            // 1. 把攒下的新绑定全部取走（可能不止一条：改设置时连着按几下）
            while let Ok(next) = rx.try_recv() {
                unsafe {
                    for b in &current {
                        UnregisterHotKey(std::ptr::null_mut(), b.id as i32);
                    }
                }
                let mut ok = Vec::new();
                for b in next.into_iter() {
                    let r = unsafe {
                        RegisterHotKey(std::ptr::null_mut(), b.id as i32, b.mods, b.vk)
                    };
                    if r != 0 {
                        crate::log::w(&format!("快捷键 {}（id {}）注册成功", b.label, b.id));
                        ok.push(b);
                    } else {
                        // 被别人占了（输入法、录屏软件都爱占 Ctrl+Shift+字母）。
                        // 另外几个照常工作，但**不能静默**：原型稿第 2790 行写着
                        // 「注册失败要当场提示冲突」。只写日志等于没提示——GUI 程序
                        // 没有控制台，用户看到的现象就是「这个键按了没反应」。
                        let err = std::io::Error::last_os_error();
                        crate::log::w(&format!(
                            "快捷键 {}（id {}）注册失败：{err}（多半被别的程序占用）",
                            b.label, b.id
                        ));
                        on_fail(b.id, b.label.clone());
                    }
                }
                current = ok;
            }

            // 2. 阻塞在这里等消息。
            //
            // 原来是「PeekMessage 轮询 + 20ms sleep」，实测在 release 构建里
            // 注册会成功、但 WM_HOTKEY 一条都收不到（debug 构建却好的），
            // 见 README 10.4。改成阻塞式 `GetMessageW`：消息到了立刻醒，
            // 不再有「这一轮没轮到」的可能。
            //
            // 换绑定时用 `PostThreadMessageW(WM_APP)` 把线程叫醒——
            // 阻塞在 GetMessage 上时收不到 mpsc，得有人捅它一下。
            unsafe {
                let mut msg: Msg = std::mem::zeroed();
                let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                if r > 0 && msg.message == WM_HOTKEY {
                    on_fire(msg.wparam as u32);
                }
                // r == 0 是 WM_QUIT，-1 是出错。两种都继续转：
                // 这个线程没有别的活可干，退出等于快捷键全废。
            }
        }
    }
}

// ---------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
mod imp {
    use super::Binding;
    use std::path::Path;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex;

    // ---- 手写的 macOS 绑定 ----
    //
    // 和 Windows 侧同一个原则：不引 objc / cocoa / core-foundation 这些 crate，
    // 只链系统框架（CoreGraphics、Carbon、CoreFoundation）里的几个符号。
    // macOS 上的框架链接在 rustc 里写 `#[link(name = "…", kind = "framework")]`。

    /// CoreFoundation 的 `CFStringRef`（不可变字符串）。这里只当不透明指针用。
    type CfStringRef = *mut std::ffi::c_void;
    /// CoreFoundation 的 `CFTypeRef`（任意 CF 对象）。
    type CfTypeRef = *mut std::ffi::c_void;

    /// 空闲检测用：`kCGEventSourceStateHIDSystemState` = 1（系统 HID 事件源）。
    const HID_SYSTEM_STATE: i32 = 1;
    /// `kCGAnyInputEventType` = ~0，表示「任意输入（键鼠/触控板/数位笔）」。
    const ANY_INPUT_EVENT_TYPE: u32 = u32::MAX;

    /// CoreGraphics 的 `CGPoint`（`x`、`y` 都是 `CGFloat` = f64）。
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    /// CoreGraphics 的 `CGSize`。
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    /// CoreGraphics 的 `CGRect`。`origin` 是左上角，`size` 是宽高。
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        /// 距上一次指定类型的输入事件过去多少秒。空闲检测的核心。
        /// `source` 传 `kCGEventSourceStateHIDSystemState`（1），
        /// `event_type` 传 `kCGAnyInputEventType`（~0）。
        fn CGEventSourceSecondsSinceLastEventType(source: i32, event_type: u32) -> f64;

        /// 当前鼠标全局位置（逻辑坐标，原点在主屏左上，Y 向下）。
        /// `CGEventCreate(nil)` 造一个空事件，再取它的位置——这是官方惯用法。
        fn CGEventCreate(source: CfTypeRef) -> CfTypeRef;
        fn CGEventGetLocation(event: CfTypeRef) -> CGPoint;
        fn CFRelease(cf: CfTypeRef);

        /// 主屏 / 屏幕列表。`work_area` 需要「可见区」而不是「整块屏」。
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    /// 鼠标当前屏幕坐标。macOS 的 `CGEventGetLocation` 返回的是**逻辑像素**，
    /// 与菜单窗口落位用的 `PhysicalPosition` 口径一致（Tauri 在 macOS 上
    /// 的逻辑/物理像素默认 1:1，除非开了缩放，菜单紧贴鼠标仍能对上）。
    pub fn cursor_pos() -> (i32, i32) {
        unsafe {
            let ev = CGEventCreate(std::ptr::null_mut());
            if ev.is_null() {
                return (0, 0);
            }
            let p = CGEventGetLocation(ev);
            CFRelease(ev);
            (p.x.round() as i32, p.y.round() as i32)
        }
    }

    /// 距离最后一次键鼠活动过去多少秒。取整到秒。
    pub fn idle_seconds() -> u64 {
        unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT_TYPE) as u64 }
    }

    /// 主屏可见区（逻辑坐标，左上角 + 宽高 → 转成左/上/右/下）。
    ///
    /// macOS 没有「工作区」概念（菜单栏不是任务栏，不会遮住窗口），
    /// 这里退化为「主屏边界」：贴边逻辑在 mac 上贴的是屏幕边，足够用。
    /// `hwnd` 参数在 mac 上没用（不按窗口定位屏），保留签名只为统一调用。
    pub fn work_area(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
        unsafe {
            let b = CGDisplayBounds(CGMainDisplayID());
            if b.size.width <= 0.0 || b.size.height <= 0.0 {
                return None;
            }
            Some((
                b.origin.x.round() as i32,
                b.origin.y.round() as i32,
                (b.origin.x + b.size.width).round() as i32,
                (b.origin.y + b.size.height).round() as i32,
            ))
        }
    }

    /// 某个点所在的屏。macOS 上用 CGDisplayBounds 逐屏判断成本高，
    /// 且多屏拖拽在这个产品里优先级低，先返回主屏 id 的近似。
    /// 返回值约定与 Windows 一致：0 表示「没有屏/退化成默认」。
    pub fn monitor_at(_x: i32, _y: i32) -> isize {
        // 简化：把主屏当成唯一「有屏」的位置。返回值非 0 表示「这儿有屏」。
        1
    }

    /// 窗口在哪块屏。macOS 上同样简化为「主屏」。
    pub fn monitor_of(_hwnd: isize) -> isize {
        1
    }

    /// 让一块矩形重画。macOS 的窗口系统（Quartz）会在窗口移动后自动重绘，
    /// 不需要像 Windows 那样手动 `RedrawWindow`——空实现即可。
    pub fn repaint_rect(_left: i32, _top: i32, _right: i32, _bottom: i32) {}

    // ---- 开机自启：LaunchAgent ----

    /// LaunchAgent 的 plist 路径。`~/Library/LaunchAgents/`。
    fn launch_agent_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/unknown".into());
        std::path::PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join("com.bydota.paypulse.plist")
    }

    /// 开 / 关开机自启。
    ///
    /// macOS 的「开机自启」正规做法是 LaunchAgent：在 `~/Library/LaunchAgents/`
    /// 写一个 plist，登录时 launchd 拉起。只写这一处，删掉这个文件就是干净的卸载。
    pub fn set_autostart(on: bool, exe: &Path) -> Result<(), String> {
        let path = launch_agent_path();
        if !on {
            // 已经不存在也算成功：目的达到了
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("删 LaunchAgent 失败（{e}）"))?;
            }
            return Ok(());
        }

        let dir = path.parent().ok_or("LaunchAgents 目录不存在")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("建 LaunchAgents 目录失败（{e}）"))?;

        // plist 内容：RunAtLoad=true 表示登录即启动。exe 路径转 XML 转义。
        let exe_str = exe.to_string_lossy();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key><string>com.bydota.paypulse</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array><string>{exe_str}</string></array>\n\
             \t<key>RunAtLoad</key><true/>\n\
             </dict>\n\
             </plist>\n"
        );
        std::fs::write(&path, plist).map_err(|e| format!("写 LaunchAgent 失败（{e}）"))
    }

    // ---- 全局快捷键：RegisterEventHotKey（Carbon） ----

    /// `EventHotKeyRef` 的寄存器。每个注册的快捷键一个。
    /// 实际是 Carbon 的不透明句柄，这里用指针存。
    type EventHotKeyRef = *mut std::ffi::c_void;
    /// Carbon 的事件目标。传空表示「系统事件目标」。
    type EventTargetRef = *mut std::ffi::c_void;

    /// `EventHotKeyID`：`signature`（四字符）+ `id`。回调靠 `id` 分派。
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventHotKeyId {
        signature: u32,
        id: u32,
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        /// 注册一个全局快捷键。`hotKeyID` 用 `id` 编码，回调时原样带回。
        /// `options` 传 0；`modifiers` 用 Carbon 的修饰键位掩码（见下）。
        fn RegisterEventHotKey(
            hot_key_code: u32,
            modifiers: u32,
            hot_key_id: EventHotKeyId,
            target: EventTargetRef,
            options: u32,
            out_ref: *mut EventHotKeyRef,
        ) -> i32;
        fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> i32;
    }

    /// Carbon 修饰键位（与 `parse()` 出来的 Win32 修饰键不同，需要转换）。
    const CMD_KEY: u32 = 1 << 8; // cmdKey
    const SHIFT_KEY: u32 = 1 << 9; // shiftKey
    const OPTION_KEY: u32 = 1 << 11; // optionKey
    const CONTROL_KEY: u32 = 1 << 12; // controlKey

    /// 把 Win32 修饰键掩码转成 Carbon 的修饰键掩码。
    ///
    /// `parse()` 的约定：MOD_ALT=1, MOD_CONTROL=2, MOD_SHIFT=4, MOD_WIN=8。
    /// macOS 上把 Win 键（Super）映射成 Cmd 键——这是用户直觉里「Win/Cmd」的位置。
    fn to_carbon_mods(mods: u32) -> u32 {
        let mut m = 0;
        if mods & 1 != 0 {
            m |= OPTION_KEY;
        }
        if mods & 2 != 0 {
            m |= CONTROL_KEY;
        }
        if mods & 4 != 0 {
            m |= SHIFT_KEY;
        }
        if mods & 8 != 0 {
            m |= CMD_KEY;
        }
        m
    }

    /// 主键 → macOS 虚拟键码。macOS 的键码布局和 Windows 的 VK 不同，
    /// 这里做一层映射：字母用 `kVK_ANSI_A`=0x00 …，数字用 `kVK_ANSI_0`=0x1D …，
    /// 功能键 `kVK_F1`=0x7A …，方向键、回车、退格等各有一套。
    ///
    /// 注意：这里复用 `parse()` 已经解析好的 Win32 键码（`Binding.vk`），
    /// 所以映射的是「Win32 VK → macOS kVK」，而不是「字符 → kVK」。
    fn to_mac_keycode(vk: u32) -> u32 {
        match vk {
            // 字母 A-Z：Win32 VK 是 'A'..'Z'（0x41..0x5A）
            c @ 0x41..=0x5A => c - 0x41, // kVK_ANSI_A = 0x00
            // 数字 0-9：Win32 VK 是 '0'..'9'（0x30..0x39）
            c @ 0x30..=0x39 => 0x1D + (c - 0x30), // kVK_ANSI_0 = 0x1D
            0x0D => 36, // return
            0x20 => 49, // space
            0x09 => 48, // tab
            0x1B => 53, // escape
            0x08 => 51, // delete (backspace)
            0x2E => 117, // forward delete
            0x2D => 114, // help/insert（近似）
            0x24 => 115, // home
            0x23 => 119, // end
            0x21 => 116, // page up
            0x22 => 121, // page down
            0x25 => 123, // left
            0x26 => 126, // up
            0x27 => 124, // right
            0x28 => 125, // down
            // F1-F12：Win32 VK 0x70..0x7B，mac kVK_F1=0x7A
            c @ 0x70..=0x7B => 0x7A + (c - 0x70),
            _ => 0, // 认不出的键退回 0，RegisterEventHotKey 会失败并提示
        }
    }

    /// 快捷键线程：和 Windows 一样用专用线程 + mpsc 通道推绑定。
    ///
    /// 差别：Carbon 的 `RegisterEventHotKey` 是全局注册（不绑线程消息队列），
    /// 回调通过一个事件处理器进来。这里简化成「注册后，用 mpsc 接收换绑」，
    /// 触发回调的部分依赖 Carbon 的 `InstallEventHandler`（见下）。
    pub struct Hotkeys {
        tx: Sender<Vec<Binding>>,
    }

    pub struct SharedHotkeys(pub Mutex<Hotkeys>);

    pub fn spawn_hotkeys<F, G>(on_fire: F, on_fail: G) -> Hotkeys
    where
        F: Fn(u32) + Send + 'static,
        G: Fn(u32, String) + Send + 'static,
    {
        let (tx, rx) = channel::<Vec<Binding>>();
        std::thread::Builder::new()
            .name("paypulse-hotkeys".into())
            .spawn(move || pump_mac(rx, on_fire, on_fail))
            .ok();
        Hotkeys { tx }
    }

    impl Hotkeys {
        pub fn set(&self, list: Vec<Binding>) {
            let _ = self.tx.send(list);
        }
    }

    fn pump_mac<F, G>(rx: Receiver<Vec<Binding>>, on_fire: F, on_fail: G)
    where
        F: Fn(u32),
        G: Fn(u32, String),
    {
        // 维护当前已注册的快捷键，换绑时先全部注销。
        let mut current: Vec<(EventHotKeyRef, Binding)> = Vec::new();
        // 换绑信号：阻塞式 recv，但带超时，让线程有空处理别的（目前只有换绑）。
        // macOS 上 RegisterEventHotKey 的回调依赖 Carbon 事件处理器 + RunLoop，
        // 那部分需要和 Tauri 的主 RunLoop 集成，属于 mac 真机联调阶段。
        // 这里先把「注册 / 注销 / 换绑」做对：注册成功后系统就能捕获热键，
        // 只是触发回调的事件处理器尚待接入（见 README「mac 版」一节）。
        loop {
            // 等一条新绑定（阻塞直到有换绑请求）。
            let Ok(next) = rx.recv() else {
                break;
            };
            // 注销旧的。
            unsafe {
                for (r, _) in current.drain(..) {
                    UnregisterEventHotKey(r);
                }
            }
            // 注册新的。
            for b in next.into_iter() {
                let mac_mods = to_carbon_mods(b.mods);
                let mac_vk = to_mac_keycode(b.vk);
                let mut out_ref: EventHotKeyRef = std::ptr::null_mut();
                let hot_key_id = EventHotKeyId {
                    signature: 0x504C_5345, // 'PLSE'
                    id: b.id,
                };
                let rc = unsafe {
                    RegisterEventHotKey(
                        mac_vk,
                        mac_mods,
                        hot_key_id,
                        std::ptr::null_mut(),
                        0,
                        &mut out_ref,
                    )
                };
                if rc == 0 && !out_ref.is_null() {
                    crate::log::w(&format!("快捷键 {}（id {}）注册成功", b.label, b.id));
                    current.push((out_ref, b));
                } else {
                    crate::log::w(&format!(
                        "快捷键 {}（id {}）注册失败：rc={rc}（多半被别的程序占用）",
                        b.label, b.id
                    ));
                    on_fail(b.id, b.label.clone());
                }
            }
            // `on_fire` 在 macOS 上由 Carbon 事件处理器驱动；接入前，
            // 这里用 `_ = &on_fire;` 保留闭包所有权，避免「未使用」告警。
            let _ = &on_fire;
        }
    }
}

// ---------------------------------------------------------------- Linux 等其它平台

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::Binding;
    use std::path::Path;
    use std::sync::Mutex;

    pub fn set_autostart(_on: bool, _exe: &Path) -> Result<(), String> {
        Err("这个平台暂不支持开机自启".into())
    }

    pub fn idle_seconds() -> u64 {
        0
    }

    pub fn work_area(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
        None
    }

    pub fn monitor_at(_x: i32, _y: i32) -> isize {
        0
    }

    pub fn monitor_of(_hwnd: isize) -> isize {
        0
    }

    pub fn repaint_rect(_left: i32, _top: i32, _right: i32, _bottom: i32) {}

    pub fn cursor_pos() -> (i32, i32) {
        (0, 0)
    }

    pub struct Hotkeys;

    pub struct SharedHotkeys(pub Mutex<Hotkeys>);

    pub fn spawn_hotkeys<F, G>(_on_fire: F, _on_fail: G) -> Hotkeys
    where
        F: Fn(u32) + Send + 'static,
        G: Fn(u32, String) + Send + 'static,
    {
        Hotkeys
    }

    impl Hotkeys {
        pub fn set(&self, _list: Vec<Binding>) {}
    }
}

pub use imp::{
    cursor_pos, idle_seconds, monitor_at, monitor_of, repaint_rect, set_autostart, spawn_hotkeys,
    work_area, SharedHotkeys,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_defaults() {
        assert_eq!(parse("Ctrl+Shift+P"), Some((2 | 4, b'P' as u32)));
        assert_eq!(parse("Ctrl+Shift+Enter"), Some((2 | 4, 0x0D)));
        assert_eq!(parse("ctrl+shift+e"), Some((2 | 4, b'E' as u32)));
    }

    /// 没有修饰键的全局快捷键会抢走整个系统的那个键，一律不认。
    #[test]
    fn bare_keys_are_rejected() {
        assert_eq!(parse("P"), None);
        assert_eq!(parse("F5"), None);
    }

    /// 写错一个不该让另外两个也不好使——调用方靠 `None` 跳过这一个。
    #[test]
    fn garbage_is_rejected_not_guessed() {
        assert_eq!(parse("Ctrl+Shift+"), None);
        assert_eq!(parse("Ctrl+P+Q"), None);
        assert_eq!(parse("Ctrl+Shift+F13"), None);
        assert_eq!(parse("Hyper+P"), None);
    }

    #[test]
    fn function_keys_map_to_their_range() {
        assert_eq!(parse("Ctrl+F1"), Some((2, 0x70)));
        assert_eq!(parse("Ctrl+F12"), Some((2, 0x7B)));
    }

    #[test]
    fn alternatives_spell_the_same_modifier() {
        assert_eq!(parse("Control + Shift + P"), parse("CTRL+SHIFT+P"));
        assert_eq!(parse("Win+Shift+P"), Some((8 | 4, b'P' as u32)));
    }
}

