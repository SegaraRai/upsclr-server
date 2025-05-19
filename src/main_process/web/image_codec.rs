use axum::{http::header, response::{IntoResponse, Response}};
use tracing::debug;

use super::error::ApiError;

/// Color format for image encoding/decoding
#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Png { compression: u8 },
    Jpeg { quality: u8 },
    Bmp,
    Tga,
    Qoi,
}

impl TryFrom<&mime::Mime> for OutputFormat {
    type Error = ();

    fn try_from(value: &mime::Mime) -> Result<Self, Self::Error> {
        if value.type_() == mime::IMAGE {
            match value.subtype().as_str() {
                "png" => {
                    let compression = value
                        .get_param("compression")
                        .and_then(|c| c.as_str().parse::<u8>().ok())
                        .unwrap_or(6);
                    Ok(OutputFormat::Png { compression })
                }
                "jpeg" => {
                    let quality = value
                        .get_param("quality")
                        .and_then(|q| q.as_str().parse::<u8>().ok())
                        .unwrap_or(85);
                    Ok(OutputFormat::Jpeg { quality })
                }
                "bmp" | "x-bmp" => Ok(OutputFormat::Bmp),
                "x-tga" | "x-targa" => Ok(OutputFormat::Tga),
                "x-qoi" => Ok(OutputFormat::Qoi),
                _ => Err(()),
            }
        } else {
            Err(())
        }
    }
}

// Convert OutputFormat to the ColorFormat used by plugin API
impl From<OutputFormat> for crate::models::ColorFormat {
    fn from(_value: OutputFormat) -> Self {
        crate::models::ColorFormat::Rgb
    }
}

// Helper function to decode various input image formats
pub fn decode_input_image(
    file_data: &[u8],
    content_type_str: Option<&str>,
) -> Result<(Vec<u8>, u32, u32, u32, crate::models::ColorFormat), ApiError> {
    let media_type = content_type_str.map(|s| s[0..s.find(';').unwrap_or(s.len())].trim());

    let img_format_hint = match media_type {
        Some("image/jpeg") => Some(image::ImageFormat::Jpeg),
        Some("image/png") => Some(image::ImageFormat::Png),
        Some("image/webp") => Some(image::ImageFormat::WebP),
        Some("image/qoi") => Some(image::ImageFormat::Qoi),
        Some("image/x-qoi") => Some(image::ImageFormat::Qoi),
        Some("image/bmp") => Some(image::ImageFormat::Bmp),
        Some("image/x-bmp") => Some(image::ImageFormat::Bmp),
        Some("image/vnd.ms-dds") => Some(image::ImageFormat::Dds),
        Some("image/x-tga") => Some(image::ImageFormat::Tga),
        Some("image/x-targa") => Some(image::ImageFormat::Tga),
        _ => None,
    };

    match (media_type, img_format_hint) {
        // If an image format is detected from Content-Type or Content-Type is not provided
        (_, Some(_)) | (None, _) => {
            let dyn_img = if let Some(format) = img_format_hint {
                image::load_from_memory_with_format(file_data, format).map_err(|e| {
                    ApiError::ImageProcessingError(format!(
                        "Failed to decode image (format: {:?}): {}",
                        format, e
                    ))
                })?
            } else {
                image::load_from_memory(file_data).map_err(|e| {
                    ApiError::ImageProcessingError(format!(
                        "Failed to auto-detect and decode image: {}",
                        e
                    ))
                })?
            };

            let width = dyn_img.width();
            let height = dyn_img.height();
            let channels = if dyn_img.color().has_alpha() { 4 } else { 3 };

            let data_vec = if channels == 3 {
                dyn_img.to_rgb8().into_raw()
            } else {
                dyn_img.to_rgba8().into_raw()
            };

            Ok((
                data_vec,
                width,
                height,
                channels as u32,
                crate::models::ColorFormat::Rgb,
            ))
        }
        // If Content-Type is provided but not supported
        (Some(unknown_content_type), _) => Err(ApiError::UnsupportedMediaType(format!(
            "Content type '{}' is not supported.",
            unknown_content_type
        ))),
    }
}

// Helper function to encode output image in various formats
pub fn encode_output_image(
    raw_pixel_data: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    output_format: OutputFormat,
) -> Result<Response, ApiError> {
    use image::ImageFormat;
    use std::io::Cursor;

    let channels_u8 = channels as u8;

    match output_format {
        OutputFormat::Png { compression: _ } => {
            debug!("Encoding output as PNG.");

            let color_type_for_image_crate = match channels_u8 {
                3 => image::ColorType::Rgb8,
                4 => image::ColorType::Rgba8,
                _ => {
                    return Err(ApiError::ImageProcessingError(format!(
                        "Unsupported channel count ({}).",
                        channels
                    )));
                }
            };

            let mut buffer = Cursor::new(Vec::new());
            image::write_buffer_with_format(
                &mut buffer,
                raw_pixel_data,
                width,
                height,
                color_type_for_image_crate,
                ImageFormat::Png,
            )
            .map_err(|e| ApiError::ImageProcessingError(format!("PNG encoding failed: {}", e)))?;

            Ok((
                [(header::CONTENT_TYPE, "image/png")],
                buffer.into_inner(), // Bytes of the encoded PNG.
            )
                .into_response())
        }
        OutputFormat::Jpeg { quality } => {
            debug!("Encoding output as JPEG.");

            if channels_u8 == 4 {
                // JPEG typically doesn't support an alpha channel.
                return Err(ApiError::ImageProcessingError(
                       "JPEG output for 4-channel data (RGBA/BGRA) is not directly supported. Request 3-channel or PNG/RAW.".to_string()
                   ));
            }

            let mut buffer = Cursor::new(Vec::new());
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, quality);
            encoder
                .encode(
                    raw_pixel_data,
                    width,
                    height,
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| {
                    ApiError::ImageProcessingError(format!("JPEG encoding failed: {}", e))
                })?;
            Ok(([(header::CONTENT_TYPE, "image/jpeg")], buffer.into_inner()).into_response())
        }
        OutputFormat::Bmp | OutputFormat::Tga | OutputFormat::Qoi => {
            let (name, mime_type, format) = match output_format {
                OutputFormat::Bmp => ("BMP", "image/bmp", ImageFormat::Bmp),
                OutputFormat::Tga => ("TGA", "image/x-tga", ImageFormat::Tga),
                OutputFormat::Qoi => ("QOI", "image/x-qoi", ImageFormat::Qoi),
                _ => unreachable!(), // All cases handled above
            };

            debug!("Encoding output as {}.", name);

            let color_type_for_image_crate = match channels_u8 {
                3 => image::ColorType::Rgb8,
                4 => image::ColorType::Rgba8,
                _ => {
                    return Err(ApiError::ImageProcessingError(format!(
                        "Unsupported channel count ({}).",
                        channels
                    )));
                }
            };

            let mut buffer = Cursor::new(Vec::new());
            image::write_buffer_with_format(
                &mut buffer,
                raw_pixel_data,
                width,
                height,
                color_type_for_image_crate,
                format,
            )
            .map_err(|e| {
                ApiError::ImageProcessingError(format!("{} encoding failed: {}", name, e))
            })?;

            Ok((
                [(header::CONTENT_TYPE, mime_type)],
                buffer.into_inner(), // Bytes of the encoded image.
            )
                .into_response())
        }
    }
}
