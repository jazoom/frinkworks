use super::{CookieRead, read_session, session_deletion_header, session_set_header};
use axum::http::{HeaderMap, header};
use cookie::Cookie;

use crate::config::RuntimeConfig;
use crate::sessions::{self, SESSION_LIFETIME_HOURS};

#[test]
fn missing_cookie_is_anonymous() {
    assert!(matches!(
        read_session(&HeaderMap::new()),
        CookieRead::Missing
    ));
}

#[test]
fn invalid_cookie_value_is_invalid() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "frinkworks_session=not-a-token".parse().unwrap(),
    );
    assert!(matches!(read_session(&headers), CookieRead::Invalid));
}

#[test]
fn duplicate_session_cookies_are_invalid() {
    let mut headers = HeaderMap::new();
    headers.append(
        header::COOKIE,
        "frinkworks_session=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap(),
    );
    headers.append(
        header::COOKIE,
        "frinkworks_session=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .parse()
            .unwrap(),
    );
    assert!(matches!(read_session(&headers), CookieRead::Invalid));
}

#[test]
fn session_cookie_uses_the_server_lifetime() {
    let token = sessions::generate_session_token().expect("token");
    let header = session_set_header(&RuntimeConfig::development(), token.raw()).unwrap();
    let cookie = Cookie::parse(header.to_str().unwrap()).unwrap();
    assert_eq!(
        cookie.max_age(),
        Some(cookie::time::Duration::hours(
            i64::try_from(SESSION_LIFETIME_HOURS).expect("hours fit")
        ))
    );
}

#[test]
fn deletion_header_has_no_token() {
    let header = session_deletion_header(&RuntimeConfig::development()).unwrap();
    let cookie = Cookie::parse(header.to_str().unwrap()).unwrap();
    assert_eq!(cookie.name(), "frinkworks_session");
    assert!(cookie.value().is_empty());
}
