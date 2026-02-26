use pdf_writer::{Chunk, Content, Filter, Finish, Name, Ref};
use usvg::{Node, Size, Transform, Tree};

use crate::util::context::Context;
use crate::util::helper::{ContentExt, RectExt, TransformExt};
use crate::util::resources::ResourceContainer;
use crate::{ExternalImage, Result};

pub mod clip_path;
#[cfg(feature = "filters")]
pub mod filter;
pub mod gradient;
pub mod group;
#[cfg(feature = "image")]
pub mod image;
pub mod mask;
pub mod path;
pub mod pattern;
#[cfg(feature = "text")]
pub mod text;

/// Write a tree into a stream. Assumes that the stream belongs to transparency group and the object
/// that contains it has the correct bounding box set.
pub fn tree_to_stream(
    tree: &Tree,
    chunk: &mut Chunk,
    content: &mut Content,
    ctx: &mut Context,
    rc: &mut ResourceContainer,
) -> Result<()> {
    content.save_state_checked()?;

    // From PDF coordinate system to SVG coordinate system
    let initial_transform =
        Transform::from_row(1.0, 0.0, 0.0, -1.0, 0.0, tree.size().height());

    content.transform(initial_transform.to_pdf_transform());

    group::render(tree.root(), chunk, content, ctx, initial_transform, None, rc)?;
    content.restore_state();

    Ok(())
}

/// Convert a tree into a XObject of size 1x1, similar to an image.
pub fn tree_to_xobject(tree: &Tree, chunk: &mut Chunk, ctx: &mut Context) -> Result<Ref> {
    let bbox = tree.size().to_non_zero_rect(0.0, 0.0);
    let x_ref = ctx.alloc_ref();

    let mut rc = ResourceContainer::new();

    let mut content = Content::new();
    tree_to_stream(tree, chunk, &mut content, ctx, &mut rc)?;
    let stream = ctx.finish_content(content);

    let mut x_object = chunk.form_xobject(x_ref, &stream);
    x_object.bbox(bbox.to_pdf_rect());
    x_object.matrix([1.0 / bbox.width(), 0.0, 0.0, 1.0 / bbox.height(), 0.0, 0.0]);

    if ctx.options.compress {
        x_object.filter(Filter::FlateDecode);
    }

    let mut resources = x_object.resources();
    rc.finish(&mut resources);

    resources.finish();
    x_object.finish();

    Ok(x_ref)
}

/// Render an externally-provided image into the content stream.
///
/// This emits the same coordinate transforms that the normal image rendering
/// path would, but references an external XObject name via the `Do` operator
/// instead of encoding the image data into the chunk.
fn render_external_image(
    image: &usvg::Image,
    ext: ExternalImage,
    content: &mut Content,
    rc: &mut ResourceContainer,
) -> Result<()> {
    if !image.is_visible() {
        return Ok(());
    }

    let image_size = Size::from_wh(ext.width, ext.height)
        .ok_or(crate::ConversionError::InvalidImage)?;

    // Register the external XObject in the resource dictionary using the
    // caller-provided name and reference.
    let name_str = String::from_utf8(ext.name)
        .map_err(|_| crate::ConversionError::InvalidImage)?;
    rc.add_external_x_object(name_str.clone(), ext.r#ref);

    content.save_state_checked()?;

    // Scale the image from the default 1×1 XObject size to the actual
    // image dimensions, with a vertical flip (PDF y-up → SVG y-down).
    content.transform(
        Transform::from_row(
            image_size.width(),
            0.0,
            0.0,
            -image_size.height(),
            0.0,
            image_size.height(),
        )
        .to_pdf_transform(),
    );
    content.x_object(Name(name_str.as_bytes()));
    content.restore_state();

    Ok(())
}

trait Render {
    fn render(
        &self,
        chunk: &mut Chunk,
        content: &mut Content,
        ctx: &mut Context,
        accumulated_transform: Transform,
        rc: &mut ResourceContainer,
    ) -> Result<()>;
}

impl Render for Node {
    fn render(
        &self,
        chunk: &mut Chunk,
        content: &mut Content,
        ctx: &mut Context,
        accumulated_transform: Transform,
        rc: &mut ResourceContainer,
    ) -> Result<()> {
        match self {
            Node::Path(ref path) => {
                path::render(path, chunk, content, ctx, rc, accumulated_transform)
            }
            Node::Group(ref group) => {
                group::render(group, chunk, content, ctx, accumulated_transform, None, rc)
            }
            Node::Image(ref image) => {
                // Check the external image provider first.
                if let Some(ext) =
                    ctx.options.image_provider.as_ref().and_then(|p| p(image))
                {
                    return render_external_image(image, ext, content, rc);
                }

                #[cfg(feature = "image")]
                {
                    image::render(
                        image.is_visible(),
                        image.kind(),
                        None,
                        chunk,
                        content,
                        ctx,
                        rc,
                    )
                }

                #[cfg(not(feature = "image"))]
                {
                    log::warn!("Failed convert image because the image feature was disabled. Skipping.");
                    Ok(())
                }
            }
            #[cfg(feature = "text")]
            Node::Text(ref text) => {
                if ctx.options.embed_text {
                    text::render(text, chunk, content, ctx, rc, accumulated_transform)
                } else {
                    group::render(
                        text.flattened(),
                        chunk,
                        content,
                        ctx,
                        accumulated_transform,
                        None,
                        rc,
                    )
                }
            }
            #[cfg(not(feature = "text"))]
            Node::Text(_) => {
                log::warn!("Failed convert text because the text feature was disabled. Skipping.");
                Ok(())
            }
        }
    }
}
