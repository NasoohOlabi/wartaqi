use crate::session_identity;
use axum::http::{HeaderMap, HeaderValue, header};
use ring::digest::{SHA256, digest};

pub(crate) const SLOT_COUNT: usize = 5;
const MAX_CANDIDATES: usize = 32;

#[derive(Clone, Debug)]
pub(crate) struct Account {
    pub value: String,
    pub marker: Option<String>,
    pub user_id: String,
    pub name: String,
    pub identifier: String,
    pub expires_http: String,
}

pub(crate) enum SwitchOutcome {
    Selected(Vec<String>),
    Expired,
    LimitReached,
}

fn prefix(secure: bool) -> &'static str {
    if secure { "__Secure-" } else { "" }
}
fn session_name(secure: bool) -> String {
    format!("{}better-auth.session_token", prefix(secure))
}
fn marker_name(secure: bool) -> String {
    format!("{}better-auth.dont_remember", prefix(secure))
}

pub(crate) fn slot_name(user_id: &str, secure: bool) -> String {
    let hash = digest(&SHA256, user_id.as_bytes());
    let hex: String = hash.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("{}device-session-{hex}", prefix(secure))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .map(str::trim)
        .find_map(|part| {
            part.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
        })
}

pub(crate) fn slot_cookies(headers: &HeaderMap, secure: bool) -> Vec<(String, String)> {
    let prefix = format!("{}device-session-", prefix(secure));
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .map(str::trim)
        .filter_map(|part| {
            let (name, value) = part.split_once('=')?;
            let hash = name.strip_prefix(&prefix)?;
            (hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .then(|| {
                (
                    name.to_owned(),
                    value.split('=').next().unwrap_or("").to_owned(),
                )
            })
        })
        .take(MAX_CANDIDATES)
        .collect()
}

pub(crate) async fn remembered(
    db: &tokio_postgres::Client,
    value: &str,
    marker: Option<&str>,
    secret: &str,
    secure: bool,
) -> Result<Option<Account>, tokio_postgres::Error> {
    if value.is_empty() {
        return Ok(None);
    }
    let Ok(cookie) = HeaderValue::from_str(&format!("{}={value}", session_name(secure))) else {
        return Ok(None);
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, cookie);
    let Some(token) = session_identity::token(&headers, secret, secure) else {
        return Ok(None);
    };
    let row = db
        .query_opt(
            r#"
        SELECT u.id::text, u.name, COALESCE(a.verified_phone, ''),
          to_char(s."expiresAt" AT TIME ZONE 'UTC', 'Dy, DD Mon YYYY HH24:MI:SS "GMT"')
        FROM neon_auth.session s JOIN neon_auth."user" u ON u.id = s."userId"
        LEFT JOIN app_accounts a ON a.auth_user_id = u.id::text
        WHERE s.token = $1 AND s."expiresAt" > statement_timestamp()
    "#,
            &[&token],
        )
        .await?;
    Ok(row.map(|r| Account {
        value: value.to_owned(),
        marker: marker.filter(|m| !m.is_empty()).map(str::to_owned),
        user_id: r.get(0),
        name: r.get(1),
        identifier: r.get(2),
        expires_http: r.get(3),
    }))
}

pub(crate) async fn active(
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    secret: &str,
    secure: bool,
) -> Result<Option<Account>, tokio_postgres::Error> {
    remembered(
        db,
        cookie_value(headers, &session_name(secure)).unwrap_or(""),
        cookie_value(headers, &marker_name(secure)),
        secret,
        secure,
    )
    .await
}

pub(crate) async fn list(
    db: &tokio_postgres::Client,
    headers: &HeaderMap,
    secret: &str,
    secure: bool,
    active: Option<&Account>,
) -> Result<Vec<Account>, tokio_postgres::Error> {
    let mut accounts = Vec::new();
    if let Some(account) = active {
        accounts.push(account.clone());
    }
    for (name, value) in slot_cookies(headers, secure) {
        let Some((marker, value)) = value.split_once('~') else {
            continue;
        };
        let account = if let Some(account) =
            active.filter(|a| name == slot_name(&a.user_id, secure) && value == a.value)
        {
            Some(account.clone())
        } else {
            remembered(db, value, Some(marker), secret, secure)
                .await?
                .filter(|a| name == slot_name(&a.user_id, secure))
        };
        if let Some(account) = account
            && !accounts.iter().any(|a| a.user_id == account.user_id)
        {
            accounts.push(account);
        }
    }
    Ok(accounts)
}

fn set_cookie(
    name: &str,
    value: &str,
    expires: Option<&str>,
    remove: bool,
    secure: bool,
) -> String {
    let lifetime = if remove {
        "; Max-Age=0".to_owned()
    } else {
        expires
            .map(|d| format!("; Expires={d}"))
            .unwrap_or_default()
    };
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax{lifetime}{}",
        if secure { "; Secure" } else { "" }
    )
}

pub(crate) fn save_cookie(account: &Account, secure: bool) -> String {
    set_cookie(
        &slot_name(&account.user_id, secure),
        &format!(
            "{}~{}",
            account.marker.as_deref().unwrap_or(""),
            account.value
        ),
        if account.marker.is_some() {
            None
        } else {
            Some(&account.expires_http)
        },
        false,
        secure,
    )
}

pub(crate) fn remove_cookie(name: &str, secure: bool) -> String {
    set_cookie(name, "", None, true, secure)
}

pub(crate) fn switch(
    headers: &HeaderMap,
    active: Option<&Account>,
    accounts: &[Account],
    user_id: &str,
    secure: bool,
) -> SwitchOutcome {
    let Some(selected) = accounts.iter().find(|a| a.user_id == user_id) else {
        return SwitchOutcome::Expired;
    };
    let mut cookies = Vec::new();
    if let Some(account) = active.filter(|a| a.user_id != selected.user_id) {
        let expected = format!(
            "{}~{}",
            account.marker.as_deref().unwrap_or(""),
            account.value
        );
        if cookie_value(headers, &slot_name(&account.user_id, secure)) != Some(expected.as_str()) {
            if accounts.len() > SLOT_COUNT {
                return SwitchOutcome::LimitReached;
            }
            cookies.push(save_cookie(account, secure));
        }
    }
    cookies.push(set_cookie(
        &session_name(secure),
        &selected.value,
        if selected.marker.is_some() {
            None
        } else {
            Some(&selected.expires_http)
        },
        false,
        secure,
    ));
    cookies.push(set_cookie(
        &marker_name(secure),
        selected.marker.as_deref().unwrap_or(""),
        None,
        selected.marker.is_none(),
        secure,
    ));
    SwitchOutcome::Selected(cookies)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slot_names_filter_candidates_and_switch_session_cookies() {
        assert_eq!(
            slot_name("abc", false),
            "device-session-ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let account = Account {
            value: "signed".into(),
            marker: Some("marker".into()),
            user_id: "abc".into(),
            name: "name".into(),
            identifier: "".into(),
            expires_http: "Wed, 01 Jan 2031 00:00:00 GMT".into(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "{}=marker~signed; device-session-nope=x",
                slot_name("abc", false)
            ))
            .unwrap(),
        );
        assert_eq!(slot_cookies(&headers, false).len(), 1);
        assert!(!save_cookie(&account, false).contains("Expires="));
        let SwitchOutcome::Selected(cookies) = switch(&headers, None, &[account], "abc", false)
        else {
            panic!()
        };
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].starts_with("better-auth.session_token=signed;"));
        assert!(cookies[1].starts_with("better-auth.dont_remember=marker;"));
    }
}
