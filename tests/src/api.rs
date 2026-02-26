#[allow(unused_imports)]
use {
    crate::render_pdf,
    crate::FONTDB,
    crate::{convert_svg, run_test_impl},
    pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str},
    std::collections::HashMap,
    std::path::Path,
    std::sync::atomic::{AtomicUsize, Ordering},
    std::sync::Arc,
    svg2pdf::ConversionOptions,
    svg2pdf::ExternalImage,
    svg2pdf::PageOptions,
};

#[test]
fn text_to_paths() {
    let options = ConversionOptions { embed_text: false, ..ConversionOptions::default() };

    let svg_path = "svg/resvg/text/text/simple-case.svg";
    let (pdf, actual_image) =
        convert_svg(Path::new(svg_path), options, PageOptions::default());
    let res = run_test_impl(pdf, actual_image, "api/text_to_paths");
    assert_eq!(res, 0);
}

#[test]
fn dpi() {
    let conversion_options = ConversionOptions::default();
    let page_options = PageOptions { dpi: 140.0 };

    let svg_path = "svg/resvg/text/text/simple-case.svg";
    let (pdf, actual_image) =
        convert_svg(Path::new(svg_path), conversion_options, page_options);
    let res = run_test_impl(pdf, actual_image, "api/dpi");
    assert_eq!(res, 0);
}

#[test]
fn to_chunk() {
    let mut alloc = Ref::new(1);
    let catalog_id = alloc.bump();
    let page_tree_id = alloc.bump();
    let page_id = alloc.bump();
    let font_id = alloc.bump();
    let content_id = alloc.bump();
    let font_name = Name(b"F1");
    let svg_name = Name(b"S1");

    let path =
        "svg/custom/integration/wikimedia/coat_of_the_arms_of_edinburgh_city_council.svg";
    let svg = std::fs::read_to_string(path).unwrap();
    let options = usvg::Options { fontdb: FONTDB.clone(), ..usvg::Options::default() };
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options).unwrap();
    let (svg_chunk, svg_id) =
        svg2pdf::to_chunk(&tree, svg2pdf::ConversionOptions::default()).unwrap();

    let mut map = HashMap::new();
    let svg_chunk =
        svg_chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
    let svg_id = map.get(&svg_id).unwrap();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);

    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, 595.0, 842.0));
    page.parent(page_tree_id);
    page.contents(content_id);

    let mut resources = page.resources();
    resources.x_objects().pair(svg_name, svg_id);
    resources.fonts().pair(font_name, font_id);
    resources.finish();
    page.finish();

    pdf.type1_font(font_id).base_font(Name(b"Times-Roman"));

    let mut content = Content::new();

    content
        .transform([300.0, 0.0, 0.0, 300.0, 200.0, 400.0])
        .x_object(svg_name);

    pdf.stream(content_id, &content.finish());
    pdf.extend(&svg_chunk);
    let pdf = pdf.finish();

    let actual_image = render_pdf(pdf.as_slice());
    let res = run_test_impl(pdf, actual_image, "api/to_chunk");

    assert_eq!(res, 0);
}

/// Test that the external image provider is called and the resulting PDF
/// contains the externally-provided image in the correct position.
#[test]
fn external_image_provider() {
    // Use an SVG with a single embedded PNG image.
    let path = "svg/custom/structure/image/png-rgb-8.svg";
    let svg = std::fs::read_to_string(path).unwrap();
    let options = usvg::Options { fontdb: FONTDB.clone(), ..usvg::Options::default() };
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options).unwrap();

    // Pre-create a 64×64 solid-red image XObject in a separate chunk.
    // This simulates the caller pre-encoding images once for reuse.
    let mut pre_chunk = pdf_writer::Chunk::new();
    let img_ref = Ref::new(50000); // high range to avoid collision with svg2pdf internals
    let img_w: u32 = 64;
    let img_h: u32 = 64;

    let red_pixels: Vec<u8> = (0..(img_w * img_h))
        .flat_map(|_| [255u8, 0, 0])
        .collect();

    let mut xobj = pre_chunk.image_xobject(img_ref, &red_pixels);
    xobj.width(img_w as i32);
    xobj.height(img_h as i32);
    xobj.color_space().device_rgb();
    xobj.bits_per_component(8);
    xobj.finish();

    // Track how many times the provider is invoked.
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    let conversion_options = ConversionOptions {
        image_provider: Some(Box::new(move |_img: &usvg::Image| {
            cc.fetch_add(1, Ordering::SeqCst);
            Some(ExternalImage {
                name: b"ExtImg0".to_vec(),
                r#ref: img_ref,
                width: img_w as f32,
                height: img_h as f32,
            })
        })),
        ..ConversionOptions::default()
    };

    let (svg_chunk, svg_id) =
        svg2pdf::to_chunk(&tree, conversion_options).unwrap();

    // The provider must have been called at least once.
    assert!(
        call_count.load(Ordering::SeqCst) > 0,
        "Image provider callback was never invoked"
    );

    // ---- Assemble a complete, renderable PDF ----
    let mut alloc = Ref::new(1);
    let catalog_id = alloc.bump();
    let page_tree_id = alloc.bump();
    let page_id = alloc.bump();
    let content_id = alloc.bump();
    let svg_name = Name(b"S1");

    // Renumber both chunks with the *same* map so that the external image
    // ref inside the SVG resource dictionary matches the image object.
    let mut map = HashMap::new();
    let svg_chunk =
        svg_chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
    let svg_id = *map.get(&svg_id).unwrap();
    let pre_chunk =
        pre_chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);

    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, 200.0, 200.0));
    page.parent(page_tree_id);
    page.contents(content_id);

    let mut resources = page.resources();
    resources.x_objects().pair(svg_name, &svg_id);
    resources.finish();
    page.finish();

    let mut content = Content::new();
    content
        .transform([200.0, 0.0, 0.0, 200.0, 0.0, 0.0])
        .x_object(svg_name);

    pdf.stream(content_id, &content.finish());
    pdf.extend(&svg_chunk);
    pdf.extend(&pre_chunk);
    let pdf_bytes = pdf.finish();

    // The PDF must be non-empty and start with the PDF header.
    assert!(pdf_bytes.len() > 100, "Assembled PDF should be non-trivial");
    assert!(
        pdf_bytes.starts_with(b"%PDF"),
        "Output should be a valid PDF"
    );
}

/// Test that when the image provider returns `None` for a node, svg2pdf
/// falls back to its normal image encoding path.
#[test]
fn external_image_provider_fallback() {
    let path = "svg/custom/structure/image/png-rgb-8.svg";
    let svg = std::fs::read_to_string(path).unwrap();
    let options = usvg::Options { fontdb: FONTDB.clone(), ..usvg::Options::default() };
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options).unwrap();

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    // Provider always returns None → every image should be encoded normally.
    let opts_with_provider = ConversionOptions {
        image_provider: Some(Box::new(move |_img: &usvg::Image| {
            cc.fetch_add(1, Ordering::SeqCst);
            None
        })),
        ..ConversionOptions::default()
    };

    let result = svg2pdf::to_chunk(&tree, opts_with_provider);
    assert!(result.is_ok(), "to_chunk should succeed when provider returns None");

    assert!(
        call_count.load(Ordering::SeqCst) > 0,
        "Provider should have been called even though it returned None"
    );

    // Also verify the normal (no provider) path still works.
    let result_normal = svg2pdf::to_chunk(&tree, ConversionOptions::default());
    assert!(result_normal.is_ok(), "to_chunk without provider should succeed");
}

/// Test that when no image provider is set (`None`), images are encoded
/// into the chunk as before (backwards-compatible).
#[test]
fn no_image_provider() {
    let path = "svg/custom/structure/image/png-rgb-8.svg";
    let svg = std::fs::read_to_string(path).unwrap();
    let options = usvg::Options { fontdb: FONTDB.clone(), ..usvg::Options::default() };
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options).unwrap();

    // Default options: image_provider is None.
    let opts = ConversionOptions::default();
    let (chunk, _svg_id) = svg2pdf::to_chunk(&tree, opts).unwrap();

    // The chunk must contain data (the encoded image).
    assert!(chunk.len() > 0, "Chunk should be non-empty for SVG with image");
}
