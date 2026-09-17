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
//!     · 默认就支持 gzip/deflate 解压（`WINHTTP_OPTION_DECOMPRESSION` 默认开）；
//!     · 零新增依赖（`windows` crate 已在依赖树里）；
//!     · 行为与系统一致 —— 用户在 IE/Edge 里配了代理，这里自动跟随。
//!
//! ★ 一个必须避开的 windows crate 坑：
//!
//!   `WinHttpOpen` 的签名里 agent 是 `P0: Param<PCWSTR>`，代理两个参数是
//!   `P2/P3: Param<PCWSTR>`。传 `None` 时 windows crate 会往
//!   `PCWSTR::param()` 里塞 `self.0.as_ptr()` —— 而 `None` 的 `PCWSTR`
//!   内部是空指针，**空指针调 `as_ptr()` 在 debug 构建下会直接 panic**
//!   （`Option::unwrap()` on a `None` value）。这不是我们的 bug，是
//!   windows crate 生成代码里的一个已知缺陷。
//!
//!   绕法很简单：传**空宽字符串** `"\0"` 而不是 `None`。语义完全等价
//!   （空串的 `PCWSTR` 是合法指针，WinHTTP 认它），但不再触发那条
//!   panic 路径。
//!
//! 非 Windows 平台这里返回一个明确的错误 —— 上层会把它渲染成
//! 「网络不可用」，不假装查到了。

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
    /// 其它 Win32 错误。
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
/// ★★ 实现选择：**Windows 上走系统自带 `curl.exe` 子进程**，不用 WinHTTP。
///
///   为什么（真实踩坑记录，2026-09）：
///   这台机器上 WinHTTP 对任何 HTTPS 请求的 `WinHttpSendRequest` 都返回
///   ERROR_INVALID_PARAMETER(87) —— 明文/裸 WINHTTP 调用复现、代理无关、
///   header 无关。同机 `curl.exe https://example.com/` 却一直 200。
///   排查指向**安全软件（火绒之类）钩挂了 WinHTTP.dll** 的发送路径。
///
///   所以改用系统自带的 curl（Windows 10 1803+ 必带，走 schannel 的
///   独立网络栈，不经过被钩的 WinHTTP）。代价是 spawn 一个子进程，
///   对本功能（几秒一次、额度查询）可忽略。
///
///   为规避「子进程管道输出」的沙箱/环境限制，**结果全部走临时文件**
///   （`-o` 写响应体、`-D` 写响应头）而不是捕获子进程 stdout ——
///   进程只等待退出，不产生任何管道。
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
            let parsed = p.parse::<u16>().map_err(|_| {
                HttpError::Other(format!("端口不是合法数字：{p}"))
            })?;
            (h.to_string(), parsed)
        }
        _ => (
            authority.to_string(),
            if secure { 443 } else { 80 },
        ),
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
// Windows：系统 curl 子进程实现（绕过被安全软件钩住的 WinHTTP）
// ===========================================================================
#[cfg(windows)]
mod imp {
    use super::{HttpError, HttpResponse, parse_url};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// 值净化：防 header 注入，且把用户从网页复制的 Key 里可能带的
    /// CR/LF 替换成空格（换行会破坏 header 结构）。
    fn clean(s: &str) -> String {
        s.chars()
            .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
            .collect()
    }

    pub(super) fn get(
        url: &str,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<HttpResponse, HttpError> {
        let parts = parse_url(url)?;

        // 结果全部走临时文件（-o 响应体、-D 响应头），不捕获子进程
        // stdout —— 避免任何管道/输出捕获的坑，也便于超时清理。
        let stamp = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir();
        let body_path = dir.join(format!("pt_body_{stamp}.txt"));
        let hdr_path = dir.join(format!("pt_hdr_{stamp}.txt"));

        let mut cmd = Command::new("curl.exe");
        cmd.arg("-sS") // 静默 + 出错时把错误写进 stderr
            .arg("-L") // 跟随重定向（额度接口常有 CDN 跳转）
            .arg("--connect-timeout")
            .arg(timeout.as_secs().saturating_div(2).max(3).to_string())
            .arg("-m")
            .arg(timeout.as_secs().to_string())
            .arg("-o")
            .arg(&body_path)
            .arg("-D")
            .arg(&hdr_path);
        for (k, v) in headers {
            cmd.arg("-H").arg(format!("{}: {}", clean(k), clean(v)));
        }
        cmd.arg(format!(
            "{}://{}:{}{}",
            if parts.secure { "https" } else { "http" },
            parts.host,
            parts.port,
            parts.path
        ));

        // 不捕获输出：stdin/stdout/stderr 全部丢弃，只等退出码。
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = cmd.status().map_err(|e| {
            HttpError::Other(format!(
                "无法启动系统 curl.exe（Windows 10 1803+ 自带）：{e}"
            ))
        })?;

        // 先读产物，再清理临时文件（读失败也继续——结果可能为空）。
        let hdr = std::fs::read(&hdr_path).unwrap_or_default();
        let body = std::fs::read(&body_path).unwrap_or_default();
        let _ = std::fs::remove_file(&body_path);
        let _ = std::fs::remove_file(&hdr_path);

        // 退出码分类：curl 的 6/7 连不上、28 超时、35/51/58/60 TLS。
        if !status.success() {
            let code = status.code().unwrap_or(-1);
            return Err(match code {
                6 | 7 => HttpError::Unreachable(format!("curl 无法连接（错误 {code}）")),
                28 => HttpError::Timeout("curl 请求超时".to_string()),
                35 | 51 | 58 | 60 | 90 => {
                    HttpError::Tls(format!("curl TLS 校验失败（错误 {code}）"))
                }
                _ => HttpError::Other(format!("curl 请求失败（退出码 {code}）")),
            });
        }

        Ok(HttpResponse {
            status: status_code_from_header(&hdr).unwrap_or(0),
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    /// 从 curl -D 导出的响应头里取 HTTP 状态码。
    ///
    /// 响应头文件第一行形如 `HTTP/1.1 200 OK` 或 `HTTP/2 200`。
    fn status_code_from_header(hdr: &[u8]) -> Option<u16> {
        let text = String::from_utf8_lossy(hdr);
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("HTTP/") else {
                continue;
            };
            let mut it = rest.split_whitespace();
            it.next(); // 协议版本
            if let Some(code) = it.next() {
                return code.strip_suffix('\r').unwrap_or(code).parse().ok();
            }
        }
        None
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
}
