//! Passive SVG rasterization. Publisher markup and external resources never reach the browser.

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, ensure};
use image::ImageEncoder as _;
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

pub(super) fn rasterize(bytes: &[u8], limits: &super::MediaLimits) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= MAX_INPUT_BYTES.min(limits.max_file_bytes),
        "SVG input exceeds limit"
    );
    let source = std::str::from_utf8(bytes).context("SVG is not UTF-8")?;
    let document = usvg::roxmltree::Document::parse_with_options(
        source,
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
