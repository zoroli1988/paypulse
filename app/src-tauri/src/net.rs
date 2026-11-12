//! 一个只做「GET 一个 https 地址」的极小 HTTP 客户端。
//!
//! ## 为什么不继续用前端 `fetch`
//!
//! 上一版是 `fetch()` 拉清单，结果用户点「检查更新」只得到一句
//! **`Failed to fetch`**。这不是 HTTP 错误（那会显示 `HTTP 404`），而是浏览器
//! 在传输层就放弃了：清单挂在 GitHub Releases 上，`github.com/.../download/...`
//! 会 302 到 `objects.githubusercontent.com`，而后者**不带 CORS 头**。
//! WebView 里的 `fetch` 受同源策略约束，跨域响应缺 `Access-Control-Allow-Origin`
//! 就直接抛这个 TypeError——请求根本没发出去，服务端日志里也看不到。
//!
//! 原生代码没有同源策略。所以把这一次 GET 挪进来：CORS 这一类故障从此不存在，
//! 顺带还能把「连不上 / 404 / 证书错」分开报清楚——前端只能说「失败了」。
//!
//! ## 为什么不引 reqwest / ureq
//!
//! 和 `platform.rs` 同一个理由：这条路跑在 `x86_64-pc-windows-gnu` 上，
//! reqwest 会拉 tokio + hyper + rustls 一大套；ureq 也要带一个 TLS 后端。
//! 而我们要的能力只有一个：**跟着 302 走、把 body 读完**。
//!
//! * Windows：`WinHTTP`——系统自带，schannel 负责 TLS，自动跟随重定向，
//!   自动吃系统代理设置（国内用户很多靠代理访问 GitHub，这一点很重要）。
//! * macOS / Linux：调系统自带的 `curl`。写一套 CFNetwork 绑定只为发一个 GET
//!   不划算，而 `curl` 在那两个系统上一定存在。
//!
//! ## 这条线只用来「问一句有没有新版本」
//!
//! 它不发任何用户数据，请求里只有 User-Agent（版本号）。清单拉下来之后
//! 立刻回到 `update.rs` 走 Ed25519 验签——HTTPS 只保证「没被中间人换掉传输内容」，
//! 签名才保证「这份清单是我们签的」。

/// 一次 GET。`timeout_ms` 是总预算（连接 + 收完）。
///
/// 返回 body 文本。错误信息是给人看的：它会原样出现在设置页那一格里，
/// 所以写「连不上更新服务器（网络或代理）」而不是 `ERROR_WINHTTP_...`。
pub fn get(url: &str, timeout_ms: u32) -> Result<String, String> {
    let (host, path) = split_url(url)?;
    imp::get(&host, &path, timeout_ms)
}

/// 把 `https://host/a/b?c` 拆成 `(host, /a/b?c)`。
///
/// 只认 https：清单必须走 TLS。http 一律拒绝——不是洁癖，是「检查更新」这条路
/// 一旦能被降级成明文，中间人就能把清单换成指向任意安装包的版本（虽然还有验签兜底，
/// 但没必要留这条路）。
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "更新地址必须是 https".to_string())?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() || host.contains(' ') {
        return Err("更新地址里的主机名不合法".into());
    }
    Ok((host.to_string(), path.to_string()))
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    type Handle = *mut c_void;
    type Bool = i32;

    /// 「自动代理」：Win8.1 起支持，会读系统的 PAC/WPAD。
    const ACCESS_AUTOMATIC_PROXY: u32 = 4;
    /// 退路：读注册表里的 IE 代理设置。老系统上只有这一档。
    const ACCESS_DEFAULT_PROXY: u32 = 0;
    const FLAG_SECURE: u32 = 0x0080_0000;
    const HTTPS_PORT: u16 = 443;
    /// 查询响应状态码
    const QUERY_STATUS_CODE: u32 = 19;
    const QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    /// 清单正常只有几百字节。封顶 256 KB：一个「检查更新」不该能吃满内存，
    /// 也不该被一个坏掉的 CDN 拖着读几十兆。
    const MAX_BODY: usize = 256 * 1024;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access_type: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            connect: Handle,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept_types: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSendRequest(
            request: Handle,
            headers: *const u16,
            headers_len: u32,
            optional: *const c_void,
            optional_len: u32,
            total_len: u32,
            context: usize,
        ) -> Bool;
        fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> Bool;
        fn WinHttpQueryHeaders(
            request: Handle,
            info_level: u32,
            name: *const u16,
            buffer: *mut c_void,
            buffer_len: *mut u32,
            index: *mut u32,
        ) -> Bool;
        fn WinHttpReadData(
            request: Handle,
            buffer: *mut c_void,
            to_read: u32,
            read: *mut u32,
        ) -> Bool;
        fn WinHttpSetTimeouts(
            session: Handle,
            resolve: i32,
            connect: i32,
            send: i32,
            receive: i32,
        ) -> Bool;
        fn WinHttpCloseHandle(handle: Handle) -> Bool;
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
    }

    pub fn get(host: &str, path: &str, timeout_ms: u32) -> Result<String, String> {
        let agent = wide(&format!("PayPulse/{}", env!("CARGO_PKG_VERSION")));
        let host_w = wide(host);
        let path_w = wide(path);
        let verb = wide("GET");

        unsafe {
            let mut session = WinHttpOpen(
                agent.as_ptr(),
                ACCESS_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            );
            if session.is_null() {
                // 老系统不认 AUTOMATIC_PROXY，退回注册表代理
                session = WinHttpOpen(
                    agent.as_ptr(),
                    ACCESS_DEFAULT_PROXY,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                );
            }
            if session.is_null() {
                return Err("初始化网络失败".into());
            }
            let t = timeout_ms.min(i32::MAX as u32) as i32;
            WinHttpSetTimeouts(session, t, t, t, t);

            let connect = WinHttpConnect(session, host_w.as_ptr(), HTTPS_PORT, 0);
            if connect.is_null() {
                WinHttpCloseHandle(session);
                return Err("连不上更新服务器".into());
            }

            let request = WinHttpOpenRequest(
                connect,
                verb.as_ptr(),
                path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                FLAG_SECURE,
            );
            if request.is_null() {
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("构造请求失败".into());
            }

            // 不传额外头：WinHTTP 自己会带 Host / Accept。
            let mut ok = WinHttpSendRequest(
                request,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                0,
                0,
            );
            if ok != 0 {
                ok = WinHttpReceiveResponse(request, std::ptr::null_mut());
            }
            if ok == 0 {
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("请求没发出去（网络不通或被代理拦住）".into());
            }

            // 状态码：非 200 直接报数字。这是上一版最缺的信息——
            // 「Failed to fetch」把所有可能性揉成了一句废话。
            let mut status: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request,
                QUERY_STATUS_CODE | QUERY_FLAG_NUMBER,
                std::ptr::null(),
                &mut status as *mut u32 as *mut c_void,
                &mut len,
                std::ptr::null_mut(),
            );

            let mut body: Vec<u8> = Vec::new();
            if status == 200 {
                let mut buf = vec![0u8; 8 * 1024];
                loop {
                    let mut read: u32 = 0;
                    let r = WinHttpReadData(
                        request,
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len() as u32,
                        &mut read,
                    );
                    if r == 0 || read == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..read as usize]);
                    if body.len() >= MAX_BODY {
                        break;
                    }
                }
            }

            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);

            if status != 200 {
                // 404 是最常见的一种：tag 发了但 manifest 没上传，或者版本还没发过。
                return Err(if status == 0 {
                    "没拿到服务器响应".to_string()
                } else {
                    format!("更新服务器回了 HTTP {status}")
                });
            }
            String::from_utf8(body).map_err(|_| "清单不是 UTF-8 文本".to_string())
        }
    }
}

// ---------------------------------------------------------------- macOS / 其它

#[cfg(not(windows))]
mod imp {
    pub fn get(host: &str, path: &str, timeout_ms: u32) -> Result<String, String> {
        let url = format!("https://{host}{path}");
        let out = std::process::Command::new("curl")
            .args([
                "-sS",              // 安静，但把错误打到 stderr
                "-L",               // 跟着 302 走（GitHub 的资产都靠重定向）
                "--max-time",
                &(timeout_ms / 1000).max(5).to_string(),
                "--fail",           // 4xx/5xx 直接算失败，别把错误页当清单
                &url,
            ])
            .output()
            .map_err(|e| format!("起不来 curl（{e}）"))?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if why.is_empty() {
                "连不上更新服务器".to_string()
            } else {
                format!("连不上更新服务器：{why}")
            });
        }
        String::from_utf8(out.stdout).map_err(|_| "清单不是 UTF-8 文本".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_host_and_path() {
        assert_eq!(
            split_url("https://github.com/a/b/c.json").unwrap(),
            ("github.com".to_string(), "/a/b/c.json".to_string())
        );
        // 带查询串的整段都要算进 path
        assert_eq!(
            split_url("https://x.y/p?q=1").unwrap(),
            ("x.y".to_string(), "/p?q=1".to_string())
        );
        // 没有路径：补一个 `/`，否则请求行会是空的
        assert_eq!(split_url("https://x.y").unwrap(), ("x.y".to_string(), "/".to_string()));
    }

    /// 明文一律拒绝：这条路上不允许被降级。
    #[test]
    fn plain_http_is_rejected() {
        assert!(split_url("http://github.com/x").is_err());
        assert!(split_url("github.com/x").is_err());
    }

    #[test]
    fn empty_host_is_rejected() {
        assert!(split_url("https:///path").is_err());
    }

    /// 真联网跑一次取清单——**默认跳过**，因为它要外网，CI 和离线机器上会红。
    ///
    /// 存在的原因很具体：`Failed to fetch` 这个 bug 的修法是把 GET 从 WebView
    /// 挪到原生（WinHTTP / curl）。而「原生能不能连通」恰恰是 curl 通了也不能
    /// 证明的那件事——WinHTTP 走的是系统代理与 schannel，跟 curl 不是一套。
    /// 所以留一条能手动打的用例：
    ///
    /// ```text
    /// PAYPULSE_NET_TEST=1 cargo test --lib -- --ignored --nocapture real_get
    /// ```
    #[test]
    #[ignore = "要联网，手动跑：PAYPULSE_NET_TEST=1 cargo test --lib -- --ignored real_get"]
    fn real_get_reaches_the_manifest() {
        if std::env::var("PAYPULSE_NET_TEST").is_err() {
            eprintln!("没设 PAYPULSE_NET_TEST=1，跳过");
            return;
        }
        let body = get(crate::update::UPDATE_URL, 12_000).expect("取清单失败");
        eprintln!("拿到 {} 字节：{}", body.len(), &body[..body.len().min(120)]);
        // 清单是个 JSON，至少得有 latest 字段，不然说明拿回来的不是它
        assert!(body.contains("\"latest\""), "响应里没有 latest 字段：{body}");
    }
}
