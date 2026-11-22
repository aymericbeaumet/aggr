//! Strict publication keeps automatic resource loads inside the generated site.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use scraper::{Html, Selector};

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(super) fn reader_body(body: &str) -> Result<String> {
    let document = Html::parse_fragment(body);
    let selector = Selector::parse("iframe, video, audio, object, embed, a[data-video-embed]")
        .map_err(|err| anyhow::anyhow!("media selector: {err:?}"))?;
    // Serialize both the fragment and selected elements from the same parsed tree. Original
    // quote style, attribute order, and tag casing must not prevent an embed from being removed.
    let mut output = document.root_element().inner_html();
    for element in document.select(&selector) {
        let target = ["href", "src", "data", "data-video-embed"]
            .iter()
            .find_map(|name| element.value().attr(name))
            .filter(|value| {
                url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            });
        let replacement = target.map(|url| format!(
            "<p><a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">Open original media</a></p>", escape(url)
        )).unwrap_or_default();
        output = output.replace(&element.html(), &replacement);
    }
    Ok(output)
}

/// Validate rendered references before promotion, so a failed strict build preserves prior output.
pub(crate) fn verify(out: &Path, base_url: Option<&str>) -> Result<()> {
    let mut base = url::Url::parse(base_url.unwrap_or("https://aggr.invalid/"))?;
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let selector = Selector::parse(
        "img, source, script[src], link[href], iframe, object, embed, audio, video, image",
    )
    .map_err(|err| anyhow::anyhow!("resource selector: {err:?}"))?;
    let styles = Selector::parse("style, [style]")
        .map_err(|err| anyhow::anyhow!("style selector: {err:?}"))?;
    let css_urls = regex::Regex::new(
        r#"(?i)url\(\s*(?:"([^"]*)"|'([^']*)'|([^\s)]+))\s*\)|@import\s+(?:"([^"]*)"|'([^']*)')"#,
    )?;
    for entry in walkdir::WalkDir::new(out).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "hermetic output contains a symlink: {}",
                entry.path().display()
            );
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let extension = entry.path().extension().and_then(|value| value.to_str());
        let relative = entry
            .path()
            .strip_prefix(out)?
            .to_string_lossy()
            .replace('\\', "/");
        let page = base.join(&relative)?;
        if extension == Some("css") {
            let css = std::fs::read_to_string(entry.path())?;
            verify_css(out, &base, &page, &css, &relative, &css_urls)?;
        }
        if extension != Some("html") {
            continue;
        }
        let html = std::fs::read_to_string(entry.path())?;
        let document = Html::parse_document(&html);
        for element in document.select(&styles) {
            let css = if element.value().name() == "style" {
                element.inner_html()
            } else {
                element.value().attr("style").unwrap_or_default().to_owned()
            };
            verify_css(out, &base, &page, &css, &relative, &css_urls)?;
        }
        for element in document.select(&selector) {
            if element.value().name() == "link"
                && !element.value().attr("rel").is_some_and(|rel| {
                    rel.split_whitespace().any(|rel| {
                        matches!(
                            rel.to_ascii_lowercase().as_str(),
                            "stylesheet"
                                | "icon"
                                | "apple-touch-icon"
                                | "preload"
                                | "modulepreload"
                                | "manifest"
                                | "prefetch"
                                | "preconnect"
                                | "dns-prefetch"
                        )
                    })
                })
            {
                continue;
            }
            let attributes: &[&str] = match element.value().name() {
                "link" => &["href"],
                "image" => &["href", "xlink:href"],
                _ => &["src", "poster", "data"],
            };
            for resource in attributes
                .iter()
                .filter_map(|name| element.value().attr(name))
            {
                verify_resource(out, &base, &page, resource, &relative)?;
            }
            if let Some(srcset) = element.value().attr("srcset") {
                for resource in crate::media::srcset::candidates(srcset) {
                    verify_resource(out, &base, &page, resource.url, &relative)?;
                }
            }
        }
    }
    Ok(())
}

fn verify_css(
    out: &Path,
    base: &url::Url,
    page: &url::Url,
    css: &str,
    owner: &str,
    urls: &regex::Regex,
) -> Result<()> {
    for capture in urls.captures_iter(css) {
        if let Some(resource) = capture.iter().skip(1).flatten().next() {
            verify_resource(out, base, page, resource.as_str(), owner)?;
        }
    }
    Ok(())
}

fn verify_resource(
    out: &Path,
    base: &url::Url,
    page: &url::Url,
    resource: &str,
    owner: &str,
) -> Result<()> {
    // Tiny placeholders and the embedded search stylesheet's SVG icons carry their own bytes.
    // Content sanitization is separate; this check establishes resource locality.
    if resource.starts_with("data:image/") || resource.starts_with("data:font/") {
        return Ok(());
    }
    let resource = page
        .join(resource)
        .with_context(|| format!("hermetic resource in {owner}"))?;
    if resource.origin() != base.origin() || !matches!(resource.scheme(), "http" | "https") {
        bail!(
            "hermetic build: missing local asset {resource} in {owner}; enable local preservation and run sync --backfill-media first"
        );
    }
    let base_path = base.path().trim_end_matches('/');
    let relative = resource
        .path()
        .strip_prefix(&format!("{base_path}/"))
        .context("hermetic resource escapes the site base path")?;
    let root = out.canonicalize()?;
    let mut target = url::Url::from_directory_path(&root)
        .map_err(|()| anyhow::anyhow!("invalid output path"))?
        .join(relative)?
        .to_file_path()
        .map_err(|()| anyhow::anyhow!("invalid local resource path"))?;
    // A prefetched article is the directory the reader opens; its file is index.html.
    if relative.ends_with('/') {
        target.push("index.html");
    }
    if !target
        .canonicalize()
        .is_ok_and(|target| target.starts_with(&root) && target.is_file())
    {
        bail!("hermetic build: missing local asset {relative} in {owner}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_become_links_and_prose_remains() {
        let result = reader_body(
            "<p>Before</p><IFRAME src='https://video.example/embed'></IFRAME><p>After</p>",
        )
        .unwrap();
        assert!(!result.contains("<iframe"));
        assert!(result.contains("href=\"https://video.example/embed\""));
        assert!(result.contains("<p>After</p>"));
    }

    #[test]
    fn all_styles_and_local_resource_existence_are_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for html in [
            "<style>@import 'HTTPS://example.com/theme.css';</style>",
            "<div style=\"background:url(https://example.com/pixel.png)\"></div>",
            "<link rel='STYLESHEET' href='https://example.com/theme.css'>",
            "<link rel='preconnect' href='https://example.com'>",
            "<link rel='stylesheet' href='missing.css'>",
        ] {
            std::fs::write(root.join("index.html"), html).unwrap();
            assert!(verify(root, None).is_err(), "{html}");
        }
        std::fs::write(
            root.join("index.html"),
            "<link rel='stylesheet' href='style.css'>",
        )
        .unwrap();
        std::fs::write(root.join("style.css"), "@font-face{src:url(font.woff2)}").unwrap();
        assert!(verify(root, None).is_err());
        std::fs::write(root.join("font.woff2"), "local test font").unwrap();
        verify(root, None).unwrap();
        std::fs::write(root.join("style.css"), "div{background:url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'/%3E\")}").unwrap();
        verify(root, None).unwrap();
    }
}
