use std::net::IpAddr;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderName, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use url::{Host, Url};

use crate::compat_process::CompatProcess;

// This client must be used only for the private loopback companion.
pub(crate) fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
}

fn destination(base: &Url, uri: &Uri) -> Option<Url> {
    let loopback = match base.host()? {
        Host::Ipv4(ip) => IpAddr::V4(ip).is_loopback(),
        Host::Ipv6(ip) => IpAddr::V6(ip).is_loopback(),
        Host::Domain(_) => false,
    };
    if !loopback
        || base.scheme() != "http"
        || base.port().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return None;
    }
    let origin = base.origin().ascii_serialization();
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    // Concatenation rather than URL joining prevents //host from replacing authority.
    Url::parse(&format!("{origin}{path}")).ok()
}

fn strip_hop_headers(headers: &mut HeaderMap) {
    let nominated: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|&b| b == b','))
        .filter_map(|part| {
            let trimmed = part.trim_ascii();
            HeaderName::from_bytes(trimmed).ok()
        })
        .collect();
    for name in nominated {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

pub(crate) async fn forward(
    client: &reqwest::Client,
    supervisor: &CompatProcess,
    req: Request,
) -> Response {
    let lease = match supervisor.acquire().await {
        Ok(lease) => lease,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let Some(url) = destination(lease.url(), req.uri()) else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let (mut parts, body) = req.into_parts();
    strip_hop_headers(&mut parts.headers);
    let upstream = client
        .request(parts.method, url)
        .headers(parts.headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let upstream = match upstream {
        Ok(response) => response,
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    let status = upstream.status();
    let mut headers = upstream.headers().clone();
    strip_hop_headers(&mut headers);
    let stream = upstream.bytes_stream();
    let stream = futures_util::stream::unfold(
        (Box::pin(stream), lease),
        |(mut stream, lease)| async move { stream.next().await.map(|chunk| (chunk, (stream, lease))) },
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn strips_connection_tokens_without_losing_cookies() {
        let mut h = HeaderMap::new();
        h.append(
            header::CONNECTION,
            HeaderValue::from_static("keep-alive, X-Remove"),
        );
        h.append(header::CONNECTION, HeaderValue::from_static("x-other"));
        h.insert("x-remove", HeaderValue::from_static("yes"));
        h.insert("x-other", HeaderValue::from_static("yes"));
        h.append(header::SET_COOKIE, HeaderValue::from_static("a=1"));
        h.append(header::SET_COOKIE, HeaderValue::from_static("b=2"));
        strip_hop_headers(&mut h);
        assert!(!h.contains_key("x-remove") && !h.contains_key("x-other"));
        assert_eq!(h.get_all(header::SET_COOKIE).iter().count(), 2);
    }

    #[test]
    fn confines_destination_to_loopback_authority() {
        let base = Url::parse("http://127.0.0.1:1234").unwrap();
        let dest = destination(&base, &"//other.example/path?q=%2F".parse().unwrap()).unwrap();
        assert_eq!(dest.host_str(), Some("127.0.0.1"));
        assert_eq!(dest.query(), Some("q=%2F"));
        assert!(
            destination(
                &Url::parse("http://example.com:1234").unwrap(),
                &"/".parse().unwrap()
            )
            .is_none()
        );
    }
}
