use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};

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

/// Serve a non-API request from the configured asset root.
pub(crate) fn serve(root: &Path, method: &Method, path: &str) -> Response {
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
    let Some((file, bytes)) = read_file(root, relative) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let length = bytes.len();
    // Vite puts content-hashed build output in assets/.
    let cache = if relative.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&file))
        .header(header::CONTENT_LENGTH, length)
        .header(header::CACHE_CONTROL, cache)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("static response headers are valid")
}

fn read_file(root: &Path, relative: &str) -> Option<(PathBuf, Vec<u8>)> {
    let root = root.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    if !file.starts_with(&root) || !file.is_file() {
        return None;
    }
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
    fn responses_have_expected_status_and_headers() {
        let root = std::env::temp_dir().join(format!("wartaq-assets-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), "hello").unwrap();
        let response = serve(&root, &Method::HEAD, "/");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html;charset=utf-8"
        );
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "5");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/app-abcdef12.js"), "export {}").unwrap();
        let asset = serve(&root, &Method::GET, "/assets/app-abcdef12.js");
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            serve(&root, &Method::POST, "/").status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            serve(&root, &Method::GET, "/missing").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
