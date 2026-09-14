use anyhow::{Context, Result, anyhow, bail};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{Pdf, object::Object};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use image::{DynamicImage, RgbaImage};

const MAX_PAGES: usize = 1024;
const MAX_OBJECTS: usize = 20_000;
const MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_OPERATIONS: usize = 100_000;

pub(super) fn thumbnail(bytes: &[u8], alt: Option<String>) -> Result<super::Thumbnail> {
    if bytes.len() > super::MAX_INPUT_BYTES || !bytes.starts_with(b"%PDF-") {
        bail!("invalid PDF or PDF exceeds preview input limit");
    }
    let document = Pdf::new(bytes.to_vec()).map_err(|error| anyhow!("reading PDF: {error:?}"))?;
    if document.len() > MAX_OBJECTS || document.pages().len() > MAX_PAGES {
        bail!("PDF exceeds preview document limits");
    }
    for object in document.objects() {
        if let Object::Stream(stream) = object
            && let (Some(width), Some(height)) = (
                stream.dict().get::<u32>(b"Width"),
                stream.dict().get::<u32>(b"Height"),
            )
            && (width > super::MAX_AXIS
                || height > super::MAX_AXIS
                || u64::from(width) * u64::from(height) > super::MAX_PIXELS)
        {
            bail!("PDF embedded image exceeds preview pixel limits");
        }
    }
    let page = document.pages().first().context("PDF contains no pages")?;
    let (page_width, page_height) = page.render_dimensions();
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
        || page_width > super::MAX_AXIS as f32
        || page_height > super::MAX_AXIS as f32
    {
        bail!("PDF page exceeds preview dimension limits");
    }
    if page
        .page_stream()
        .is_some_and(|stream| stream.len() > MAX_CONTENT_BYTES)
    {
        bail!("PDF page exceeds preview content limits");
    }
    let mut operations = page.operations();
    if (0..=MAX_OPERATIONS).all(|_| operations.next().is_some()) {
        bail!("PDF page exceeds preview operation limit");
    }
    let scale = 256.0 / page_width.max(page_height);
    let width = (page_width * scale).round() as u16;
    let height = (page_height * scale).round() as u16;
    if width < 2 || height < 2 {
        bail!("PDF page is too narrow for a preview");
    }
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: Some(width),
        height: Some(height),
        bg_color: WHITE,
    };
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings {
            render_annotations: false,
            ..InterpreterSettings::default()
        },
        &settings,
    );
    let image = RgbaImage::from_raw(
        u32::from(width),
        u32::from(height),
        pixmap.data_as_u8_slice().to_vec(),
    )
    .context("PDF renderer returned an invalid bitmap")?;
    let image = DynamicImage::ImageRgba8(image);
    let rgb = image.to_rgb8();
    let mut output = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut output)
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .context("encoding PDF preview")?;
    if output.len() > super::MAX_BYTES {
        bail!("encoded PDF preview exceeds size limit");
    }
    Ok(super::Thumbnail {
        bytes: output,
        extension: "webp",
        width: rgb.width(),
        height: rgb.height(),
        alt: super::clean_alt(alt.as_deref()),
        color: super::dominant_color(&image),
    })
}

#[cfg(test)]
pub(super) fn test_pdf(width: u32, height: u32) -> Vec<u8> {
    test_document(width, height, false)
}

#[cfg(test)]
fn test_document(width: u32, height: u32, second_page: bool) -> Vec<u8> {
    let stream = format!(
        "1 0 0 rg 0 0 {} {} re f\nBT /F1 24 Tf 10 30 Td (PDF) Tj ET\n",
        width / 2,
        height / 2
    );
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        if second_page {
            "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>"
        } else {
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"
        }
        .to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
        ),
        format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    if second_page {
        let blue = format!("0 0 1 rg 0 0 {width} {height} re f\n");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 7 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{blue}endstream",
            blue.len()
        ));
    }
    let mut document = "%PDF-1.4\n".to_string();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(document.len());
        document.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = document.len();
    document.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
    for offset in offsets.iter().skip(1) {
        document.push_str(&format!("{offset:010} 00000 n \n"));
    }
    document.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len()
    ));
    document.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_pdf_first_page_as_a_bounded_thumbnail() {
        let preview = thumbnail(
            &test_document(200, 400, true),
            Some(" Paper \n preview ".into()),
        )
        .unwrap();
        assert_eq!((preview.width, preview.height), (128, 256));
        assert_eq!(preview.alt.as_deref(), Some("Paper preview"));
        assert!(preview.bytes.len() < super::super::MAX_BYTES);
        let image = image::load_from_memory(&preview.bytes).unwrap().to_rgb8();
        assert!(
            image
                .pixels()
                .any(|pixel| pixel[0] > 220 && pixel[1] < 30 && pixel[2] < 30)
        );
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0.iter().all(|channel| *channel > 240))
        );
        assert!(
            !image
                .pixels()
                .any(|pixel| pixel[2] > 220 && pixel[0] < 30 && pixel[1] < 30)
        );
    }

    #[test]
    fn pdf_preview_preserves_landscape_ratio() {
        let preview = thumbnail(&test_pdf(600, 300), None).unwrap();
        assert_eq!((preview.width, preview.height), (256, 128));
    }

    #[test]
    fn invalid_and_oversized_pdf_previews_fail_without_an_image() {
        for bytes in [
            b"not a PDF".to_vec(),
            b"%PDF-1.7\ntruncated".to_vec(),
            vec![b' '; super::super::MAX_INPUT_BYTES + 1],
            test_pdf(1_000_000, 1_000_000),
        ] {
            assert!(thumbnail(&bytes, None).is_err());
        }
    }

    #[test]
    fn unsupported_encryption_is_a_preview_error() {
        let document = String::from_utf8(test_pdf(200, 400)).unwrap().replace(
            "/Root 1 0 R >>",
            "/Root 1 0 R /Encrypt << /Filter /Standard /V 99 >> >>",
        );
        assert!(thumbnail(document.as_bytes(), None).is_err());
    }
}
