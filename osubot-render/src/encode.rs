use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
use std::io::Cursor;

use crate::error::RenderError;

pub async fn encode_jpeg(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, RenderError> {
    tokio::task::spawn_blocking(move || encode_jpeg_sync(&rgba, width, height, quality))
        .await
        .map_err(|e| RenderError::Encode(e.to_string()))?
}

fn encode_jpeg_sync(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, RenderError> {
    let pixel_count = (width * height) as usize;

    if rgba.len() < pixel_count * 4 {
        return Err(RenderError::Encode(format!(
            "buffer too small: expected at least {} bytes, got {}",
            pixel_count * 4,
            rgba.len()
        )));
    }

    let mut rgb_data = Vec::with_capacity(pixel_count * 3);
    for chunk in rgba[..pixel_count * 4].chunks_exact(4) {
        rgb_data.extend_from_slice(&chunk[..3]);
    }
    let rgb = RgbImage::from_raw(width, height, rgb_data)
        .ok_or_else(|| RenderError::Encode("invalid buffer dimensions".into()))?;

    let mut buf = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .encode_image(&rgb)
        .map_err(|e| RenderError::Encode(e.to_string()))?;
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_encode_jpeg_simple() {
        let rgba: Vec<u8> = [255u8, 0u8, 0u8, 255u8].repeat(10 * 10);
        let result = encode_jpeg(rgba, 10, 10, 80).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() > 100);
    }

    #[tokio::test]
    async fn test_encode_jpeg_large() {
        let rgba: Vec<u8> = [0u8, 255u8, 0u8, 255u8].repeat(20 * 20);
        let result = encode_jpeg(rgba, 20, 20, 90).await;
        assert!(result.is_ok());
        let jpeg = result.unwrap();
        assert!(!jpeg.is_empty());
    }

    #[tokio::test]
    async fn test_encode_jpeg_invalid_dims() {
        let rgba = vec![0u8; 10];
        let result = encode_jpeg(rgba, 100, 100, 80).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_encode_jpeg_rejects_small_buffer() {
        let rgba = vec![0u8; 99];
        let result = encode_jpeg(rgba, 10, 10, 80).await;
        assert!(result.is_err());
    }
}
