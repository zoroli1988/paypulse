/// 系统的「选择文件夹」对话框（IFileOpenDialog + FOS_PICKFOLDERS）。
///
/// 为什么不用 SHBrowseForFolder：那是 XP 时代的树，在 Windows 11 上照旧长那样，
/// 在一张深色卡片上点出一个 2005 年的对话框，比没有这个按钮更显廉价。
/// IFileOpenDialog 才是资源管理器自己弹的那一个。
///
/// 为什么不引 tauri-plugin-dialog：它会把 rfd 和一整套 ACL 权限文件拖进来，
/// 为一次选目录多加两三百 KB 和一份 capability 配置，不划算。
///
/// 为什么手写 vtable：和 reg 模块同一个原因（本机工具链缺 windows-sys 要的
/// raw-dylib 链路）。COM 的调用约定只有一条：第一个参数是 this，其余按接口顺序排。
/// 这里直接按 vtbl 下标取函数指针，所以下面每个数字都必须和 shobjidl_core.h
/// 的声明顺序对上——顺序错了不会报错，是当场崩。

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }

    // {DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7}  FileOpenDialog
    const CLSID_FILE_OPEN_DIALOG: Guid = Guid {
        d1: 0xDC1C_5A9C,
        d2: 0xE88A,
        d3: 0x4DDE,
        d4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
    };
    // {D57C7288-D4AD-4768-BE02-9D969532D960}  IFileOpenDialog
    const IID_IFILE_OPEN_DIALOG: Guid = Guid {
        d1: 0xD57C_7288,
        d2: 0xD4AD,
        d3: 0x4768,
        d4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
    };
    // {43826D1E-E718-42EE-BC55-A1E261C37BFE}  IShellItem
    const IID_ISHELL_ITEM: Guid = Guid {
        d1: 0x4382_6D1E,
        d2: 0xE718,
        d3: 0x42EE,
        d4: [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE],
    };

    const CLSCTX_INPROC_SERVER: u32 = 1;
    const COINIT_APARTMENTTHREADED: u32 = 2;
    const FOS_PICKFOLDERS: u32 = 0x20;
    const FOS_FORCEFILESYSTEM: u32 = 0x40;
    const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
    /// ERROR_CANCELLED：用户在对话框里按了取消。Show 不是「失败」，是「没选」。
    const HR_CANCELLED: u32 = 0x8007_04C7;

    // IUnknown 占 0/1/2，IModalWindow::Show 是 3，往下是 IFileDialog 自己的方法。
    const VT_RELEASE: usize = 2;
    const VT_SHOW: usize = 3;
    const VT_SET_OPTIONS: usize = 9;
    const VT_SET_FOLDER: usize = 12;
    const VT_SET_TITLE: usize = 17;
    const VT_SET_OK_LABEL: usize = 18;
    const VT_GET_RESULT: usize = 20;
    // IShellItem: IUnknown 0/1/2、BindToHandler 3、GetParent 4、GetDisplayName 5
    const VT_ITEM_DISPLAY_NAME: usize = 5;

    type Hr = i32;
    type Fn1<A> = unsafe extern "system" fn(*mut c_void, A) -> Hr;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hr;
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            ctx: u32,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> Hr;
        fn CoTaskMemFree(p: *mut c_void);
        fn CoUninitialize();
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHCreateItemFromParsingName(
            path: *const u16,
            bind_ctx: *mut c_void,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> Hr;
    }

    unsafe fn slot(obj: *mut c_void, index: usize) -> *const c_void {
        let vtbl = *(obj as *mut *const *const c_void);
        *vtbl.add(index)
    }

    /// 取第 index 个方法。指向函数的指针和普通指针同宽，所以这一跳是安全的。
    unsafe fn method<A>(obj: *mut c_void, index: usize) -> Fn1<A> {
        std::mem::transmute::<*const c_void, Fn1<A>>(slot(obj, index))
    }

    unsafe fn release(obj: *mut c_void) {
        if obj.is_null() {
            return;
        }
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(slot(obj, VT_RELEASE));
        f(obj);
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub fn pick_folder(owner: isize, current: &str) -> Result<Option<String>, String> {
        unsafe {
            // 单线程单元：文件对话框必须是 STA，否则 CoCreateInstance 会挂
            let init = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
            if init < 0 {
                return Err(format!("初始化 COM 失败（0x{:08X}）", init as u32));
            }
            let r = run(owner, current);
            CoUninitialize();
            r
        }
    }

    unsafe fn run(owner: isize, current: &str) -> Result<Option<String>, String> {
        let mut dlg: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_FILE_OPEN_DIALOG,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IFILE_OPEN_DIALOG,
            &mut dlg,
        );
        if hr < 0 || dlg.is_null() {
            return Err(format!("拿不到系统选目录对话框（0x{:08X}）", hr as u32));
        }

        let set_options = method::<u32>(dlg, VT_SET_OPTIONS);
        let set_folder = method::<*mut c_void>(dlg, VT_SET_FOLDER);
        let set_title = method::<*const u16>(dlg, VT_SET_TITLE);
        let set_ok = method::<*const u16>(dlg, VT_SET_OK_LABEL);
        let get_result: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hr =
            std::mem::transmute(slot(dlg, VT_GET_RESULT));
        let show = method::<isize>(dlg, VT_SHOW);

        // 只让选文件系统里的目录：能选到「此电脑」「网络」这种虚拟节点的话，
        // 拿回来的不是路径，后面建目录会失败。
        let _ = set_options(dlg, FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);

        let t = wide("选择安装位置");
        let _ = set_title(dlg, t.as_ptr());
        let ok = wide("选择此文件夹");
        let _ = set_ok(dlg, ok.as_ptr());

        // 打开时停在当前目录。不存在就算了——SHCreateItemFromParsingName 会失败，
        // 对话框退回它自己的默认位置（文档/此电脑），不影响使用。
        let cur = wide(current);
        let mut folder: *mut c_void = std::ptr::null_mut();
        if SHCreateItemFromParsingName(cur.as_ptr(), std::ptr::null_mut(), &IID_ISHELL_ITEM, &mut folder) >= 0
            && !folder.is_null()
        {
            let _ = set_folder(dlg, folder);
            release(folder);
        }

        let hr = show(dlg, owner);
        if hr as u32 == HR_CANCELLED || hr == 1 {
            release(dlg);
            return Ok(None);
        }
        if hr < 0 {
            release(dlg);
            return Err(format!("选目录对话框出错了（0x{:08X}）", hr as u32));
        }

        let mut item: *mut c_void = std::ptr::null_mut();
        let hr = get_result(dlg, &mut item);
        release(dlg);
        if hr < 0 || item.is_null() {
            return Err(format!("读不出选中的目录（0x{:08X}）", hr as u32));
        }

        let get_name: unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> Hr =
            std::mem::transmute(slot(item, VT_ITEM_DISPLAY_NAME));
        let mut pw: *mut u16 = std::ptr::null_mut();
        let hr = get_name(item, SIGDN_FILESYSPATH, &mut pw);
        release(item);
        if hr < 0 || pw.is_null() {
            return Err(format!("这个位置没有文件系统路径（0x{:08X}）", hr as u32));
        }

        // pw 是 COM 分配的内存，必须还给 CoTaskMemFree，不能用 Vec 的释放器
        let mut len = 0usize;
        while *pw.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(pw, len));
        CoTaskMemFree(pw as *mut c_void);
        Ok(Some(s))
    }
}

#[cfg(windows)]
pub use win::pick_folder;

#[cfg(not(windows))]
pub fn pick_folder(_owner: isize, _current: &str) -> Result<Option<String>, String> {
    Err("安装器目前只支持 Windows".into())
}
