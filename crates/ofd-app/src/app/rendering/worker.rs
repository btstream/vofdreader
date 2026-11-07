//! Turning background raster output into texture-ready images.
//!
//! The GPUI closures should only claim a ticket, hand work to the background
//! executor, and deliver the result. Pixel rotation, buffer validation and
//! `RenderImage` construction are identical for every job kind, so they live
//! here instead of being copied into each closure.

use std::sync::Arc;

use gpui_kit::RenderImage;

/// Validate a rasterized buffer, rotate it into view space and wrap it for
/// the window. `expected` is the size the rotated output must have; tile jobs
/// know it in advance, whole-page jobs do not.
pub(crate) fn image_from_raster(
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    rotation: u8,
    expected: Option<(u32, u32)>,
    error: &'static str,
) -> Result<(Arc<RenderImage>, usize), &'static str> {
    let (width, height, pixels) =
        crate::rotate::owned(width, height, bgra, rotation).ok_or(error)?;
    if expected.is_some_and(|expected| expected != (width, height)) {
        return Err(error);
    }
    let bytes = width as usize * height as usize * 4;
    let buffer = image::RgbaImage::from_raw(width, height, pixels).ok_or(error)?;
    Ok((
        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
        bytes,
    ))
}

/// Convenience for whole-page jobs, whose rotated size is whatever the
/// renderer produced.
pub(crate) fn page_image_from_raster(
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    rotation: u8,
) -> Result<(Arc<RenderImage>, usize), &'static str> {
    image_from_raster(
        width,
        height,
        bgra,
        rotation,
        None,
        "页面图像缓冲区尺寸无效",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_buffer_keeps_its_pixel_count() {
        // 2x1 red/blue pixels: rotation only moves bytes around.
        let mut bgra = Vec::new();
        bgra.extend_from_slice(&[0, 0, 255, 255]);
        bgra.extend_from_slice(&[255, 0, 0, 255]);

        let (image, bytes) = page_image_from_raster(2, 1, bgra.clone(), 0).unwrap();
        assert_eq!(bytes, 8);
        assert_eq!(image.size(0).width.0, 2);
        assert_eq!(image.size(0).height.0, 1);

        let (rotated, bytes) = page_image_from_raster(2, 1, bgra, 1).unwrap();
        assert_eq!(bytes, 8);
        assert_eq!(rotated.size(0).width.0, 1);
        assert_eq!(rotated.size(0).height.0, 2);
    }

    #[test]
    fn mismatched_expected_size_is_rejected() {
        let bgra = vec![0u8; 8];
        assert!(image_from_raster(2, 1, bgra.clone(), 0, Some((2, 1)), "bad").is_ok());
        assert!(image_from_raster(2, 1, bgra, 0, Some((1, 1)), "bad").is_err());
    }

    #[test]
    fn a_buffer_that_does_not_match_its_size_is_rejected() {
        let short = vec![0u8; 3];
        assert!(page_image_from_raster(2, 1, short, 0).is_err());
    }
}
