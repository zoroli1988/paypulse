//! 单实例保护：双击两次图标，不该出现两个悬浮窗。
//!
//! ## 为什么必须有
//!
//! 没有这层保护时，用户双击两次 exe（或者装完之后从桌面图标再启动一次）会得到：
//!
//! - 两个悬浮窗、两个托盘图标，都在按同一份配置算钱——看起来像软件发疯了；
//! - 两套全局热键注册，第二套全部失败（`os error 1409` 热键已注册）；
//! - 两个进程各自往 `settings.json` 里写（走的是 tmp + rename 原子写，
//!   文件不会写坏，但**后写的会盖掉先写的**，用户会看到「刚改的设置又跳回去了」）；
//! - 两份 60s 心跳同时更新 `last_alive`，补记问询的判断依据变得不可信。
//!
//! ## 三个平台三种做法
//!
//! - **Windows**：命名互斥量。内核维护，进程一死（包括被强杀）句柄自动回收，
//!   不存在「陈旧锁文件」的麻烦事。名字用 `Local\` 而不是 `Global\`——
//!   `Local\` 是「每个登录会话一个」，同一台机器上两个用户各开一份互不干扰；
//!   `Global\` 会让第二个登录的用户永远打不开程序。
//! - **Linux**：抽象命名空间的 Unix socket 占坑。抽象 socket 不落文件系统，
//!   进程退出由内核释放，同样没有残留文件问题。
//! - **macOS**：什么都不做。双击 `.app` 本来就由 LaunchServices 激活已有实例，
//!   系统层面就起不了第二个；主动退出反而会让 `open -n` 这类正常用法失效。

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;

    /// 互斥量名字。带产品标识，别用泛名（`paypulse` 太短，容易跟别的程序撞）。
    const MUTEX_NAME: &str = "Local\\com.bydota.paypulse.single-instance";

    /// 悬浮窗标题，用来「唤出已有窗口」。
    ///
    /// 必须和 `tauri.conf.json` 里 `windows[0].title` 一字不差。改标题时这里要跟着改，
    /// 否则第二个实例退出了、却没能把已有窗口叫到前面——用户看到的是「双击了没反应」。
    const HUD_TITLE: &str = "牛马薪动了！";

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_SHOW: i32 = 5;
    const SW_RESTORE: i32 = 9;

    type Handle = *mut c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
        fn GetLastError() -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
        fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: Handle) -> i32;
        fn IsIconic(hwnd: Handle) -> i32;
        fn IsWindowVisible(hwnd: Handle) -> i32;
        fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `CreateMutexW` 之后紧接着读 `GetLastError` 是 Windows 的惯用法：
    /// 句柄总能拿到，只有 `ERROR_ALREADY_EXISTS` 才说明「这个名字已经有人在用」。
    /// 也就是说这两个调用之间不能插任何其它 Win32 调用，否则错误码会被覆盖。
    pub fn claim_named(name: &str) -> bool {
        let name = wide(name);
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if h.is_null() {
                // 建不出来（极少见，多半是句柄耗尽）。宁可放行，也不要卡住用户。
                return true;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // 不是我们的句柄，别 CloseHandle——关掉别人还在用的句柄后果难料。
                return false;
            }
            // 句柄故意不关：一关就等于放弃了「我在跑」这个声明。
            // 进程退出时由内核回收，正是我们要的生命周期。
            // （这里是裸指针，没有 Drop，所以也不需要 mem::forget —— 写了反而多余。）
            true
        }
    }

    pub fn claim() -> bool {
        claim_named(MUTEX_NAME)
    }

    /// 把已经在跑的那个实例叫到前面来。
    ///
    /// 顺序有讲究：先 `ShowWindow` 再 `SetForegroundWindow`。只调后者的话，
    /// 窗口处于隐藏状态（用户用「隐藏悬浮窗」收起来了）时不会出现，用户会觉得没反应。
    /// 最小化的先 `SW_RESTORE`，否则它只是任务栏上闪一下。
    ///
    /// 返回 false 表示没找到窗口（可能还在启动、或用户把悬浮窗关了）。
    pub fn activate() -> bool {
        let title = wide(HUD_TITLE);
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            SetForegroundWindow(hwnd);
            true
        }
    }

    /// 已有实例的进程号，只用于写日志（诊断「到底是谁在占着」）。
    pub fn existing_pid() -> u32 {
        let title = wide(HUD_TITLE);
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                return 0;
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener};

    /// 抽象命名空间的 socket 名（`@` 前缀是这套 API 对「无路径」的写法）。
    /// 抽象 socket 不落文件系统，进程一退内核就释放——所以不需要处理陈旧锁。
    fn addr_name() -> String {
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        format!("@paypulse-single-{user}")
    }

    /// 占坑成功 = 我是第一个。占坑失败（地址已在用）= 已经有实例。
    ///
    /// 拿到 listener 之后**不能 drop**：drop 就等于关掉 socket、释放占坑。
    /// 这里把它交给全局，活到进程结束。
    pub fn claim() -> bool {
        let name = addr_name();
        let Ok(addr) = SocketAddr::from_abstract_name(name.as_bytes()) else {
            return true; // 建不出地址就别拦用户
        };
        match UnixListener::bind_addr(&addr) {
            Ok(l) => {
                let _ = LOCK.set(l);
                true
            }
            Err(_) => false,
        }
    }

    /// 用 OnceLock 而不是 thread_local：`claim()` 在 `run()` 开头调用，
    /// 万一将来挪到别的线程也还成立。
    static LOCK: std::sync::OnceLock<UnixListener> = std::sync::OnceLock::new();

    /// Linux 上「唤出已有窗口」要做对得分别处理 X11 / Wayland / 各桌面环境的
    /// 激活协议（`_NET_ACTIVE_WINDOW`、xdg-activation…），为一个双开场景不值当。
    /// 第二个实例安静退出，日志里写明原因。
    pub fn activate() -> bool {
        false
    }

    pub fn existing_pid() -> u32 {
        0
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// macOS 由 LaunchServices 保证单实例，这里不拦。
    pub fn claim() -> bool {
        true
    }

    pub fn activate() -> bool {
        false
    }

    pub fn existing_pid() -> u32 {
        0
    }
}

/// 拿到「我在跑」的资格就返回 true；已经有实例在跑返回 false。
pub fn claim() -> bool {
    imp::claim()
}

/// 把已有实例叫到前面来。返回是否真的找到了窗口。
pub fn activate() -> bool {
    imp::activate()
}

/// 已有实例的 PID；0 表示不知道（非 Windows，或没找到窗口）。
pub fn existing_pid() -> u32 {
    imp::existing_pid()
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    use super::imp::claim_named;

    /// 同一个名字只能被占住一次——这正是「第二个实例该退出」的判据。
    ///
    /// 名字里带随机后缀，避免和**正在运行的正式程序**抢同一个互斥量：
    /// 否则用户开着应用跑 `cargo test`，这条用例就会红，而且红得莫名其妙。
    #[test]
    #[cfg(target_os = "windows")]
    fn the_second_claim_on_the_same_name_fails() {
        let name = format!(
            "Local\\com.bydota.paypulse.test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        assert!(claim_named(&name), "第一个应该成功");
        assert!(!claim_named(&name), "第二个必须失败（同一次进程内也算占用）");
    }

    /// 不同名字互不影响：多开测试/多用户场景不该互相顶掉。
    #[test]
    #[cfg(target_os = "windows")]
    fn different_names_do_not_collide() {
        let base = std::process::id();
        assert!(claim_named(&format!("Local\\com.bydota.paypulse.test-a-{base}")));
        assert!(claim_named(&format!("Local\\com.bydota.paypulse.test-b-{base}")));
    }
}
