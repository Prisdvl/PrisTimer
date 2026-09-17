//! 极简 HTTP 客户端：只做「GET 一个 HTTPS JSON」这一件事。
//!
//! ★ 为什么不引入 `reqwest`：
//!
//!   Tauri 确实默认带 reqwest，但它在本项目里**没有启用 TLS**（Tauri 把
//!   `reqwest` 当纯内部依赖，只在少数 feature 下才拉 rustls/native-tls）。
//!   自己在 `src-tauri/Cargo.toml` 里加 `reqwest = { features = ["json", "rustls-tls"] }`
//!   会引入一整条新依赖链：rustls + aws-lc-rs（一个需要 C 编译器与 cmake 的
//!   巨大构建依赖）/ webpki-roots / hyper / tower / 压缩库…… 对一个只想
//!   「查一个额度」的功能来说，代价完全不成比例。
//!
//!   而 Windows 自带的 **WinHTTP** 已经解决了全部问题：
//!     · TLS 走 schannel，直接复用系统证书库与系统代理设置；
//!     · 默认就支持 gzip/deflate 解压；
//!     · 零新增依赖（`windows` crate 已在依赖树里）；
//!     · 行为与系统一致 —— 用户在系统里配了代理，这里自动跟随。
//!
//! ★★ 为什么现在**不用子进程**（2026-09-17 回归 WinHTTP）：
//!
//!   中间有一版实现改成了 spawn 系统 `curl.exe`（当时的理由是"本机
//!   WinHttpSendRequest 对任何 HTTPS 都返回 ERROR_INVALID_PARAMETER(87)"）。
//!   那个方案有个不能接受的副作用：**Tauri 是 GUI 子系统进程、没有控制台**，
//!   去 spawn 一个控制台程序时 Windows 会为它新建一个控制台窗口 ——
//!   每查一次额度就在用户眼前闪一个黑窗（用户实测：配置时闪、启动时闪两三个）。
//!   设 `Stdio::null()` 并不解决问题，那只重定向标准流，不等于 CREATE_NO_WINDOW。
//!
//!   回归 WinHTTP 是对的（零子进程 = 黑窗从根上消失），但"87 已不复现"
//!   这个判断**当时是错的** —— 探测脚本只发了不带自定义头的请求：
//!     · `get("https://example.com/", &[])`               → 200
//!     · `get("https://opencode.ai/zen/go/v1/usage", &[])` → 401
//!   两条都走"无头"分支，恰好绕开了真正的 bug。带 `Authorization` 的
//!   真实额度查询依然 87，见下面「真凶」一节。
//!
//! ★ 一个必须避开的 windows crate 坑（历史记录，当前版本已不复现）：
//!
//!   `WinHttpOpen` 的签名里 agent 是 `P0: Param<PCWSTR>`，代理两个参数是
//!   `P2/P3: Param<PCWSTR>`。旧版 windows crate 在传 `None` 时会往
//!   `PCWSTR::param()` 里塞 `self.0.as_ptr()` —— 而 `None` 的 `PCWSTR`
//!   内部是空指针，**空指针调 `as_ptr()` 在 debug 构建下会 panic**。
//!
//!   为稳妥起见，代理参数一律传 `PCWSTR::null()`（不用 `None`），
//!   agent 传真实字符串。语义与 WinHTTP 文档一致。
//!
//! ★★ 真凶（2026-09-17 定位并修复）：**头部块的长度把结尾 NUL 也算进去了**
//!
//!   windows 0.61 把 `WinHttpSendRequest` 的 `lpszheaders` 声明成
//!   `Option<&[u16]>`，内部实现是：
//!
//!   ```text
//!   lpszheaders.as_deref().map_or(0, |slice| slice.len().try_into().unwrap())
//!   ```
//!
//!   也就是说 **`dwHeadersLength` = slice 的元素个数**。而本模块给 Win32
//!   用的 `wide()` 会在末尾补一个 NUL 结束符 —— 拿它去传头部，长度就
//!   多算了 1，WinHTTP 视为非法头部块并返回 `ERROR_INVALID_PARAMETER(87)`。
//!
//!   实测矩阵（`examples/winhttp_probe.rs`，同机同 Key）：
//!
//!   | 头部块形态                        | 长度含 NUL | 结果        |
//!   |-----------------------------------|-----------|-------------|
//!   | 无头（走 `None`，长度 0）          | –         | ✅ 401      |
//!   | 1 条头 + CRLF                     | 否        | ✅ 401      |
//!   | 1 条头 + CRLF                     | 是        | ❌ **87**   |
//!   | 4 条头 + CRLF（本模块旧的写法）     | 是        | ❌ **87**   |
//!   | 4 条头 + CRLF（现在的写法）        | 否        | ✅ 401      |
//!
//!   （401 = 真的到达了服务端，只是 Key 无效 —— 这正是预期行为。）
//!
//!   ★ 上一轮之所以误判"87 不复现"，是因为探测脚本构造的是
//!     `wide("Authorization: Bearer x")` —— **没有 CRLF 结尾**。
//!     这种"结尾 NUL 但不带 CRLF"的块 WinHTTP 恰好容忍，于是
//!     一个不带 CRLF 的样本骗过了探测。教训：复现要贴着真实调用构造，
//!     "简化过的样例"很容易把 bug 简化掉。
//!
//!   修法：头部单独构造 —— 每条 `Name: value\r\n`，**不补 NUL**，
//!   由 `header_block()` 统一产出（见其文档与单测）。

use std::time::Duration;

/// 一次 GET 的结果。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP 状态码（200 / 401 / 500 …）。
    pub status: u16,
    /// 响应体，已按 UTF-8 宽松解码（非法字节用替换字符，不丢整段）。
    pub body: String,
}

/// 请求失败的原因分类。
///
/// ★ 分开是有产品意义的：WinHTTP 的错误码里，12007/12029 是 DNS/连不上
///   （网络问题），12017/12002 是超时（服务端或链路慢），而 HTTP 层的
///   401 是"Key 不对"。三类在界面上要显示三句不同的话 —— 把用户支使去
///   检查网络，而真正的问题是他复制 Key 时少了一位，这是很糟的体验。
#[derive(Debug, Clone)]
pub enum HttpError {
    /// 连不上（DNS 解析失败、目标拒绝连接）。多半是断网或服务地址写错。
    Unreachable(String),
    /// 超时。
    Timeout(String),
    /// TLS / 证书问题。企业网络做中间人代理时常见。
    Tls(String),
    /// 其它错误。
    Other(String),
}

impl HttpError {
    /// 给用户看的一句话。
    pub fn message(&self) -> String {
        match self {
            HttpError::Unreachable(detail) => format!("无法连接（{detail}）"),
            HttpError::Timeout(detail) => format!("请求超时（{detail}）"),
            HttpError::Tls(detail) => format!("TLS 校验失败（{detail}）"),
            HttpError::Other(detail) => detail.clone(),
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// HTTP GET。`headers` 是 `("Name", "Value")` 列表。
///
/// ★ 这个函数是**阻塞**的，必须在 `spawn_blocking` 里调。
///
/// ★★ 实现：Windows 上走系统 **WinHTTP**（无子进程 —— 不会闪黑窗）。
pub fn get(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<HttpResponse, HttpError> {
    #[cfg(windows)]
    {
        imp::get(url, headers, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (url, headers, timeout);
        Err(HttpError::Other(
            "当前平台未实现 HTTP 客户端（本功能优先适配 Windows）".to_string(),
        ))
    }
}

/// 把 URL 拆成 WinHTTP 需要的三段：主机、路径、是否 HTTPS。
///
/// 只支持 http/https 与「主机[:端口]/路径[?查询]」这种最朴素的形态 ——
/// 我们只查自己的一个固定接口，不打算做通用 URL 解析器。
/// 但**必须**自己拆而不是让 WinHTTP 自己解析：`WinHttpCrackUrl` 在
/// 包含非 ASCII 域名时会给出让人意外的结果，而拆出来的三段我们
/// 可以直接校验、把错误说清楚。
#[derive(Debug)]
pub(crate) struct UrlParts {
    pub host: String,
    pub path: String,
    pub secure: bool,
    pub port: u16,
}

pub(crate) fn parse_url(url: &str) -> Result<UrlParts, HttpError> {
    // ---- 协议自动补全 ----
    //
    // ★ 用户手填地址时**八成不会写 `https://`**（"api.xxx.com/user/self"）。
    //   早期版本直接拒绝无协议地址，结果就是：用户填了、保存、然后
    //   "没反应"（实际上是保存被校验挡下，错误只出现在弹窗里一行
    //   不起眼的小红字）。
    //
    //   现在自动补全：无协议默认按 HTTPS 处理（额度接口几乎全是 HTTPS；
    //   非要 HTTP 的可以显式写 `http://`）。
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };

    let (secure, rest) = if let Some(rest) = with_scheme.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = with_scheme.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(HttpError::Other(format!(
            "接口地址必须以 http:// 或 https:// 开头：{url}"
        )));
    };

    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(HttpError::Other(format!("接口地址缺少主机名：{url}")));
    }

    // 端口：只在显式写了 `:port` 时用，默认是协议默认端口。
    // IPv6 字面量（`[::1]:443`）这里不处理 —— 这个功能不会用到，
    // 真支持反而要写一堆括号解析，不如让它在主机名校验处失败并说清楚。
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            let parsed = p
                .parse::<u16>()
                .map_err(|_| HttpError::Other(format!("端口不是合法数字：{p}")))?;
            (h.to_string(), parsed)
        }
        _ => (authority.to_string(), if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(HttpError::Other(format!("接口地址缺少主机名：{url}")));
    }

    Ok(UrlParts {
        host,
        path: path.to_string(),
        secure,
        port,
    })
}

// ===========================================================================
// Windows：WinHTTP 原生实现（无子进程）
// ===========================================================================
#[cfg(windows)]
mod imp {
    use super::{HttpError, HttpResponse, parse_url};
    use std::time::Duration;
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::PCWSTR;

    /// 值净化：防 header 注入，且把用户从网页复制的 Key 里可能带的
    /// CR/LF 替换成空格（换行会破坏 header 结构）。
    fn clean(s: &str) -> String {
        s.chars()
            .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
            .collect()
    }

    /// Rust 字符串 → NUL 结尾的宽字符串。
    ///
    /// ★ **只用于 `Param<PCWSTR>` 参数**（verb / host / path / agent 这类）。
    ///   `WinHttpSendRequest` 的头部参数不是 PCWSTR，而是一个「长度即语义」的
    ///   slice —— 用它就会多算 1 个字符（NUL）并把请求打成 87。头部走
    ///   `header_block()`。
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 把 `("Name", "Value")` 列表合成 WinHTTP 的头部块。
    ///
    /// 产出**不含结尾 NUL**：windows crate 把 `slice.len()` 原样当
    /// `dwHeadersLength` 传给 `WinHttpSendRequest`，多一个 NUL 就是
    /// `ERROR_INVALID_PARAMETER(87)`。
    ///
    /// 每条头以 CRLF 结束（WinHTTP 要求；同一块里给多条头也是这个格式）。
    /// 名称与值都过 `clean()` —— 用户从网页复制 Key 时常带上换行，
    /// 而换行会破坏头部结构（Header Injection）。
    pub(super) fn header_block(headers: &[(&str, &str)]) -> Vec<u16> {
        let mut out: Vec<u16> = Vec::new();
        for (k, v) in headers {
            out.extend(clean(k).encode_utf16());
            out.extend(": ".encode_utf16());
            out.extend(clean(v).encode_utf16());
            out.extend("\r\n".encode_utf16());
        }
        out
    }

    /// `HINTERNET` 的 RAII 包装。
    ///
    /// ★ 必须包装：WinHTTP 有 4 个句柄要关（session / connect / request），
    ///   而中间任何一步都可能提前 return —— 手写 close 迟早漏一个，
    ///   漏掉就是句柄泄漏，长期常驻的后台刷新会慢慢累积。
    struct Handle(*mut core::ffi::c_void);

    impl Handle {
        fn new(ptr: *mut core::ffi::c_void, what: &str) -> Result<Self, HttpError> {
            if ptr.is_null() {
                return Err(classify(last_error_code(), what));
            }
            Ok(Handle(ptr))
        }

        fn raw(&self) -> *mut core::ffi::c_void {
            self.0
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: 句柄非空且由 WinHttp* 创建；只关一次。
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }

    /// 取最近一次 Win32 错误码（`Handle::new` 里失败时用）。
    fn last_error_code() -> u32 {
        unsafe { windows::Win32::Foundation::GetLastError().0 }
    }

    /// WinHTTP 错误码 → 产品语义。
    ///
    /// ★ 注意 windows crate 的 `.ok()` 把失败包成 `HRESULT_FROM_WIN32(err)`，
    ///   也就是 `0x8007xxxx`。直接拿 `e.code().0` 比较会永远不匹配，
    ///   必须**取低 16 位**还原成 Win32 错误码。
    pub(super) fn classify(code: u32, context: &str) -> HttpError {
        let code = code & 0xFFFF;
        match code {
            12007 | 12029 | 12030 | 12031 => {
                HttpError::Unreachable(format!("{context}，WinHTTP 错误 {code}"))
            }
            12002 | 12017 | 12028 => {
                HttpError::Timeout(format!("{context}，WinHTTP 错误 {code}"))
            }
            12157 | 12169 | 12175 | 12179 => {
                HttpError::Tls(format!("{context}，WinHTTP 错误 {code}"))
            }
            _ => HttpError::Other(format!("{context}（WinHTTP 错误 {code}）")),
        }
    }

    /// 把 `windows_core::Error` 的 HRESULT 还原成 Win32 码后分类。
    fn classify_err(e: &windows::core::Error, context: &str) -> HttpError {
        classify(e.code().0 as u32, context)
    }

    pub(super) fn get(
        url: &str,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<HttpResponse, HttpError> {
        let parts = parse_url(url)?;
        let seconds = |d: Duration| (d.as_secs().min(i32::MAX as u64)) as i32;
        let total = seconds(timeout).max(5);

        // SAFETY: 所有句柄经 Handle 包装，作用域结束逐个关闭。
        unsafe {
            // ① 会话。AUTOMATIC_PROXY = 跟随系统/IE 代理设置（Win8.1+），
            //    这也是"用户在系统里配了代理，这里自动生效"的实现方式。
            let agent = wide("PrisTimer/0.4");
            let session = Handle::new(
                WinHttpOpen(
                    PCWSTR(agent.as_ptr()),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    0,
                ),
                "WinHttpOpen",
            )?;

            // ② 超时。connect 取总量的一半（至少 3s），避免慢网被过早掐断；
            //    receive 用总量（服务器思考时间算在这里）。
            let connect = (total / 2).max(3);
            let _ = WinHttpSetTimeouts(session.raw(), connect, connect, connect, total);

            // ③ 连接（DNS + TCP，HTTPS 的 TLS 握手在 SendRequest 时发生）。
            let host = wide(&parts.host);
            let conn = Handle::new(
                WinHttpConnect(session.raw(), PCWSTR(host.as_ptr()), parts.port, 0),
                "WinHttpConnect",
            )?;

            // ④ 请求对象。
            let verb = wide("GET");
            let path = wide(&parts.path);
            let flags = if parts.secure {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            };
            let req = Handle::new(
                WinHttpOpenRequest(
                    conn.raw(),
                    PCWSTR(verb.as_ptr()),
                    PCWSTR(path.as_ptr()),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    flags,
                ),
                "WinHttpOpenRequest",
            )?;

            // ⑤ 跟随重定向（等价于 curl -L）。额度接口常挂在 CDN 上，
            //    不跟随会拿到 301/302 的空 body，表现为"解析失败"。
            let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
            let _ = WinHttpSetOption(
                Some(req.raw() as *const core::ffi::c_void),
                WINHTTP_OPTION_REDIRECT_POLICY,
                Some(&policy.to_ne_bytes()),
            );

            // ⑥ 请求头合成一整块（WinHttpSendRequest 只接受一整块）。
            //
            // ★★ 这里**不能**用 `wide()`：那个函数会补 NUL，而 windows
            //   crate 把整个 slice 的长度直接当作 `dwHeadersLength`，
            //   多出的 NUL 会被 WinHTTP 判为非法字符 → ERROR_INVALID_PARAMETER(87)。
            //   头部块因此单独构造，见 `header_block()`。
            let block = header_block(headers);
            let block_ref = if block.is_empty() {
                None
            } else {
                Some(block.as_slice())
            };

            WinHttpSendRequest(req.raw(), block_ref, None, 0, 0, 0)
                .map_err(|e| classify_err(&e, "WinHttpSendRequest"))?;

            WinHttpReceiveResponse(req.raw(), std::ptr::null_mut())
                .map_err(|e| classify_err(&e, "WinHttpReceiveResponse"))?;

            // ⑦ 状态码。
            let mut status: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                req.raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut core::ffi::c_void),
                &mut len,
                std::ptr::null_mut(),
            )
            .map_err(|e| classify_err(&e, "WinHttpQueryHeaders"))?;

            // ⑧ 读 body。循环到 avail == 0（读尽）；单次读失败就收下已读到
            //    的部分 —— 半个 JSON 也好过一句"查询失败"，上层解析失败时
            //    至少能把原文显示给用户。
            let mut body: Vec<u8> = Vec::new();
            loop {
                let mut avail: u32 = 0;
                if WinHttpQueryDataAvailable(req.raw(), &mut avail).is_err() || avail == 0 {
                    break;
                }
                let mut buf = vec![0u8; avail as usize];
                let mut read: u32 = 0;
                if WinHttpReadData(
                    req.raw(),
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    avail,
                    &mut read,
                )
                .is_err()
                {
                    break;
                }
                body.extend_from_slice(&buf[..read as usize]);
            }

            Ok(HttpResponse {
                status: status as u16,
                body: String::from_utf8_lossy(&body).into_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url_with_default_port() {
        let parts = parse_url("https://api.opencode-go.dev/v1/quota").unwrap();
        assert_eq!(parts.host, "api.opencode-go.dev");
        assert_eq!(parts.path, "/v1/quota");
        assert!(parts.secure);
        assert_eq!(parts.port, 443);
    }

    #[test]
    fn parses_explicit_port_and_query() {
        let parts = parse_url("http://127.0.0.1:8080/api/quota?key=1").unwrap();
        assert_eq!(parts.host, "127.0.0.1");
        assert_eq!(parts.port, 8080);
        assert_eq!(parts.path, "/api/quota?key=1");
        assert!(!parts.secure);
    }

    /// 没有路径时补一个 `/` —— 直接传空路径给 WinHttpOpenRequest
    /// 会被当成非法请求目标（错误码 12005）。
    #[test]
    fn bare_host_gets_root_path() {
        let parts = parse_url("https://example.com").unwrap();
        assert_eq!(parts.path, "/");
    }

    /// 用户手填不带协议 → 自动补 https://（这是"没反应"的最大来源）。
    #[test]
    fn auto_prepends_https_when_scheme_missing() {
        let parts = parse_url("api.opencode-go.dev/v1/user/self").unwrap();
        assert_eq!(parts.host, "api.opencode-go.dev");
        assert_eq!(parts.path, "/v1/user/self");
        assert!(parts.secure);
        assert_eq!(parts.port, 443);
    }

    /// 非法协议必须被挡住并说清楚，而不是把 "ftp://x" 当成主机名
    /// 发给 WinHTTP 然后收到一个莫名其妙的错误码。
    #[test]
    fn rejects_unsupported_scheme() {
        let err = parse_url("ftp://example.com/x").unwrap_err();
        assert!(err.message().contains("http://"));
    }

    #[test]
    fn rejects_missing_host() {
        assert!(parse_url("https:///path").is_err());
    }

    /// 冒号后面不是数字时，整段当成主机名（不是端口），
    /// 这样 IPv6 之外的各种奇葩写法不会静默产生错误端口。
    #[test]
    fn non_numeric_colon_is_not_a_port() {
        let parts = parse_url("https://example.com:abc/x").unwrap();
        assert_eq!(parts.host, "example.com:abc");
        assert_eq!(parts.port, 443);
    }

    /// HRESULT 低 16 位才是 Win32 错误码 —— 分类函数必须按这个还原，
    /// 否则 12007/12029 永远不会被识别成"连不上"。
    #[cfg(windows)]
    #[test]
    fn classifies_hresult_wrapped_win32_codes() {
        // 0x80072EE5 = HRESULT_FROM_WIN32(12005) 非法 URL
        // 0x80072EFD = HRESULT_FROM_WIN32(12029) 无法连接
        assert!(matches!(imp::classify(0x8007_2EE5, "t"), HttpError::Other(_)));
        assert!(matches!(
            imp::classify(0x8007_2EFD, "t"),
            HttpError::Unreachable(_)
        ));
        assert!(matches!(
            imp::classify(0x8007_2F02, "t"), // 12034? 未列出的码
            HttpError::Other(_)
        ));
        // 裸码也要能识别（Handle::new 走的是 GetLastError 直传）
        assert!(matches!(
            imp::classify(12002, "t"),
            HttpError::Timeout(_)
        ));
    }

    /// ★★ 回归测试：头部块**绝不能**含 NUL。
    ///
    /// windows crate 把 `WinHttpSendRequest` 的头部参数声明为
    /// `Option<&[u16]>`，内部把 `slice.len()` 直接当 `dwHeadersLength`。
    /// 一旦沿用会给 Win32 字符串补 NUL 的 `wide()`，长度就多 1，
    /// WinHTTP 判定头部块含非法字符并返回 `ERROR_INVALID_PARAMETER(87)` ——
    /// 症状是"额度查询永远网络错误"，而**不带头的请求却完全正常**
    /// （当年正是这个不对称让人误判成"本机 WinHTTP 坏了"）。
    #[cfg(windows)]
    #[test]
    fn header_block_must_not_contain_nul() {
        let block = imp::header_block(&[("Authorization", "Bearer oc_sk_abc")]);
        assert!(
            !block.contains(&0),
            "头部块含 NUL —— WinHttpSendRequest 会返回错误 87"
        );
        // 长度必须等于实际字符数（含 CRLF），一个不多一个不少
        assert_eq!(block.len(), "Authorization: Bearer oc_sk_abc\r\n".encode_utf16().count());
    }

    /// 无头时返回空块 —— 调用方据此传 `None`（WinHTTP 的
    /// "WINHTTP_NO_ADDITIONAL_HEADERS" 语义），而不是一个空 buffer。
    #[cfg(windows)]
    #[test]
    fn header_block_is_empty_without_headers() {
        assert!(imp::header_block(&[]).is_empty());
    }

    /// 每条头必须以 CRLF 结束，否则 WinHTTP 找不到头部边界。
    #[cfg(windows)]
    #[test]
    fn header_block_terminates_each_line_with_crlf() {
        let block = imp::header_block(&[("A", "1"), ("B", "2")]);
        assert_eq!(&block[block.len() - 2..], &[13, 10]);
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "A: 1\r\nB: 2\r\n");
    }

    /// 值里混进换行（网页复制 Key 的常见附带物）必须被净化，
    /// 否则会注入出伪造的头部。
    #[cfg(windows)]
    #[test]
    fn header_block_sanitizes_crlf_injection() {
        let block = imp::header_block(&[("Authorization", "Bearer x\r\nX-Evil: 1")]);
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "Authorization: Bearer x  X-Evil: 1\r\n");
        // 整块只应有一个 CRLF —— 就是结尾那一个
        assert_eq!(text.matches("\r\n").count(), 1);
    }
}
