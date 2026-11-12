//! HGhub Web 服务入口：把 Tauri 客户端的 Rust 后端桥成 HTTP 服务。
//!
//! 路由：
//! - `POST /api/{cmd}`        原 Tauri command 的 HTTP 对应（请求体 = 参数 JSON 对象，
//!                             成功返回结果 JSON，失败返回 400 + JSON 字符串错误信息）
//! - `GET /img|/hls|/seg?u=`  原 `hg://` 自定义协议的 HTTP 对应（封面解密 / 播放列表改写 / 分片代理）
//! - 其余路径                 前端构建产物（dist）静态托管，SPA 回退到 index.html
//!
//! 环境变量：HG_PORT（默认 8892）、HG_DATA_DIR（默认 ./data）、HG_DIST_DIR（默认 ./dist）
//! 会话与线路状态落盘在 HG_DATA_DIR（与客户端版同格式：session.json / 线路文件 / debug.log）。

mod api;
mod auto_account;
mod lines;
mod logging;
mod media;
mod net;
mod network_policy;
mod playback_log;
mod session;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::services::{ServeDir, ServeFile};

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 数据目录（debug.log 落盘用），在 main 启动时注入。
pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("data"))
}

#[derive(Clone)]
struct AppState {
    api: Arc<api::Api>,
}

/* ───────────────────────── 参数读取 ───────────────────────── */

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("缺少参数 {key}"))
}

fn arg_u32(args: &Value, key: &str) -> Result<u32, String> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .ok_or_else(|| format!("缺少参数 {key}"))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("缺少参数 {key}"))
}

fn arg_bool(args: &Value, key: &str) -> Result<bool, String> {
    args.get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("缺少参数 {key}"))
}

fn arg_str_vec(args: &Value, key: &str) -> Result<Vec<String>, String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .ok_or_else(|| format!("缺少参数 {key}"))
}

/// Tauri 会把前端的 camelCase 参数名转成 Rust 的 snake_case（如 replyId → reply_id），
/// 这里手动对齐那一个多词参数。
fn arg_reply_id(args: &Value) -> Option<i64> {
    args.get("replyId")
        .or_else(|| args.get("reply_id"))
        .and_then(|v| v.as_i64())
}

fn to_value<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, String> {
    match r {
        Ok(v) => serde_json::to_value(v).map_err(|e| e.to_string()),
        Err(e) => Err(e),
    }
}

fn unit(r: Result<(), String>) -> Result<Value, String> {
    r.map(|_| Value::Null)
}

/* ───────────────────────── command 分发 ───────────────────────── */

/* 列表类只读接口的响应缓存：上游列表接口单次约 1.4s（隧道往返 + 上游处理），
   缓存命中后本地直接返回；TTL 10 分钟与上游内容更新节奏匹配。
   登录/注册/退出成功后整表作废，避免账号态变化后看到旧列表。 */
static RESP_CACHE: Lazy<Mutex<HashMap<String, (Instant, Value)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const RESP_TTL: Duration = Duration::from_secs(600);
const RESP_CACHE_MAX: usize = 512;

fn cacheable(cmd: &str) -> bool {
    matches!(
        cmd,
        "api_feed" | "api_category" | "api_rank" | "api_posts" | "api_search"
    )
}

fn resp_cache_get(key: &str) -> Option<Value> {
    let cache = RESP_CACHE.lock().ok()?;
    cache
        .get(key)
        .filter(|(exp, _)| Instant::now() < *exp)
        .map(|(_, v)| v.clone())
}

fn resp_cache_put(key: String, val: Value) {
    if let Ok(mut cache) = RESP_CACHE.lock() {
        if cache.len() >= RESP_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, (Instant::now() + RESP_TTL, val));
    }
}

fn resp_cache_clear() {
    if let Ok(mut cache) = RESP_CACHE.lock() {
        cache.clear();
    }
}

async fn run_cmd(api: Arc<api::Api>, cmd: &str, args: &Value) -> Result<Value, String> {
    run_cmd_impl(api, cmd, args, true).await
}

async fn run_cmd_impl(
    api: Arc<api::Api>,
    cmd: &str,
    args: &Value,
    prefetch: bool,
) -> Result<Value, String> {
    if matches!(cmd, "api_login" | "api_register" | "api_logout") {
        let r = run_cmd_inner(&api, cmd, args).await;
        if r.is_ok() {
            resp_cache_clear();
        }
        return r;
    }
    if !cacheable(cmd) {
        return run_cmd_inner(&api, cmd, args).await;
    }
    let key = format!("{cmd}|{args}");
    if let Some(v) = resp_cache_get(&key) {
        return Ok(v);
    }
    let r = run_cmd_inner(&api, cmd, args).await;
    if let Ok(v) = &r {
        resp_cache_put(key, v.clone());
        // 预取下一页（仅一层）：翻页时大概率直接命中缓存。
        // 注：run_cmd 链的 future 不是 Send（移植自 Tauri 的代码里有跨 await 的
        // 非 Send 状态），tokio::spawn 用不了；预取频率低，用独立线程 +
        // current-thread runtime 跑，互不影响主链。
        if prefetch && matches!(cmd, "api_feed" | "api_category") {
            if let Some(page) = args.get("page").and_then(Value::as_u64) {
                let mut next_args = args.clone();
                next_args["page"] = json!(page + 1);
                let api2 = api.clone();
                let cmd2 = cmd.to_string();
                std::thread::spawn(move || {
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        rt.block_on(async move {
                            let _ = run_cmd_impl(api2, &cmd2, &next_args, false).await;
                        });
                    }
                });
            }
        }
    }
    r
}

async fn run_cmd_inner(api: &api::Api, cmd: &str, args: &Value) -> Result<Value, String> {
    use api::*;
    match cmd {
        "app_boot" => app_boot(api).await,
        "security_set" => to_value(security_set(arg_bool(args, "strict")?).await),
        "debug_log" => unit(debug_log(arg_str(args, "msg")?).await),
        "net_diag" => net_diag().await,
        "line_state" => line_state().await,
        "line_set" => to_value(line_set(api, arg_str(args, "url")?).await),
        "line_mirrors" => line_mirrors().await,
        "line_probe" => line_probe(arg_str_vec(args, "urls")?).await,
        "check_update" => check_update().await,
        "open_url" => unit(open_url(arg_str(args, "url")?).await),
        "set_orientation" => unit(set_orientation(arg_bool(args, "landscape")?).await),
        "api_login" => {
            api_login(
                api,
                arg_str(args, "username")?,
                arg_str(args, "password")?,
            )
            .await
        }
        "api_register" => {
            api_register(
                api,
                arg_str(args, "username")?,
                arg_str(args, "password")?,
                arg_str(args, "email").unwrap_or_default(),
            )
            .await
        }
        "api_logout" => api_logout(api).await,
        "api_me" => api_me(api).await,
        "api_feed" => api_feed(api, arg_u32(args, "page")?, arg_u32(args, "size")?).await,
        "api_category" => {
            api_category(
                api,
                arg_str(args, "cate")?,
                arg_u32(args, "page")?,
                arg_u32(args, "size")?,
            )
            .await
        }
        "api_rank" => api_rank(api, arg_str(args, "kind")?).await,
        "api_search" => {
            api_search(
                api,
                arg_str(args, "q")?,
                arg_u32(args, "page")?,
                arg_u32(args, "size")?,
            )
            .await
        }
        "api_posts" => api_posts(api, arg_u32(args, "page")?, arg_u32(args, "size")?).await,
        "api_post" => api_post(api, arg_i64(args, "id")?).await,
        "api_post_like" => {
            api_post_like(api, arg_i64(args, "id")?, arg_bool(args, "on")?).await
        }
        "api_post_collect" => {
            api_post_collect(api, arg_i64(args, "id")?, arg_bool(args, "on")?).await
        }
        "api_post_comments" => {
            api_post_comments(
                api,
                arg_i64(args, "id")?,
                arg_u32(args, "page")?,
                arg_u32(args, "size")?,
            )
            .await
        }
        "api_video" => api_video(api, arg_i64(args, "id")?).await,
        "api_play" => api_play(api, arg_i64(args, "id")?, arg_u32(args, "ep")?).await,
        "api_comments" => {
            api_comments(
                api,
                arg_i64(args, "id")?,
                arg_u32(args, "ep")?,
                arg_u32(args, "page")?,
                arg_u32(args, "size")?,
            )
            .await
        }
        "api_comment_send" => {
            api_comment_send(
                api,
                arg_i64(args, "id")?,
                arg_u32(args, "ep")?,
                arg_str(args, "text")?,
                arg_reply_id(args),
            )
            .await
        }
        "api_like" => {
            api_like(
                api,
                arg_i64(args, "id")?,
                arg_u32(args, "ep")?,
                arg_bool(args, "on")?,
            )
            .await
        }
        "api_collect" => api_collect(api, arg_i64(args, "id")?, arg_bool(args, "on")?).await,
        "api_follow" => api_follow(api, arg_i64(args, "uid")?, arg_bool(args, "on")?).await,
        "cache_clear" => unit(cache_clear().await),
        "playback_log_status" => to_value(playback_log::playback_log_status()),
        "playback_log_set" => {
            to_value(playback_log::playback_log_set(arg_bool(args, "enabled")?))
        }
        "playback_log_write" => {
            let entries: Vec<playback_log::Entry> = args
                .get("entries")
                .cloned()
                .map(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
                .transpose()?
                .unwrap_or_default();
            unit(playback_log::playback_log_write(entries))
        }
        "playback_log_read" => to_value(playback_log::playback_log_read()),
        // 桌面端退出命令：网页版无意义，空实现（与客户端里 .catch(() => {}) 的用法兼容）
        "plugin:app|exit" => Ok(Value::Null),
        other => Err(format!("未知命令 {other}")),
    }
}

/// 只允许同源调用：带 Origin 头且与 Host 不一致时拒绝
/// （挡住局域网里其它网站页面借浏览器直调本服务，等价于客户端版 hg:// 的来源校验）。
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    url::Url::parse(origin)
        .ok()
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(p) => format!("{h}:{p}"),
                None => h.to_string(),
            })
        })
        .is_some_and(|oh| oh.eq_ignore_ascii_case(host))
}

async fn dispatch(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    if !origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, Json(json!("origin denied"))).into_response();
    }
    let args = if args.is_object() { args } else { json!({}) };
    match run_cmd(st.api.clone(), &cmd, &args).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    }
}

/* ───────────────────────── 媒体代理 ───────────────────────── */

async fn media_route(headers: HeaderMap, uri: Uri) -> Response {
    if !origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, Json(json!("origin denied"))).into_response();
    }
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let origin = format!("http://{host}");
    let resp = media::handle(uri.path(), uri.query().unwrap_or(""), &origin).await;
    let (parts, body) = resp.into_parts();
    Response::from_parts(parts, axum::body::Body::from(body))
}

/* ───────────────────────── 启动 ───────────────────────── */

#[tokio::main]
async fn main() {
    let data = PathBuf::from(std::env::var("HG_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let _ = std::fs::create_dir_all(&data);
    let _ = DATA_DIR.set(data.clone());

    let dist = std::env::var("HG_DIST_DIR").unwrap_or_else(|_| "dist".into());
    let port: u16 = std::env::var("HG_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8892);

    let api = api::Api::new();
    net::bind_base(&data);
    playback_log::bind(&data);
    api.session.bind(data.clone());
    let state = AppState { api: Arc::new(api) };

    // 自动账号：无有效登录态时自动注册新号并登录（HGHUB_AUTO_ACCOUNT=force
    // 可强制换号）；上游不稳时只记日志、不阻塞启动（逻辑见 auto_account.rs）。
    // 注：run_cmd 链的 future 不是 Send，用独立线程 + current-thread runtime 跑。
    {
        let api_for_auto = state.api.clone();
        let data_for_auto = data.clone();
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    auto_account::ensure_on_startup(&api_for_auto, &data_for_auto).await;
                });
            }
        });
    }

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/{cmd}", post(dispatch))
        .route("/img", get(media_route))
        .route("/hls", get(media_route))
        .route("/seg", get(media_route))
        .fallback_service(
            ServeDir::new(&dist).fallback(ServeFile::new(format!("{dist}/index.html"))),
        )
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    println!("hghub-server listening on {addr} (data={data:?}, dist={dist})");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    axum::serve(listener, app).await.expect("server error");
}
