//! Provider-session cookie.

use axum::http::{HeaderMap, HeaderValue, header};
use cookie::{Cookie, SameSite, time::Duration};

use crate::{config::RuntimeConfig, sessions::tokens::ValidatedToken};

pub(crate) const SESSION_COOKIE_NAME: &str = "frinkworks_session";

#[derive(Debug)]
pub(crate) enum CookieRead {
    Missing,
    Valid(ValidatedToken),
    Invalid,
}

#[derive(Debug)]
pub(crate) struct CookieError(&'static str);

impl std::fmt::Display for CookieError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for CookieError {}

pub(crate) fn read_session(headers: &HeaderMap) -> CookieRead {
    let mut token: Option<ValidatedToken> = None;

    for header_value in headers.get_all(header::COOKIE) {
        let Ok(raw) = header_value.to_str() else {
            return CookieRead::Invalid;
        };
        if raw.is_empty() {
            return CookieRead::Invalid;
        }

        for source in raw.split(';').map(str::trim) {
            if source.is_empty() {
                return CookieRead::Invalid;
            }
            let Some((cookie_name, cookie_value)) = source.split_once('=') else {
                return CookieRead::Invalid;
            };
            if !valid_cookie_name(cookie_name) || !valid_cookie_value(cookie_value) {
                return CookieRead::Invalid;
            }
            let Ok(cookie) = Cookie::parse(source) else {
                return CookieRead::Invalid;
            };
            if cookie.name() != cookie_name || cookie_name != SESSION_COOKIE_NAME {
                continue;
            }

            let value = cookie.value();
            if token.is_some() {
                return CookieRead::Invalid;
            }
            let Some(validated) = ValidatedToken::parse(value) else {
                return CookieRead::Invalid;
            };
            token = Some(validated);
        }
    }

    match token {
        Some(validated) => CookieRead::Valid(validated),
        None => CookieRead::Missing,
    }
}

pub(crate) fn session_set_header(
    config: &RuntimeConfig,
    token: &ValidatedToken,
) -> Result<HeaderValue, CookieError> {
    let cookie = build_cookie(config, token.as_str().to_owned())
        .max_age(Duration::hours(
            i64::try_from(crate::sessions::SESSION_LIFETIME_HOURS)
                .expect("session lifetime hours fit cookie max-age"),
        ))
        .build();
    render_header(cookie)
}

pub(crate) fn session_deletion_header(config: &RuntimeConfig) -> Result<HeaderValue, CookieError> {
    let mut cookie = build_cookie(config, String::new()).build();
    cookie.make_removal();
    render_header(cookie)
}

pub(crate) fn response_has_replacement(headers: &HeaderMap) -> bool {
    headers.get_all(header::SET_COOKIE).iter().any(|value| {
        let Ok(value) = value.to_str() else {
            return false;
        };
        let Some(pair) = value.split(';').next() else {
            return false;
        };
        let Ok(cookie) = Cookie::parse(pair) else {
            return false;
        };
        cookie.name() == SESSION_COOKIE_NAME && ValidatedToken::parse(cookie.value()).is_some()
    })
}

fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_cookie_value(value: &str) -> bool {
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    value
        .bytes()
        .all(|byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e))
}

fn build_cookie(config: &RuntimeConfig, value: String) -> cookie::CookieBuilder<'static> {
    Cookie::build((SESSION_COOKIE_NAME, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(config.uses_secure_cookies())
}

fn render_header(cookie: Cookie<'_>) -> Result<HeaderValue, CookieError> {
    HeaderValue::from_str(&cookie.to_string())
        .map_err(|_| CookieError("cookie construction failed"))
}

#[cfg(test)]
mod tests;
