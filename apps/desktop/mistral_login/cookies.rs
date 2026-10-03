//! Selects only provider-owned authentication cookies for the fixed usage API.
use tauri::webview::Cookie;
const API_PATH: &str = "/api/local-trpc/";

pub(super) fn matches_api_path(path: &str) -> bool {
    path == API_PATH
        || API_PATH.starts_with(path)
            && (path.ends_with('/') || API_PATH.as_bytes().get(path.len()) == Some(&b'/'))
}

pub(super) fn cookie_header(cookies: &[Cookie<'_>], now: i64) -> Result<String, ()> {
    let mut selected = Vec::new();
    if cookies.len() > 512 {
        return Err(());
    }
    for cookie in cookies {
        let domain = cookie.domain().unwrap_or_default().trim_start_matches('.');
        if !matches!(domain, "admin.mistral.ai" | "mistral.ai")
            || !matches_api_path(cookie.path().unwrap_or("/"))
            || cookie
                .expires_datetime()
                .is_some_and(|expiry| expiry.unix_timestamp() <= now)
        {
            continue;
        }
        let name = cookie.name();
        let value = cookie.value();
        // RFC 6265 permits a cookie value enclosed in double quotes. Preserve
        // that browser representation, validating the enclosed cookie octets.
        let octets = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || !octets
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && !b"\";,\\".contains(&byte))
        {
            // Optional analytics cookies must not discard a usable session.
            // The remaining header still requires provider verification.
            continue;
        }
        selected.push((
            cookie.path().unwrap_or("/").len(),
            format!("{name}={value}"),
        ));
    }
    selected.sort_by_key(|cookie| std::cmp::Reverse(cookie.0));
    let header = selected
        .into_iter()
        .map(|(_, cookie)| cookie)
        .collect::<Vec<_>>()
        .join("; ");
    if header.is_empty() || header.len() > 32768 {
        return Err(());
    }
    Ok(header)
}
