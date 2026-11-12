//! 自动账号（仅 Web 服务版）：启动时若无有效登录态，自动到黄果官网注册新账号并登录。
//!
//! - 默认：当前线路已有有效 token 时什么都不做；无登录态时先试存量自动账号，
//!   失败再注册新号。
//! - 环境变量 `HGHUB_AUTO_ACCOUNT=force`：即使已有登录态也注册新号并切换
//!   （一次性操作；成功后应把该变量从部署配置里去掉，避免每次重启都换号）。
//!
//! 切换前先验证，失败不碰现有会话：注册/登录走与 `api_login` / `api_register`
//! 同一路 `net::edge` 信封；拿到 token 后先调 `/api/account/me` 再调新播放接口
//! 确认能拿到真实 m3u8，验证通过才写 session 并落盘。
//! 上游不稳（502 等）时带重试，全部失败只记日志、不影响服务启动。
//!
//! 账号密码只存数据目录的 `auto-account.json`（不进仓库、不写日志）。

use crate::api::Api;
use crate::net::{base, edge};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILE_NAME: &str = "auto-account.json";
const ENV_VAR: &str = "HGHUB_AUTO_ACCOUNT";
const MAX_REGISTER_ATTEMPTS: usize = 5;
const MAX_LOGIN_ATTEMPTS: usize = 3;
const MAX_VERIFY_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// 播放验证用的已知可播视频（之前实测新播放接口能返回真实 m3u8）。
const VERIFY_VIDEO_ID: i64 = 2833;
const USERNAME_PREFIX: &str = "HG";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoAccount {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub user_id: i64,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub created_at: String,
}

fn log(data_dir: &Path, msg: &str) {
    let _ = crate::logging::append_debug(data_dir, &format!("[auto-account] {msg}"));
}

fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn load(data_dir: &Path) -> Option<AutoAccount> {
    std::fs::read_to_string(file_path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn save(data_dir: &Path, acc: &AutoAccount) {
    let _ = std::fs::create_dir_all(data_dir);
    if let Ok(text) = serde_json::to_string_pretty(acc) {
        let _ = std::fs::write(file_path(data_dir), text);
    }
}

/* ───────────────────────── 随机账号 ───────────────────────── */

const ALPHANUM: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";

fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        return buf;
    }
    // 回退：时间 + 进程号播种的 xorshift（仅用于生成随机用户名/密码）。
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15)
        ^ (std::process::id() as u64).wrapping_mul(0x2545f4914f6cdd1d);
    for b in buf.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 32) as u8;
    }
    buf
}

/// 用户名：`HG` + 8 位随机字母数字（上游要求 4–20 位字母数字下划线）。
fn gen_username() -> String {
    let mut name = String::from(USERNAME_PREFIX);
    for b in random_bytes(8) {
        name.push(ALPHANUM[(b as usize) % ALPHANUM.len()] as char);
    }
    name
}

/// 密码：12 位字母+数字，首位字母、次位数字（上游要求 6–20 位字母+数字）。
fn gen_password() -> String {
    let letters = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz";
    let digits = b"23456789";
    let mut pw = String::new();
    for (i, b) in random_bytes(12).iter().enumerate() {
        let c = if i == 0 {
            letters[(b % letters.len() as u8) as usize]
        } else if i == 1 {
            digits[(b % digits.len() as u8) as usize]
        } else {
            ALPHANUM[(b % ALPHANUM.len() as u8) as usize]
        };
        pw.push(c as char);
    }
    pw
}

fn username_taken(err: &str) -> bool {
    ["已存在", "已被注册", "已被使用", "already", "duplicate", "taken"]
        .iter()
        .any(|s| err.contains(s))
}

/* ───────────────────────── 上游调用（与 api_login/api_register 同路） ───────────────────────── */

async fn register_via_edge(
    origin: &str,
    username: &str,
    password: &str,
) -> Result<(String, Value), String> {
    let data = edge(
        origin,
        "POST",
        "/api/account/registerByPassword",
        Some(json!({
            "username": username,
            "password": password,
            "confirm_password": password,
        })),
        None,
    )
    .await?;
    let token = data
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "注册返回缺少 token".to_string())?
        .to_string();
    let user = data.get("user").cloned().unwrap_or(Value::Null);
    if base() != origin {
        return Err("注册期间线路已变化，请重新登录".into());
    }
    Ok((token, user))
}

async fn login_via_edge(
    origin: &str,
    username: &str,
    password: &str,
) -> Result<(String, Value), String> {
    let data = edge(
        origin,
        "POST",
        "/api/account/loginByPassword",
        Some(json!({ "username": username, "password": password })),
        None,
    )
    .await?;
    let token = data
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "登录返回缺少 token".to_string())?
        .to_string();
    let user = data.get("user").cloned().unwrap_or(Value::Null);
    if base() != origin {
        return Err("登录期间线路已变化，请重新登录".into());
    }
    Ok((token, user))
}

/// 验证 token 有效且能拿到真实播放地址；返回上游用户 id。
async fn verify_token_once(origin: &str, token: &str) -> Result<i64, String> {
    let me = edge(origin, "GET", "/api/account/me", None, Some(token)).await?;
    let user_id = me
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "验证账号返回缺少 id".to_string())?;
    let play = edge(
        origin,
        "POST",
        &format!("/api/videos/{VERIFY_VIDEO_ID}/episodes/0/playback"),
        Some(json!({})),
        Some(token),
    )
    .await?;
    let main = play
        .get("streams")
        .and_then(|s| s.get("main"))
        .and_then(|u| u.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            play.get("video_url")
                .and_then(|u| u.as_str())
                .filter(|s| !s.is_empty())
        });
    match main {
        Some(_) => Ok(user_id),
        None => Err("验证播放未拿到播放地址".into()),
    }
}

async fn verify_token(origin: &str, token: &str) -> Result<i64, String> {
    let mut last = String::new();
    for attempt in 1..=MAX_VERIFY_ATTEMPTS {
        match verify_token_once(origin, token).await {
            Ok(id) => return Ok(id),
            Err(e) => {
                last = e;
                if attempt < MAX_VERIFY_ATTEMPTS {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
    Err(last)
}

/* ───────────────────────── 主流程 ───────────────────────── */

/// 返回 (用户名, 上游用户 id)；失败时现有会话原样不动。
async fn ensure(api: &Api, data_dir: &Path, origin: &str, force: bool) -> Result<(String, i64), String> {
    // 1. 存量自动账号（force 模式跳过：就是要换一个新号）。
    if !force {
        if let Some(acc) = load(data_dir) {
            let mut last = String::new();
            for attempt in 1..=MAX_LOGIN_ATTEMPTS {
                match login_via_edge(origin, &acc.username, &acc.password).await {
                    Ok((token, user)) => match verify_token(origin, &token).await {
                        Ok(user_id) => {
                            api.session.set(origin, token.clone(), user);
                            save(
                                data_dir,
                                &AutoAccount {
                                    username: acc.username.clone(),
                                    password: acc.password.clone(),
                                    user_id,
                                    token,
                                    created_at: acc.created_at.clone(),
                                },
                            );
                            return Ok((acc.username, user_id));
                        }
                        Err(e) => {
                            last = format!("存量账号验证失败: {e}");
                            break;
                        }
                    },
                    Err(e) => {
                        last = e;
                        if attempt < MAX_LOGIN_ATTEMPTS {
                            tokio::time::sleep(RETRY_DELAY).await;
                        }
                    }
                }
            }
            log(
                data_dir,
                &format!("存量自动账号登录失败（{last}），改注册新号"),
            );
        }
    }

    // 2. 注册新号：用户名已存在则换名重试，其它错误同名重试。
    let password = gen_password();
    let mut username = gen_username();
    let mut last_err = String::new();
    for _ in 0..MAX_REGISTER_ATTEMPTS {
        match register_via_edge(origin, &username, &password).await {
            Ok((token, user)) => match verify_token(origin, &token).await {
                Ok(user_id) => {
                    api.session.set(origin, token.clone(), user);
                    let acc = AutoAccount {
                        username: username.clone(),
                        password: password.clone(),
                        user_id,
                        token,
                        created_at: crate::logging::utc_timestamp().unwrap_or_default(),
                    };
                    save(data_dir, &acc);
                    return Ok((acc.username, acc.user_id));
                }
                Err(e) => {
                    last_err = format!("新号验证失败: {e}");
                    break;
                }
            },
            Err(e) => {
                last_err = e.clone();
                if username_taken(&e) {
                    username = gen_username();
                }
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    }
    Err(last_err)
}

/// 启动时调用：已有有效登录态且非 force 时直接返回；否则走自动账号流程。
/// 只记日志（用户名/用户 id，不含密码与 token），失败不影响服务启动。
pub async fn ensure_on_startup(api: &Api, data_dir: &Path) {
    let force = std::env::var(ENV_VAR)
        .map(|v| v.trim().eq_ignore_ascii_case("force"))
        .unwrap_or(false);
    let origin = base();
    if !force && api.session.token(&origin).is_some() {
        return;
    }
    log(
        data_dir,
        &format!(
            "启动检查：mode={} origin={}，开始自动账号流程",
            if force { "force" } else { "auto" },
            origin
        ),
    );
    match ensure(api, data_dir, &origin, force).await {
        Ok((username, user_id)) => {
            log(
                data_dir,
                &format!("自动账号就绪：{username}（id={user_id}），会话已切换"),
            );
        }
        Err(e) => {
            log(
                data_dir,
                &format!("自动账号失败：{e}；现有登录态未动，服务继续运行"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_credentials_match_upstream_rules() {
        for _ in 0..200 {
            let u = gen_username();
            assert!((4..=20).contains(&u.len()));
            assert!(u.starts_with("HG"));
            assert!(u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            let p = gen_password();
            assert!((6..=20).contains(&p.len()));
            assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
            assert!(p.chars().any(|c| c.is_ascii_alphabetic()));
            assert!(p.chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn username_taken_matches_known_messages() {
        assert!(username_taken("用户名已存在"));
        assert!(username_taken("Username already exists"));
        assert!(!username_taken("Bad Gateway"));
        assert!(!username_taken("账号或密码错误"));
    }
}
