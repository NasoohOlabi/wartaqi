use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::digest::{SHA256, digest};

fn decoded_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16).map(|n| n as u8);
            decoded.push(hex(*bytes.get(i + 1)?)? * 16 + hex(*bytes.get(i + 2)?)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    if !path.starts_with('/') || path.contains("..") || path.contains('\\') || path.contains('\0') {
        return None;
    }
    Some(path)
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html;charset=utf-8",
        "js" | "mjs" => "text/javascript;charset=utf-8",
        "css" => "text/css;charset=utf-8",
        "txt" => "text/plain;charset=utf-8",
        "json" | "map" => "application/json;charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

// Explicit coding preferences override the wildcard; invalid quality values are ignored.
fn encoding_quality(headers: &HeaderMap, coding: &str) -> f32 {
    let mut explicit = None;
    let mut wildcard = None;
    for item in headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
    {
        let mut parts = item.trim().split(';');
        let name = parts.next().unwrap_or("").trim();
        let mut quality = Some(1.0_f32);
        for parameter in parts {
            let Some((key, value)) = parameter.trim().split_once('=') else {
                quality = None;
                break;
            };
            if key.trim().eq_ignore_ascii_case("q") {
                quality = value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|q| q.is_finite() && (0.0..=1.0).contains(q));
            }
        }
        if name.eq_ignore_ascii_case(coding) {
            explicit = Some(quality.unwrap_or(0.0));
        } else if name == "*" {
            wildcard = Some(quality.unwrap_or(0.0));
        }
    }
    if coding == "identity" {
        explicit.unwrap_or(if wildcard == Some(0.0) { 0.0 } else { 1.0 })
    } else {
        explicit.or(wildcard).unwrap_or(0.0)
    }
}

/// Serve a non-API request from the configured asset root.
pub(crate) fn serve(root: &Path, method: &Method, path: &str, headers: &HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed").into_response();
    }
    let Some(path) = decoded_path(path) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let relative = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if relative.ends_with(".br") || relative.ends_with(".gz") {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let stem = relative
        .strip_suffix(".js")
        .or_else(|| relative.strip_suffix(".css"));
    let eligible = relative.starts_with("assets/")
        && stem.is_some_and(|stem| {
            let bytes = stem.as_bytes();
            bytes.len() >= 9
                && bytes[bytes.len() - 9] == b'-'
                && bytes[bytes.len() - 8..]
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        });
    let mut selected = None;
    if eligible {
        let mut encodings = [
            ("br", encoding_quality(headers, "br")),
            ("gzip", encoding_quality(headers, "gzip")),
        ];
        encodings.sort_by(|a, b| b.1.total_cmp(&a.1));
        // Check the original exists before serving a sibling, but do not read both files.
        if let Some(original) = safe_file(root, relative) {
            for (encoding, quality) in encodings {
                if quality <= 0.0 {
                    continue;
                }
                let suffix = if encoding == "br" { ".br" } else { ".gz" };
                if let Some((_, bytes)) = read_file(root, &format!("{relative}{suffix}")) {
                    selected = Some((original, bytes, Some(encoding)));
                    break;
                }
            }
        }
    }
    if eligible
        && selected.is_none()
        && encoding_quality(headers, "identity") == 0.0
        && safe_file(root, relative).is_some()
    {
        return (StatusCode::NOT_ACCEPTABLE, "No acceptable encoding").into_response();
    }
    let Some((file, bytes, encoding)) =
        selected.or_else(|| read_file(root, relative).map(|(file, bytes)| (file, bytes, None)))
    else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let length = bytes.len();
    // Vite puts content-hashed build output in assets/.
    let (cache, tag) = if relative.starts_with("assets/") {
        ("public, max-age=31536000, immutable", None)
    } else {
        // The same content can be served with different content encodings.
        let fingerprint = URL_SAFE_NO_PAD.encode(digest(&SHA256, &bytes).as_ref());
        ("no-cache", Some(format!("W/\"{fingerprint}\"")))
    };
    let response = Response::builder()
        .header(header::CACHE_CONTROL, cache)
        .header("x-content-type-options", "nosniff");
    let response = if let Some(tag) = tag {
        let unchanged = headers
            .get_all(header::IF_NONE_MATCH)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .any(|value| value == "*" || value.strip_prefix("W/").unwrap_or(value) == &tag[2..]);
        let response = response.header(header::ETAG, tag);
        if unchanged {
            return response
                .status(StatusCode::NOT_MODIFIED)
                .header(header::VARY, "accept-encoding")
                .body(Body::empty())
                .expect("static cache headers are valid");
        }
        response
    } else {
        response
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    let response = response.header(header::VARY, "accept-encoding");
    let response = if let Some(encoding) = encoding {
        response.header(header::CONTENT_ENCODING, encoding)
    } else {
        response
    };
    response
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&file))
        .header(header::CONTENT_LENGTH, length)
        .body(body)
        .expect("static response headers are valid")
}

fn safe_file(root: &Path, relative: &str) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    if !file.starts_with(&root) || !file.is_file() {
        return None;
    }
    Some(file)
}

fn read_file(root: &Path, relative: &str) -> Option<(PathBuf, Vec<u8>)> {
    let file = safe_file(root, relative)?;
    let bytes = std::fs::read(&file).ok()?;
    Some((file, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_decoded_and_guarded() {
        assert_eq!(
            decoded_path("/assets/a%20b.js").as_deref(),
            Some("/assets/a b.js")
        );
        for path in [
            "/%2e%2e/secret",
            "/%5csecret",
            "/%00",
            "/%ff",
            "/%",
            "relative",
        ] {
            assert!(decoded_path(path).is_none(), "{path}");
        }
    }
    #[test]
    fn negotiates_precompressed_assets() {
        let root = std::env::temp_dir().join(format!("wartaq-compressed-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app-abcdef12.js"), "original").unwrap();
        std::fs::write(root.join("assets/app-abcdef12.js.br"), "brotli").unwrap();
        std::fs::write(root.join("assets/app-abcdef12.js.gz"), "gzip").unwrap();
        let mut headers = HeaderMap::new();
        for (value, expected) in [
            ("br, gzip", Some("br")),
            ("br;q=0, gzip;q=0.5", Some("gzip")),
            ("*;q=0.5, br;q=0", Some("gzip")),
            ("br;q=0, gzip;q=0", None),
            ("br;q=0.2, gzip;q=0.8", Some("gzip")),
        ] {
            headers.insert(header::ACCEPT_ENCODING, value.parse().unwrap());
            let response = serve(&root, &Method::GET, "/assets/app-abcdef12.js", &headers);
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_ENCODING)
                    .map(|v| v.to_str().unwrap()),
                expected,
                "{value}"
            );
            assert_eq!(response.headers()[header::VARY], "accept-encoding");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/javascript;charset=utf-8"
            );
            assert_eq!(
                response.headers()[header::CONTENT_LENGTH],
                match expected {
                    Some("br") => "6",
                    Some("gzip") => "4",
                    _ => "8",
                }
            );
        }
        headers.insert(header::ACCEPT_ENCODING, "*;q=0".parse().unwrap());
        assert_eq!(
            serve(&root, &Method::GET, "/assets/app-abcdef12.js", &headers).status(),
            StatusCode::NOT_ACCEPTABLE
        );
        assert_eq!(
            serve(&root, &Method::GET, "/assets/app-abcdef12.js.br", &headers).status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn responses_have_expected_status_and_headers() {
        let root = std::env::temp_dir().join(format!("wartaq-assets-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "hello").unwrap();
        let mut headers = HeaderMap::new();
        let response = serve(&root, &Method::HEAD, "/", &headers);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html;charset=utf-8"
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "5");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        headers.insert(
            header::IF_NONE_MATCH,
            response.headers()[header::ETAG].clone(),
        );
        let cached = serve(&root, &Method::GET, "/", &headers);
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        std::fs::write(root.join("index.html"), "new").unwrap();
        assert_eq!(
            serve(&root, &Method::GET, "/", &headers).status(),
            StatusCode::OK
        );
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app-abcdef12.js"), "export {}").unwrap();
        let asset = serve(&root, &Method::GET, "/assets/app-abcdef12.js", &headers);
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            serve(&root, &Method::POST, "/", &headers).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            serve(&root, &Method::GET, "/missing", &headers).status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
