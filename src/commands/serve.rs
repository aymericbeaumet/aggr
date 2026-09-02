//! `aggr serve`: build, then serve the output directory over plain HTTP/1.1 on localhost.
//! Just enough of a server to click through the site; nothing here is meant to face the internet.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::{Project, build};
use crate::cli::ServeArgs;

pub async fn run(project: &Project, args: &ServeArgs) -> Result<()> {
    if !args.no_build {
        build::run(project, &args.build)?;
    }
    let root = build::out_dir(project, &args.build);
    if !root.is_dir() {
        anyhow::bail!(
            "{} does not exist; run without --no-build first",
            root.display()
        );
    }
    let base = crate::site::base_path(build::base_url(project, &args.build)?.as_deref());
    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .await
        .with_context(|| format!("binding 127.0.0.1:{}", args.port))?;
    println!(
        "serving {} at http://127.0.0.1:{}{base} (ctrl-c to stop)",
        root.display(),
        listener.local_addr()?.port()
    );
    loop {
        let (stream, _) = listener.accept().await?;
        let root = root.clone();
        let base = base.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(stream, &root, &base).await {
                log::debug!("serve: {err:#}");
            }
        });
    }
}

async fn handle(stream: TcpStream, root: &Path, base: &str) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    // Drain the headers; we never need them.
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 && line != "\r\n" && line != "\n" {
        line.clear();
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let (status, file) = match resolve(root, base, path) {
        Some(file) => ("200 OK", file),
        None => ("404 Not Found", root.join("404.html")),
    };
    let body = std::fs::read(&file).unwrap_or_else(|_| b"not found".to_vec());
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        content_type(&file),
        body.len()
    );
    let stream = reader.get_mut();
    stream.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(&body).await?;
    }
    stream.shutdown().await?;
    Ok(())
}

/// Map a request path to a file under `root`, honoring the base path a release build was made
/// for, serving `index.html` for directories, and refusing anything that escapes the root.
pub fn resolve(root: &Path, base: &str, path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path);
    let rest = decoded.strip_prefix(base.trim_end_matches('/'))?;
    let mut file = root.to_path_buf();
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        match Path::new(segment).components().next() {
            Some(Component::Normal(_)) => file.push(segment),
            _ => return None,
        }
    }
    if file.is_dir() {
        if !rest.is_empty() && !rest.ends_with('/') {
            // Directory pages are only ever linked with a trailing slash.
            return None;
        }
        file.push("index.html");
    }
    file.is_file().then_some(file)
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("xml") => "application/xml",
        Some("md") => "text/markdown; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_under_the_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("items/a b")).unwrap();
        std::fs::write(root.join("index.html"), "").unwrap();
        std::fs::write(root.join("items/a b/index.html"), "").unwrap();
        std::fs::write(root.join("search.json"), "").unwrap();

        assert_eq!(resolve(root, "/", "/"), Some(root.join("index.html")));
        assert_eq!(
            resolve(root, "/", "/items/a%20b/"),
            Some(root.join("items/a b/index.html"))
        );
        assert_eq!(resolve(root, "/", "/items/a%20b"), None);
        assert_eq!(
            resolve(root, "/", "/search.json"),
            Some(root.join("search.json"))
        );
        assert_eq!(resolve(root, "/", "/nope/"), None);
        assert_eq!(resolve(root, "/", "/../Cargo.toml"), None);
        assert_eq!(resolve(root, "/", "/%2e%2e/Cargo.toml"), None);

        assert_eq!(
            resolve(root, "/repo/", "/repo/"),
            Some(root.join("index.html"))
        );
        assert_eq!(
            resolve(root, "/repo/", "/repo/search.json"),
            Some(root.join("search.json"))
        );
        assert_eq!(resolve(root, "/repo/", "/search.json"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.json")), "application/json");
        assert_eq!(
            content_type(Path::new("manifest.webmanifest")),
            "application/manifest+json"
        );
        assert_eq!(content_type(Path::new("CNAME")), "application/octet-stream");
    }
}
