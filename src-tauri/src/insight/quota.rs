//! opencode-go 订阅额度查询。
//!
//! ★ 关于「字段名不确定」这件事，必须先说清楚：
//!
//!   opencode-go 的额度接口不是公开稳定契约，不同版本的返回体长这样：
//!
//!   ```json
//!   { "remaining": 12.5, "total": 50, "unit": "USD" }              // 扁平
//!   { "data": { "quota": { "remaining": 12.5, "limit": 50 } } }    // 嵌套
//!   { "balance": 12.5, "currency": "USD", "reset_at": "..." }      // 换名
//!   { "usage": { "remaining_percent": 62 } }                       // 百分比制
//!   ```
//!
//!   所以解析器**不能**写死一个 `#[derive(Deserialize)]` 的结构体 ——
//!   接口一改字段名，前端就会看到「接口返回异常」，而其实数据好好地在
//!   响应里躺着，只是我们读错了键。这里的做法是在 JSON 树里**按候选
//!   键名 + 递归下钻**去找，找到哪个用哪个。宁可多写 60 行查找逻辑，
//!   也不要一个字段名变更就把功能打瘫。
//!
//!   同时把**原始响应体**一并带回给前端（折叠在详情里）：万一所有候选
//!   键都落空，用户能直接把原始 JSON 贴出来，我们照着加一个键名就行，
//!   不用去猜。

use std::time::Duration;

use serde_json::Value;

use super::http::{self, HttpError};
use super::QuotaStatus;

/// opencode Go 订阅的**官方用量端点**。
///
/// ★ 这是本功能真正要打的地址（2026-09 实测：HTTP 200，返回三窗口用量）。
///   它不在 opencode 的公开文档里，由社区从 Go 仪表页逆出，多个第三方
///   工具在用 —— 因此**内置成默认值**：用户只需要填 API Key，
///   不该被迫去猜一个路径。
///
///   实测响应：
///   ```json
///   {"usage":{
///     "rolling":{"status":"ok","percent":0, "resetsAt":"2026-09-17T11:36:53Z"},
///     "weekly": {"status":"ok","percent":26,"resetsAt":"2026-09-21T00:00:00Z"},
///     "monthly":{"status":"ok","percent":13,"resetsAt":"2026-10-14T00:51:38Z"}}}
///   ```
///   `percent` 是**已用**百分比。
pub const DEFAULT_ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";

/// opencode 网关的根路径。用户常把它当成"额度接口"填进来（少一段 `/usage`）
/// —— 实测这就是 404 的直接原因，见 `normalized_endpoint`。
const OPENCODE_GATEWAY_ROOT: &str = "https://opencode.ai/zen/go";

/// 额度接口配置。
///
/// 默认地址留空而不是写一个猜的地址：写错了会让用户以为"网络有问题"，
/// 而真相是我们把请求发去了一个不存在的地方。空地址 → 用内置的
/// `DEFAULT_ENDPOINT`（opencode Go 官方用量端点）；连 Key 都没有才是
/// `NotConfigured` → 界面提示"未配置 API Key"，用户一看就知道要去做配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaConfig {
    /// API Key。空表示未配置。
    #[serde(default)]
    pub api_key: String,
    /// 额度接口完整 URL。空表示用内置默认端点。
    #[serde(default)]
    pub endpoint: String,
}

/// 额度配置在设置表里的键名（与 `pomodoro_config` / `current_tag` 同一张表）。
pub const SETTINGS_KEY: &str = "quotaconfig";

impl QuotaConfig {
    /// 有 Key 就算配置完整 —— 地址可以留空走内置默认。
    pub fn is_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// **实际要请求的地址**（把用户的输入规范化）。
    ///
    /// ★ 为什么需要这一步（实测缺陷）：用户填的是
    ///   `https://opencode.ai/zen/go/v1`（网关根路径，看着像额度接口），
    ///   服务端直接 404；真正的用量端点是根路径 + `/usage`。
    ///   界面上让用户自己猜出这一段的代价太高（实测就卡在这里），
    ///   所以对 opencode 网关的地址**自动补全 `/usage`**；
    ///   其它地址（one-api / OpenAI billing 等自建网关）原样保留，
    ///   用户仍可指向任何兼容端点。
    pub fn normalized_endpoint(&self) -> String {
        let trimmed = self.endpoint.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return DEFAULT_ENDPOINT.to_string();
        }
        if trimmed.starts_with(OPENCODE_GATEWAY_ROOT) && !trimmed.contains("/usage") {
            return format!("{trimmed}/usage");
        }
        trimmed.to_string()
    }
}

/// 单次查询的超时。
///
/// 8 秒是个经验值：普通 HTTPS 往返 200–600ms，跨洋链路 1–2 秒，
/// 8 秒能容下"网络很慢但没断"，又不会让用户等太久。
/// 前端那边有独立的一分钟刷新周期，一次慢查询不会堆积 ——
/// 而且后台任务是 `loop + sleep` 串行结构，上一次没回来就不会发下一次。
const TIMEOUT: Duration = Duration::from_secs(8);

/// 候选键名表。
///
/// 分成三组是因为**数值语义不同**，不能混在一起找：
///   · 剩余额度 —— 我们要显示的主数字；
///   · 总量 / 上限 —— 分子分母里的分母；
///   · 已用 —— 有些接口只给已用，剩余得自己减（见 `derive_remaining`）。
///
/// 每个数组内按"最可能"到"最不可能"排：先命中的优先，
/// 这样 `remaining` 和 `remaining_percent` 同时存在时会取前者
/// （百分比是退化信息，能拿到绝对值就别用百分比）。
const KEYS_REMAINING: &[&str] = &[
    "remaining",
    "remaining_quota",
    "remainingQuota",
    "remain",
    "balance",
    "credits_remaining",
    "creditsRemaining",
    "remaining_credits",
    "left",
    "quota_remaining",
    "available",
    "remaining_usd",
];
// ★ `quota` 刻意不放这里：one-api / new-api 的 `quota` 是"剩余额度"语义，
//   放总量列表会与专用分支打架，见 `try_oneapi`。
const KEYS_TOTAL: &[&str] = &[
    "total",
    "limit",
    "total_quota",
    "totalQuota",
    "quota_limit",
    "allowance",
    "total_credits",
    "max",
];
const KEYS_USED: &[&str] = &[
    "used",
    "usage",
    "consumed",
    "used_quota",
    "usedQuota",
    "spent",
    "credits_used",
];

/// OpenAI billing（以及大量中转站模仿它）的专用键。
///
/// `GET /v1/dashboard/billing/usage` 通常返回：
/// ```json
/// {
///   "object": "usage",
///   "total_usage": 1234,        // 美分
///   "total_granted": 100000,    // 美分（可选）
///   "hard_limit_usd": 100,      // 美元（可选）
///   "soft_limit_usd": 100
/// }
/// ```
/// 一些中转把 `total_granted` 省了，只给 `total_usage` + `hard_limit_usd`。
const OPENAI_USAGE_KEYS: &[&str] = &["total_usage", "totalUsage", "used_amount"];
const OPENAI_GRANTED_KEYS: &[&str] = &["total_granted", "totalGranted", "granted"];
const OPENAI_LIMIT_KEYS: &[&str] = &["hard_limit_usd", "soft_limit_usd", "limit_usd"];

/// 单位候选键。
const KEYS_UNIT: &[&str] = &["unit", "currency", "unit_name", "unitName", "denomination"];
/// 重置时间候选键。
const KEYS_RESET: &[&str] = &[
    "resets_at",
    "resetsAt",
    "reset_at",
    "resetAt",
    "renew_at",
    "renewAt",
    "expires_at",
    "expiresAt",
    "next_reset",
    "period_end",
];

/// 递归下钻去寻找候选键所在的"数据层"。
///
/// ★ 为什么需要 `ok` 这个"值形态"谓词，而不是见键就认：
///
///   候选键名和常见的**嵌套层名**会撞车 —— 最典型的就是 `quota`：
///   它既可能是总量字段（`{"quota": 50}`），更常见的是外包一层数据
///   （`{"data":{"quota":{"remaining":...}}}`）。如果「对象里出现
///   `quota` 键就命中」，第一个命中的就是那个**包装对象**，而包装对象
///   里 `quota` 的值是对象不是数字 —— 提取必然失败。
///
///   所以匹配条件必须是「有候选键 **且** 键值形态符合我们要提取的东西」：
///   找数字时要求值能当数字读，找字符串时要求值是串或数字（重置时间）。
///   包装对象因为值形态不满足，会继续被 BFS 往下展开，最终命中真正的叶子。
///
/// 深度限死 6 层：正常信封 2–3 层，给一倍余量即可。不限深度的递归在
/// 遇到极深嵌套（畸形/恶意响应）时会栈溢出 —— 那是一次远程 DoS。
fn find_object_with_keys<'a>(
    root: &'a Value,
    keys: &[&str],
    ok: fn(&Value) -> bool,
) -> Option<&'a Value> {
    const MAX_DEPTH: usize = 6;

    let mut queue: Vec<(&Value, usize)> = vec![(root, 0)];
    let mut cursor = 0;
    while cursor < queue.len() {
        let (node, depth) = queue[cursor];
        cursor += 1;
        if depth > MAX_DEPTH {
            continue;
        }
        match node {
            Value::Object(map) => {
                // 命中判定：有任意候选键，且该键的值形态符合要求。
                if keys.iter().any(|k| map.get(*k).is_some_and(ok)) {
                    return Some(node);
                }
                // 没命中就把子节点排进队列继续找。
                for child in map.values() {
                    if child.is_object() || child.is_array() {
                        queue.push((child, depth + 1));
                    }
                }
            }
            Value::Array(items) => {
                for child in items {
                    if child.is_object() || child.is_array() {
                        queue.push((child, depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// 数值查找用的谓词：JSON 数字，或能解析成数字的字符串
/// （不少后端把金额序列化成串）。
///
/// 刻意**不含 null / bool**：`null` 在不少接口里是"没数据"的意思，
/// 不能把它当成 0。
fn is_numeric_like(value: &Value) -> bool {
    match value {
        Value::Number(_) => true,
        Value::String(s) => s.trim().parse::<f64>().is_ok(),
        _ => false,
    }
}

/// 字符串查找用的谓词：字符串，或数字（重置时间可能是 Unix 时间戳）。
fn is_string_like(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_))
}

/// 在对象里按候选键取数值。
///
/// 顺手处理三种"看起来像数字但 JSON 里不是数字"的情况：
///   · 字符串数字（`"12.5"`）—— 很多 Go/Python 后端会把金额序列化成串；
///   · 布尔与 null —— 明确不算数；
///   · 负数 —— 额度不会是负的，负数说明字段语义搞错了，宁可当没找到。
fn pick_number(obj: &Value, keys: &[&str]) -> Option<f64> {
    let map = obj.as_object()?;
    for key in keys {
        let Some(value) = map.get(*key) else {
            continue;
        };
        let number = match value {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        if let Some(n) = number {
            if n.is_finite() && n >= 0.0 {
                return Some(n);
            }
        }
    }
    None
}

/// 在对象里按候选键取字符串（重置时间 / 单位）。
fn pick_string(obj: &Value, keys: &[&str]) -> Option<String> {
    let map = obj.as_object()?;
    for key in keys {
        match map.get(*key) {
            Some(Value::String(s)) if !s.trim().is_empty() => return Some(s.trim().to_string()),
            // 有些接口给的是 Unix 秒/毫秒数字，也接受，转成串交给前端显示。
            Some(Value::Number(n)) => return Some(n.to_string()),
            _ => continue,
        }
    }
    None
}

/// 从响应体解析额度。
///
/// 返回 `Err(String)` 时内容是**给用户看的失败原因**，
/// 由调用方包成 `ApiError`。
pub(crate) fn parse_quota(body: &str) -> Result<QuotaStatus, String> {
    let root: Value = serde_json::from_str(body).map_err(|err| {
        // 把响应开头截一小段贴进错误里：如果是「被重定向到登录门户」，
        // 错误信息里会直接出现 `<!DOCTYPE html>`，一眼就能看出
        // 不是接口坏了而是地址/鉴权配置错了。截 120 字符是为了不让
        // 一个整页 HTML 把日志和界面刷屏。
        let head: String = body.chars().take(120).collect();
        format!("响应不是合法 JSON：{err}；响应开头：{head}")
    })?;

    // 先看有没有明确的错误字段 —— 有些接口 HTTP 返回 200，
    // 但在 body 里用 `{"error": "..."}` 表达失败。
    if let Some(message) = extract_error(&root) {
        return Err(message);
    }

    // ---- 依次尝试四个分支，前三个是"网关注庭"专用格式 ----
    //
    //   · opencode Go 系（本功能的主场景）：`usage.{rolling,weekly,monthly}`
    //   · OpenAI billing 系（大量中转站照抄）：total_usage / total_granted...
    //   · one-api / new-api 系（国内中转站最常见的后端）：
    //     `{"success":true,"data":{"quota":...,"used_quota":...}}`
    //   · 通用键（remaining / balance / …）
    // 每步把 `(remaining, total, used, unit)` 填上，谁先成功用谁。
    // opencode 分支放最前：它的 `usage` 键与通用键的 `usage`（"已用"）
    // 同名但语义完全不同，必须先用更具体的形态把它认出来。

    // ① opencode Go 多窗口用量。
    if let Some(parsed) = try_opencode_go(&root) {
        return Ok(parsed);
    }

    // ② OpenAI billing 系（美分体系）。
    if let Some(parsed) = try_openai_billing(&root) {
        return Ok(parsed);
    }

    // ③ one-api / new-api 系。
    if let Some(parsed) = try_oneapi(&root) {
        return Ok(parsed);
    }

    // ④ 通用候选键。
    let remaining = find_object_with_keys(&root, KEYS_REMAINING, is_numeric_like)
        .and_then(|obj| pick_number(obj, KEYS_REMAINING));
    let total = find_object_with_keys(&root, KEYS_TOTAL, is_numeric_like)
        .and_then(|obj| pick_number(obj, KEYS_TOTAL));
    let used = find_object_with_keys(&root, KEYS_USED, is_numeric_like)
        .and_then(|obj| pick_number(obj, KEYS_USED));

    let remaining = match remaining {
        Some(v) => v,
        None => derive_remaining(total, used).ok_or_else(|| {
            format!(
                "响应里找不到额度字段（已尝试 OpenAI billing / one-api / remaining 等 {} 种命名）",
                KEYS_REMAINING.len()
            )
        })?,
    };

    // 单位：找不到时按"额度点"处理，而不是瞎猜 USD。
    // 猜错货币单位比不显示单位更糟 —— 用户会拿它去对账。
    let unit = find_object_with_keys(&root, KEYS_UNIT, is_string_like)
        .and_then(|obj| pick_string(obj, KEYS_UNIT))
        .unwrap_or_else(|| "credits".to_string());
    let resets_at = find_object_with_keys(&root, KEYS_RESET, is_string_like)
        .and_then(|obj| pick_string(obj, KEYS_RESET));

    // 原始响应：格式化一下再带走，前端折叠展示时可读性更好。
    // 格式化失败（理论上不会，能 parse 就能 to_string）就退回原文。
    let raw = serde_json::to_string_pretty(&root).unwrap_or_else(|_| body.to_string());

    Ok(QuotaStatus::Ok {
        remaining: round2(remaining),
        total: total.map(round2),
        used: used.map(round2),
        unit,
        resets_at,
        raw,
        windows: Vec::new(),
    })
}

/// opencode Go 专用：`usage.rolling / weekly / monthly` 三窗口百分比。
///
/// 实测响应形态见 `DEFAULT_ENDPOINT` 的注释。要点：
///   · `percent` 是**已用**百分比，出口一律换算成**剩余**；
///   · 有的版本会直接给 `percent_remaining`（直给剩余，优先采信）；
///   · 三个窗口各自有重置时间，全部带上 —— "还有多久恢复"是用户
///     在这一栏最常问的问题。
///
/// 返回 `None` 表示响应长得不像这个接口（交给后面几个分支继续试）。
fn try_opencode_go(root: &Value) -> Option<QuotaStatus> {
    let usage = root.get("usage")?.as_object()?;

    // 顺序即展示顺序：先短周期后长周期。
    const WINDOWS: &[(&str, &str)] = &[("rolling", "滚动"), ("weekly", "本周"), ("monthly", "本月")];

    let mut windows: Vec<super::QuotaWindow> = Vec::new();
    for (key, label) in WINDOWS {
        let Some(window) = usage.get(*key).and_then(Value::as_object) else {
            continue;
        };
        let node = Value::Object(window.clone());

        // 优先"直给剩余"的字段；否则用 100 − 已用，并夹到 0..=100
        //（接口在超额时可能给 percent > 100）。
        let remaining = pick_number(
            &node,
            &["percent_remaining", "percentRemaining", "remaining_percent", "remainingPercent"],
        )
        .or_else(|| {
            pick_number(
                &node,
                &["percent", "used_percent", "usedPercent", "usage_percent", "usagePercent"],
            )
            .map(|used| (100.0 - used).clamp(0.0, 100.0))
        });
        let Some(remaining) = remaining else {
            continue;
        };

        windows.push(super::QuotaWindow {
            label: (*label).to_string(),
            remaining_percent: round2(remaining),
            resets_at: pick_string(&node, KEYS_RESET),
            status: window
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    if windows.is_empty() {
        return None;
    }

    // 主数字 = 最紧张的窗口：托盘/状态栏一行空间有限，先报最该关心的那个。
    let tightest = windows
        .iter()
        .min_by(|a, b| a.remaining_percent.total_cmp(&b.remaining_percent))
        .cloned()?;

    Some(QuotaStatus::Ok {
        remaining: tightest.remaining_percent,
        total: Some(100.0),
        used: Some(round2(100.0 - tightest.remaining_percent)),
        unit: "%".to_string(),
        resets_at: tightest.resets_at.clone(),
        raw: serde_json::to_string_pretty(root).unwrap_or_default(),
        windows,
    })
}

/// OpenAI billing 系：`total_usage`（美分）+ `total_granted`（美分）
/// 或 `hard_limit_usd`（美元）。
///
/// 返回 `Some` 表示识别成功并完成换算。换算规则：
///   · 有 `total_granted` → 剩余美分 = total_granted − total_usage；
///   · 只有 `hard_limit_usd` → 剩余美分 = hard_limit_usd×100 − total_usage；
///   · 单位统一输出 USD。
fn try_openai_billing(root: &Value) -> Option<QuotaStatus> {
    let usage_cents = find_object_with_keys(root, OPENAI_USAGE_KEYS, is_numeric_like)
        .and_then(|obj| pick_number(obj, OPENAI_USAGE_KEYS))?;

    let granted_cents =
        find_object_with_keys(root, OPENAI_GRANTED_KEYS, is_numeric_like).and_then(|obj| {
            pick_number(obj, OPENAI_GRANTED_KEYS)
        });
    let limit_usd = find_object_with_keys(root, OPENAI_LIMIT_KEYS, is_numeric_like).and_then(|obj| {
        pick_number(obj, OPENAI_LIMIT_KEYS)
    });

    let total_cents = granted_cents.or_else(|| limit_usd.map(|usd| usd * 100.0))?;
    let remaining_cents = (total_cents - usage_cents).max(0.0);

    Some(QuotaStatus::Ok {
        remaining: round2(remaining_cents / 100.0),
        total: Some(round2(total_cents / 100.0)),
        used: Some(round2(usage_cents / 100.0)),
        unit: "USD".to_string(),
        resets_at: None,
        raw: serde_json::to_string_pretty(root).unwrap_or_default(),
        windows: Vec::new(),
    })
}

/// one-api / new-api 系：`data.quota`（剩余）+ `data.used_quota`（已用）。
///
/// 这是国内中转站最常见的后端（常见地址 `/api/user/self`，格式
/// `{"success":true,"data":{"quota":...,"used_quota":...}}`）。
///
/// ★ 语义说明：新版本 one-api 的 `quota` 是**剩余额度**，`used_quota`
///   是**已用**。两者单位是同一种"额度分"（服务端自定义，多数按
///   美元×500000 或按元）。我们无法可靠换算成美元，所以：
///   · 剩余 = quota − used_quota（语义为"剩余额度"时直接取 quota）；
///   · 单位标为 "credits"，不假装是美元。
fn try_oneapi(root: &Value) -> Option<QuotaStatus> {
    // 必须同时有 quota 与 used_quota（或 request_count 佐证是 one-api）。
    let quota = find_object_with_keys(root, &["quota"], is_numeric_like)
        .and_then(|obj| pick_number(obj, &["quota"]))?;
    let used_quota = find_object_with_keys(root, &["used_quota", "usedQuota"], is_numeric_like)
        .and_then(|obj| pick_number(obj, &["used_quota", "usedQuota"]));

    // one-api 的 `quota` 在部分版本是剩余额度（且无 used_quota）。
    let remaining = match used_quota {
        Some(used) => (quota - used).max(0.0),
        None => quota,
    };

    let raw = serde_json::to_string_pretty(root).unwrap_or_default();
    Some(QuotaStatus::Ok {
        remaining: round2(remaining),
        total: None,
        used: used_quota.map(round2),
        unit: "credits".to_string(),
        resets_at: None,
        raw,
        windows: Vec::new(),
    })
}

/// 「总量 − 已用」。两个都要有才推得出来。
fn derive_remaining(total: Option<f64>, used: Option<f64>) -> Option<f64> {
    match (total, used) {
        (Some(t), Some(u)) => Some((t - u).max(0.0)),
        _ => None,
    }
}

/// 从响应里抽错误信息。返回 Some 表示"这是一次失败"。
fn extract_error(root: &Value) -> Option<String> {
    let map = root.as_object()?;
    // 有的接口用 `"ok": false` 表达失败，错误在别的字段里。
    for key in ["error", "error_message", "errorMessage", "message", "detail"] {
        match map.get(key) {
            // 错误可能是对象（`{"error":{"message":"..."}}`），下一层再找一次。
            Some(Value::Object(inner)) => {
                if let Some(Value::String(m)) = inner.get("message").or_else(|| inner.get("msg")) {
                    if !m.trim().is_empty() {
                        return Some(m.trim().to_string());
                    }
                }
            }
            Some(Value::String(m)) if !m.trim().is_empty() => {
                // ★ `message` 字段要小心：成功的响应里也常带一个
                //   `"message": "ok"` 之类的状态话术。只在**同时**存在
                //   错误标志或完全没有额度字段时才把它当错误。
                let is_neutral = matches!(m.trim().to_ascii_lowercase().as_str(), "ok" | "success" | "succeeded");
                if !is_neutral && key != "message" {
                    return Some(m.trim().to_string());
                }
                if !is_neutral && !has_any_quota_field(root) {
                    return Some(m.trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// 响应里是否含有任何一个我们认识的额度字段。
///
/// 用来区分「这是一条错误消息」和「这是成功的响应附带了一句状态话术」。
fn has_any_quota_field(root: &Value) -> bool {
    let all: Vec<&str> = KEYS_REMAINING
        .iter()
        .chain(KEYS_TOTAL.iter())
        .chain(KEYS_USED.iter())
        .copied()
        .collect();
    find_object_with_keys(root, &all, is_numeric_like).is_some()
}

/// 保留两位小数。
///
/// 额度是钱，界面上显示 `12.499999999` 会让人怀疑这个应用不可靠。
/// 不做货币取整（不用 `round()` 到整数）：有些单位是 credits，
/// 小数是正常且必要的。
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// 执行一次额度查询（**阻塞**，必须在 `spawn_blocking` 里调）。
pub fn fetch_blocking(config: &QuotaConfig) -> QuotaStatus {
    if !config.is_ready() {
        return QuotaStatus::NotConfigured;
    }

    let key = config.api_key.trim();
    // 走规范化后的地址：空 → 内置 opencode Go 端点；用户的网关根路径
    // 缺 `/usage` 时自动补全（实测 404 的直接原因，见 normalized_endpoint）。
    let endpoint = config.normalized_endpoint();
    // ★ 两种鉴权头都发。
    //
    //   opencode-go 用哪种不是公开信息，而**多带一个头不会有害**：
    //   服务端只认它需要的那一个，多余的会被忽略。反过来，
    //   只带错的那一个就是 401，而用户拿不到任何可行动的线索。
    //   这是一处刻意的"宁可冗余也要能跑通"。
    let auth_bearer = format!("Bearer {key}");
    let headers: Vec<(&str, &str)> = vec![
        ("Authorization", auth_bearer.as_str()),
        ("X-API-Key", key),
        ("Accept", "application/json"),
        // 明确拒绝 HTML：万一地址写错被重定向到登录页，
        // 带上这个头有机会拿到 406 而不是一个 200 的 HTML 页面，
        // 错误信息能干净得多。
        ("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8"),
    ];

    let result = match http::get(&endpoint, &headers, TIMEOUT) {
        Ok(response) => classify_response(response.status, &response.body),
        Err(err) => match err {
            // ★ 鉴权类错误在 HTTP 层拿不到（那是 401/403，走 Ok 分支），
            //   能到这里的都是传输层问题，统一归到 Network。
            HttpError::Unreachable(detail) | HttpError::Timeout(detail) => QuotaStatus::Network {
                message: detail,
            },
            HttpError::Tls(detail) => QuotaStatus::Network { message: detail },
            HttpError::Other(detail) => QuotaStatus::Network { message: detail },
        },
    };

    // ★ 每次额度查询都落一条日志 —— 用户反馈"没反应"时，
    //   日志里能看到请求到底到了哪一步、得到了什么。
    crate::log::log(format!(
        "额度查询：地址={endpoint} => {:?}",
        quota_status_summary(&result)
    ));
    result
}

/// 额度的简短摘要（给日志用，不暴露 Key）。
fn quota_status_summary(status: &QuotaStatus) -> String {
    match status {
        QuotaStatus::Ok {
            remaining, unit, ..
        } => format!("OK 剩余 {remaining} {unit}"),
        QuotaStatus::NotConfigured => "未配置".to_string(),
        QuotaStatus::Unauthorized { message } => format!("鉴权失败：{message}"),
        QuotaStatus::Network { message } => format!("网络错误：{message}"),
        QuotaStatus::ApiError { message } => format!("接口错误：{message}"),
    }
}

/// 把一次 HTTP 响应翻译成额度状态。
///
/// 独立成函数是为了**可以单测**：不需要真发网络请求就能验证
/// 「401 → Unauthorized」「500 → ApiError」「200 + 坏 JSON → ApiError」。
fn classify_response(status: u16, body: &str) -> QuotaStatus {
    match status {
        200..=299 => match parse_quota(body) {
            Ok(ok) => ok,
            Err(message) => QuotaStatus::ApiError { message },
        },
        // 401 未鉴权 / 403 无权限 / 407 代理要鉴权 —— 都指向"凭据不对"。
        401 | 403 => QuotaStatus::Unauthorized {
            message: format!("服务端返回 {status}"),
        },
        // ★ 404 / 405：地址或路径不对 —— 用户实测填 opencode.ai/zen/go/v1
        //   拿到的就是 404，如果只说"接口错误"用户完全不知道怎么改。
        //   这里把方向说清楚：查网关面板里的真实路径
        //   （OpenAI 风格 /v1/dashboard/billing/usage，one-api /api/user/self）。
        404 => QuotaStatus::ApiError {
            message: "接口不存在（404）：请检查地址路径，常见有两种格式 — OpenAI 风格 /v1/dashboard/billing/usage；one-api 风格 /api/user/self".to_string(),
        },
        405 => QuotaStatus::ApiError {
            message: "接口不支持此请求方式（405）：需 GET".to_string(),
        },
        429 => QuotaStatus::ApiError {
            message: "请求过于频繁（429），请稍后再试".to_string(),
        },
        500..=599 => QuotaStatus::ApiError {
            message: format!("服务端错误 {status}"),
        },
        _ => QuotaStatus::ApiError {
            message: format!("未预期的状态码 {status}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_response() {
        let status = parse_quota(r#"{"remaining": 12.5, "total": 50, "unit": "USD"}"#).unwrap();
        match status {
            QuotaStatus::Ok {
                remaining,
                total,
                unit,
                ..
            } => {
                assert_eq!(remaining, 12.5);
                assert_eq!(total, Some(50.0));
                assert_eq!(unit, "USD");
            }
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 信封形态（`data` 包一层）必须也能找到 —— 这正是写死字段名的
    /// 结构体反序列化会失败、而候选键搜索能救回来的场景。
    #[test]
    fn drills_into_nested_envelope() {
        let status = parse_quota(
            r#"{"code":0,"data":{"quota":{"remaining":7,"limit":20,"currency":"CNY"}}}"#,
        )
        .unwrap();
        match status {
            QuotaStatus::Ok {
                remaining,
                total,
                unit,
                ..
            } => {
                assert_eq!(remaining, 7.0);
                assert_eq!(total, Some(20.0));
                assert_eq!(unit, "CNY");
            }
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 只给"总量 + 已用"时，剩余要能自己算出来。
    #[test]
    fn derives_remaining_from_total_and_used() {
        let status = parse_quota(r#"{"total": 100, "used": 35.5}"#).unwrap();
        match status {
            QuotaStatus::Ok { remaining, .. } => assert_eq!(remaining, 64.5),
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 字符串数字（不少后端会把金额序列化成串）。
    #[test]
    fn accepts_stringified_numbers() {
        let status = parse_quota(r#"{"balance": "18.25", "currency": "USD"}"#).unwrap();
        match status {
            QuotaStatus::Ok { remaining, .. } => assert_eq!(remaining, 18.25),
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 重置时间能透传出来（不解析成时间戳，原样带走）。
    #[test]
    fn keeps_reset_hint_verbatim() {
        let status =
            parse_quota(r#"{"remaining":1,"resets_at":"2026-10-01T00:00:00Z"}"#).unwrap();
        match status {
            QuotaStatus::Ok { resets_at, .. } => {
                assert_eq!(resets_at.as_deref(), Some("2026-10-01T00:00:00Z"));
            }
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 负数额度是"字段语义搞错了"，当作没找到 —— 而不是把 -3 显示给用户。
    #[test]
    fn rejects_negative_amounts() {
        let err = parse_quota(r#"{"remaining": -3}"#).unwrap_err();
        assert!(err.contains("找不到额度字段"), "实际: {err}");
    }

    /// 坏 JSON 的错误信息里必须带上响应开头，方便定位
    /// 「是不是被重定向到了登录页」。
    #[test]
    fn malformed_json_mentions_response_head() {
        let err = parse_quota("<!DOCTYPE html><html>...").unwrap_err();
        assert!(err.contains("不是合法 JSON"));
        assert!(err.contains("<!DOCTYPE html>"));
    }

    /// 完全没有额度字段时给出可行动的提示，而不是一个空 Ok。
    #[test]
    fn missing_quota_field_is_a_clear_error() {
        let err = parse_quota(r#"{"user":"abc","plan":"pro"}"#).unwrap_err();
        assert!(err.contains("找不到额度字段"), "实际: {err}");
    }

    /// body 里带 `error` 字段时，即使 HTTP 200 也要当失败。
    #[test]
    fn body_level_error_is_detected() {
        let err = parse_quota(r#"{"error":"invalid api key"}"#).unwrap_err();
        assert!(err.contains("invalid api key"));
    }

    /// 成功的响应里带一句 `"message":"ok"` 不该被误判成错误 ——
    /// 这是一个很容易踩的坑：只看 message 字段存在与否会把
    /// 所有带状态话术的成功响应都打成失败。
    #[test]
    fn neutral_message_on_success_is_not_an_error() {
        let status = parse_quota(r#"{"message":"ok","remaining":9,"unit":"USD"}"#).unwrap();
        match status {
            QuotaStatus::Ok { remaining, .. } => assert_eq!(remaining, 9.0),
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 状态码 → 状态枚举的映射，尤其是 401 必须走鉴权分支
    /// 而不是笼统的 ApiError（界面上是两句不同的话）。
    #[test]
    fn classifies_http_status() {
        assert!(matches!(
            classify_response(401, ""),
            QuotaStatus::Unauthorized { .. }
        ));
        assert!(matches!(
            classify_response(403, ""),
            QuotaStatus::Unauthorized { .. }
        ));
        assert!(matches!(
            classify_response(503, ""),
            QuotaStatus::ApiError { .. }
        ));
        assert!(matches!(
            classify_response(429, ""),
            QuotaStatus::ApiError { .. }
        ));
        // 200 但内容是坏的，也要落到 ApiError 而不是 panic。
        assert!(matches!(
            classify_response(200, "not json"),
            QuotaStatus::ApiError { .. }
        ));
    }

    /// 未配置必须返回 NotConfigured，且**不能**去发请求。
    #[test]
    fn unconfigured_short_circuits() {
        let config = QuotaConfig::default();
        assert!(!config.is_ready());
        assert_eq!(fetch_blocking(&config), QuotaStatus::NotConfigured);

        // 只有地址、没有 Key：仍然算未配置 —— 没有凭据查不到任何东西。
        // （反过来的"只有 Key 没有地址"是**合法**配置：地址留空走内置端点。）
        let keyless = QuotaConfig {
            api_key: String::new(),
            endpoint: "https://example.com/usage".into(),
        };
        assert!(!keyless.is_ready());
        assert_eq!(fetch_blocking(&keyless), QuotaStatus::NotConfigured);

        // 有 Key、地址留空 → 配置完整（走内置 opencode 端点）。
        let keyed = QuotaConfig {
            api_key: "oc_sk_test".into(),
            endpoint: String::new(),
        };
        assert!(keyed.is_ready());
    }

    /// opencode Go 的三窗口响应必须被识别，且 **`percent` 是已用** ——
    /// 出口一律换算成"剩余"。用 2026-09 抓到的真实响应做样本。
    #[test]
    fn parses_opencode_go_windows() {
        let body = r#"{"usage":{
            "rolling":{"status":"ok","percent":0,"resetsAt":"2026-09-17T11:36:53.835Z"},
            "weekly":{"status":"ok","percent":26,"resetsAt":"2026-09-21T00:00:00.000Z"},
            "monthly":{"status":"ok","percent":13,"resetsAt":"2026-10-14T00:51:38.000Z"}}}"#;
        match parse_quota(body).unwrap() {
            QuotaStatus::Ok {
                remaining,
                unit,
                windows,
                resets_at,
                ..
            } => {
                assert_eq!(windows.len(), 3, "三个窗口都要带上");
                assert_eq!(windows[0].label, "滚动");
                assert_eq!(windows[0].remaining_percent, 100.0, "0% 已用 = 100% 剩余");
                assert_eq!(windows[1].remaining_percent, 74.0, "26% 已用 = 74% 剩余");
                assert_eq!(windows[2].remaining_percent, 87.0);
                assert_eq!(unit, "%");
                // 主数字取**最紧张**的窗口（本周 74%），供托盘/状态栏一行展示。
                assert_eq!(remaining, 74.0);
                assert_eq!(resets_at.as_deref(), Some("2026-09-21T00:00:00.000Z"));
            }
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 直给 `percent_remaining` 的版本优先采信，不再做 100−x。
    #[test]
    fn prefers_explicit_remaining_percent() {
        let body = r#"{"usage":{"rolling":{"percent":80,"percent_remaining":20}}}"#;
        match parse_quota(body).unwrap() {
            QuotaStatus::Ok { windows, .. } => assert_eq!(windows[0].remaining_percent, 20.0),
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 超额（percent > 100）时剩余夹到 0，不给界面送负数。
    #[test]
    fn clamps_over_quota_windows_to_zero() {
        let body = r#"{"usage":{"rolling":{"percent":130}}}"#;
        match parse_quota(body).unwrap() {
            QuotaStatus::Ok { windows, .. } => assert_eq!(windows[0].remaining_percent, 0.0),
            other => panic!("期望 Ok，实际 {other:?}"),
        }
    }

    /// 地址规范化：空 → 内置端点；opencode 网关根路径 → 自动补 `/usage`。
    ///
    /// ★ 守着实测缺陷：用户填 `https://opencode.ai/zen/go/v1`（少一段
    ///   `/usage`）拿到 404 —— 界面显示"接口不存在"，看着像功能不可用，
    ///   其实只差一个路径段。
    #[test]
    fn normalizes_opencode_endpoint() {
        let with_key = |endpoint: &str| QuotaConfig {
            api_key: "oc_sk_test".into(),
            endpoint: endpoint.into(),
        };

        assert_eq!(with_key("").normalized_endpoint(), DEFAULT_ENDPOINT);
        assert_eq!(with_key("   ").normalized_endpoint(), DEFAULT_ENDPOINT);
        // 网关根路径（实测踩过的坑）
        assert_eq!(
            with_key("https://opencode.ai/zen/go/v1").normalized_endpoint(),
            DEFAULT_ENDPOINT
        );
        // 已带 /usage（含多余尾斜杠）→ 归一后原样
        assert_eq!(
            with_key("https://opencode.ai/zen/go/v1/usage/").normalized_endpoint(),
            DEFAULT_ENDPOINT
        );
        // 自建网关（one-api 等）→ 不动用户的地址
        assert_eq!(
            with_key("https://my.gw/api/user/self").normalized_endpoint(),
            "https://my.gw/api/user/self"
        );
    }

    /// 极深嵌套不能把解析器打到栈溢出（远程 DoS 防线）。
    #[test]
    fn deeply_nested_json_does_not_overflow() {
        let mut body = String::from(r#"{"a":"#);
        for _ in 0..200 {
            body.push_str(r#"{"a":"#);
        }
        body.push('1');
        for _ in 0..201 {
            body.push('}');
        }
        // 不 panic 就算通过 —— 找不到字段会返回 Err。
        assert!(parse_quota(&body).is_err());
    }

    #[test]
    fn rounds_to_two_decimals() {
        assert_eq!(round2(12.499_999_9), 12.5);
        assert_eq!(round2(0.1 + 0.2), 0.3);
    }
}
