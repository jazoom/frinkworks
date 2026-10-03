//! Browser origin checks and response security headers.

use std::any::Any;

use askama::Template;
use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Method, header},
    middleware::Next,
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{rand_core::TryRng, rngs::SysRng};
use url::{Host, Url};

use crate::{config::RuntimeEnvironment, responses, state::AppState};

const PERMISSIONS_POLICY: &str = "accelerometer=(), autoplay=(), camera=(), display-capture=(), \
     encrypted-media=(), fullscreen=(), geolocation=(), gyroscope=(), \
     magnetometer=(), microphone=(), midi=(), payment=(), picture-in-picture=(), \
     publickey-credentials-get=(), screen-wake-lock=(), sync-xhr=(), usb=(), \
     xr-spatial-tracking=()";

const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";

#[derive(Clone)]
pub(crate) struct CspNonce(String);

impl CspNonce {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .expect("system random number generator failed");
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn render<T: Template>(&self, template: &T) -> Result<String, askama::Error> {
        template.render_with_values(&self.values())
    }

    fn values(&self) -> [(&'static str, &dyn Any); 1] {
        [("nonce", &self.0)]
    }
}

pub(crate) async fn enforce_origin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if is_safe_method(request.method()) {
        return next.run(request).await;
    }

    let mut origins = request.headers().get_all(header::ORIGIN).iter();
    let permitted = origins
        .next()
        .filter(|_| origins.next().is_none())
        .and_then(|value| value.to_str().ok())
        .and_then(normalise_origin)
        .is_some_and(|origin| origin == state.config.public_origin());

    if !permitted {
        return responses::origin_failure_response();
    }

    next.run(request).await
}

pub(crate) async fn add_security_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let force_no_store = is_no_store_target(&response);
    let nonce = response.extensions_mut().remove::<CspNonce>();
    let is_production_https = state.config.environment() == RuntimeEnvironment::Production
        && state.config.public_origin().starts_with("https://");
    let live_endpoint =
        hypergraft::live::LiveEndpoint::with_default_path(state.config.public_origin())
            .expect("the public origin is a canonical HTTP origin");
    let headers = response.headers_mut();

    // Browsers enforce every CSP header. A route can tighten but never relax the application policy.
    headers.append(
        header::CONTENT_SECURITY_POLICY,
        content_security_policy(nonce.as_ref(), live_endpoint.csp_connect_src()),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static(REFERRER_POLICY),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );

    if is_production_https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }

    if force_no_store {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }

    response
}

fn is_no_store_target(response: &Response) -> bool {
    if response.status().is_client_error() || response.status().is_server_error() {
        return true;
    }
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next().map(str::trim) == Some("text/html"))
}

fn is_safe_method(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

fn normalise_origin(value: &str) -> Option<String> {
    if value == "null" {
        return None;
    }
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(address) => address.to_string(),
        Host::Ipv6(address) => format!("[{address}]"),
    };
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     base-uri 'self'; \
     connect-src 'self'; \
     font-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     img-src 'self' data:; \
     object-src 'none'; \
     require-trusted-types-for 'script'; \
     script-src 'self'; \
     style-src 'self'; \
     trusted-types hypergraft";

fn content_security_policy(nonce: Option<&CspNonce>, live_connect_src: &str) -> HeaderValue {
    let mut policy = CONTENT_SECURITY_POLICY.replace(
        "connect-src 'self'",
        &format!("connect-src 'self' {live_connect_src}"),
    );
    if let Some(nonce) = nonce {
        policy = policy.replace(
            "script-src 'self'",
            &format!("script-src 'nonce-{}' 'self'", nonce.as_str()),
        );
    }
    HeaderValue::from_str(&policy)
        .expect("the nonce and live endpoint produce a valid header value")
}

#[cfg(test)]
mod tests;
