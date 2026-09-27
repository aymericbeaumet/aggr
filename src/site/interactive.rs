use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use std::sync::OnceLock;
use url::Url;

pub(crate) const METADATA_KEY: &str = "content:interactive";

#[derive(Debug, Clone, Serialize)]
pub struct InteractiveCtx {
    pub source_url: String,
}

impl InteractiveCtx {
    pub fn from_item(item: &crate::model::Item) -> Option<Self> {
        Self::from_url(
            &item.front.link,
            item.front.extra.get(METADATA_KEY)?.as_bool()?,
        )
    }

    fn from_url(link: &str, interactive: bool) -> Option<Self> {
        let url = Url::parse(link).ok()?;
        (interactive
            && matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none())
        .then(|| Self {
            source_url: url.into(),
        })
    }
}

/// Canvas applications have no diagram pixels in their HTTP HTML. Record the capability before
/// stripping executable content; rendering can offer the separately sandboxed live original.
#[cfg(test)]
pub(crate) fn is_interactive_document(page: &str) -> bool {
    mentions_interactive_markup(page) && is_interactive_document_in(&Html::parse_document(page))
}

/// The byte pre-check in front of [`is_interactive_document_in`]: a page with neither a `<canvas`
/// nor a `<figure` has nothing the archive could be missing, and never needs the parsed checks.
pub(crate) fn mentions_interactive_markup(page: &str) -> bool {
    let has = |needle: &[u8]| {
        page.as_bytes()
            .windows(needle.len())
            .any(|tag| tag.eq_ignore_ascii_case(needle))
    };
    has(b"<canvas") || has(b"<figure")
}

/// The parsed half of [`is_interactive_document`], for callers that already hold the document
/// and passed [`mentions_interactive_markup`]. Either shape means the page's own pictures are drawn by a
/// script the archive never runs.
pub(crate) fn is_interactive_document_in(document: &Html) -> bool {
    is_canvas_application(document) || draws_its_figures_with_scripts(document)
}

/// Figures a script mounts: the element names a module in a `data-` attribute and holds only the
/// fallback text the publisher wrote for readers who never run it. One such figure is an aside;
/// a page built out of them has no pictures at all without its scripts.
fn draws_its_figures_with_scripts(document: &Html) -> bool {
    static SELECTORS: OnceLock<Option<(Selector, Selector, Selector)>> = OnceLock::new();
    let Some((figures, media, scripts)) = SELECTORS.get_or_init(|| {
        Some((
            Selector::parse("figure").ok()?,
            Selector::parse("img, picture, video, audio, svg, canvas, object, embed, iframe")
                .ok()?,
            Selector::parse("script").ok()?,
        ))
    }) else {
        return false;
    };
    if !document.select(scripts).any(executable_script) {
        return false;
    }
    document
        .select(figures)
        .filter(|figure| {
            outside_chrome(*figure)
                && figure.select(media).next().is_none()
                && figure
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .any(mounts_a_module)
        })
        .take(2)
        .count()
        == 2
}

/// An element that names a script module to fill itself with.
fn mounts_a_module(element: ElementRef<'_>) -> bool {
    element.value().attrs().any(|(name, value)| {
        name.starts_with("data-")
            && value
                .split(['?', '#'])
                .next()
                .is_some_and(|path| path.ends_with(".js") || path.ends_with(".mjs"))
    })
}

fn is_canvas_application(document: &Html) -> bool {
    static SELECTORS: OnceLock<Option<(Selector, Selector, Selector)>> = OnceLock::new();
    let Some((canvases, scripts, controls)) = SELECTORS.get_or_init(|| {
        Some((
            Selector::parse("canvas").ok()?,
            Selector::parse("script").ok()?,
            Selector::parse("input, select, button, textarea").ok()?,
        ))
    }) else {
        return false;
    };
    if !document.select(canvases).any(content_canvas)
        || !document.select(scripts).any(executable_script)
    {
        return false;
    }
    document
        .select(controls)
        .filter(|element| {
            outside_chrome(*element)
                && element.value().attr("disabled").is_none()
                && !element
                    .value()
                    .attr("type")
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("hidden"))
        })
        .take(3)
        .count()
        == 3
}

fn outside_chrome(element: ElementRef<'_>) -> bool {
    std::iter::once(element)
        .chain(element.ancestors().filter_map(ElementRef::wrap))
        .all(|element| {
            let value = element.value();
            !matches!(
                value.name(),
                "nav" | "header" | "footer" | "template" | "noscript"
            ) && value.attr("hidden").is_none()
                && value.attr("inert").is_none()
                && !value
                    .attr("aria-hidden")
                    .is_some_and(|hidden| hidden.eq_ignore_ascii_case("true"))
        })
}

fn content_canvas(element: ElementRef<'_>) -> bool {
    outside_chrome(element)
        && ["width", "height"].into_iter().all(|attribute| {
            element
                .value()
                .attr(attribute)
                .and_then(|value| value.parse::<u32>().ok())
                .is_none_or(|pixels| pixels > 16)
        })
}

fn executable_script(element: ElementRef<'_>) -> bool {
    outside_chrome(element)
        && element.value().attr("type").is_none_or(|kind| {
            matches!(
                kind.trim().to_ascii_lowercase().as_str(),
                "" | "module" | "text/javascript" | "application/javascript"
            )
        })
        && (element
            .value()
            .attr("src")
            .is_some_and(|src| !src.is_empty())
            || element.text().any(|text| !text.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_a_script_mounts_are_pictures_the_archive_cannot_keep() {
        // brand.io: every figure names a module and carries only the fallback text.
        let page = r#"<html><body><article>
<figure class="interactive"><figcaption>Spymark</figcaption><div class="embed-root" data-embed-src="/article/spymarks/_embeds/1.js"><p class="embed-fallback">“Spymark”</p></div></figure>
<p>Prose between them.</p>
<figure class="interactive"><div class="embed-root" data-embed-src="/article/spymarks/_embeds/2.js"><p class="embed-fallback">Compare the original and marked image.</p></div></figure>
</article><script type="module" src="/assets/site.js"></script></body></html>"#;
        assert!(is_interactive_document(page));

        // One such figure is an aside, not a page built out of them.
        let single = page.replace(
            r#"<figure class="interactive"><div class="embed-root" data-embed-src="/article/spymarks/_embeds/2.js"><p class="embed-fallback">Compare the original and marked image.</p></div></figure>"#,
            "",
        );
        assert!(!is_interactive_document(&single));

        // A figure that carries its own picture is archived as that picture.
        let pictured = page.replace(
            r#"<p class="embed-fallback">“Spymark”</p>"#,
            r#"<img src="/hero.png" alt="Spymark">"#,
        );
        assert!(!is_interactive_document(&pictured));

        // A data attribute that names anything but a module is ordinary markup.
        let ordinary = page.replace(".js", ".json");
        assert!(!is_interactive_document(&ordinary));

        // Without a script to run, the figures are all the page ever had.
        let scriptless = page.replace(
            r#"<script type="module" src="/assets/site.js"></script>"#,
            "",
        );
        assert!(!is_interactive_document(&scriptless));
    }

    const CONTROLS: &str =
        "<label>Shape<select></select></label><input type=range><button>Reset</button>";

    #[test]
    fn interactive_canvas_controls_survive_as_metadata_without_executing_scripts() {
        let html = format!(
            "<body><main><canvas id=surface></canvas></main><aside>{CONTROLS}</aside>\
             <script src='/vendor/three.js'></script><script>throw new Error('never execute');</script></body>"
        );
        assert!(is_interactive_document(&html));
        assert!(is_interactive_document(&html.to_ascii_uppercase()));
        assert!(!is_interactive_document(&html.replace(
            "<canvas id=surface></canvas>",
            "<img src='/diagram.png'>"
        )));
    }

    #[test]
    fn ordinary_articles_tracking_canvases_and_json_scripts_are_not_applications() {
        for html in [
            "<article><p>An article with an incidental chart.</p><canvas></canvas></article><script>chart()</script>".to_owned(),
            format!("<canvas width=1 height=1></canvas>{CONTROLS}<script>track()</script>"),
            format!("<nav><canvas></canvas></nav>{CONTROLS}<script>draw()</script>"),
            format!("<div hidden><canvas></canvas></div>{CONTROLS}<script>draw()</script>"),
            format!("<canvas></canvas><footer>{CONTROLS}</footer><script>draw()</script>"),
            format!("<canvas></canvas>{CONTROLS}<script type='application/ld+json'>{{}}</script>"),
            format!("<canvas></canvas>{CONTROLS}<script></script>"),
        ] {
            assert!(!is_interactive_document(&html), "{html}");
        }
    }

    #[test]
    fn interactive_context_requires_a_marker_and_an_uncredentialed_web_original() {
        let url = "https://example.org/app?shape=torus#plate-2";
        assert_eq!(InteractiveCtx::from_url(url, true).unwrap().source_url, url);
        assert!(InteractiveCtx::from_url(url, false).is_none());
        for url in [
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///tmp/app.html",
            "//example.org/app",
            "https://user:password@example.org/app",
            "https://user@example.org/app",
        ] {
            assert!(InteractiveCtx::from_url(url, true).is_none(), "{url}");
        }
    }
}
