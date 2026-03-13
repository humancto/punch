//! # Image Generation — forging visual strikes from text commands.
//!
//! This module provides types and traits for generating images from prompt descriptions,
//! allowing fighters to conjure visual attacks on demand.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PunchResult;

/// Style presets for image generation — the fighting stance of the visual output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageStyle {
    /// Realistic, natural-looking imagery.
    Natural,
    /// High-contrast, saturated, dramatic imagery.
    Vivid,
    /// Anime/manga-inspired art style.
    Anime,
    /// Photo-realistic rendering.
    Photographic,
    /// Digital art style with clean lines.
    DigitalArt,
    /// Comic book panels and halftone dots.
    ComicBook,
}

/// Output format for generated images — the weapon's material form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// PNG format (lossless).
    Png,
    /// JPEG format (lossy, smaller size).
    Jpeg,
    /// WebP format (modern, efficient).
    Webp,
}

/// A request to generate an image — the battle orders for visual creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenRequest {
    /// The prompt describing the desired image.
    pub prompt: String,
    /// Width in pixels (default: 1024).
    #[serde(default = "default_dimension")]
    pub width: u32,
    /// Height in pixels (default: 1024).
    #[serde(default = "default_dimension")]
    pub height: u32,
    /// Specific model to use for generation.
    pub model: Option<String>,
    /// Visual style preset.
    pub style: Option<ImageStyle>,
    /// Negative prompt — what to avoid in the image.
    pub negative_prompt: Option<String>,
    /// Seed for reproducible generation.
    pub seed: Option<u64>,
}

fn default_dimension() -> u32 {
    1024
}

impl ImageGenRequest {
    /// Create a new image generation request with default dimensions.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            width: 1024,
            height: 1024,
            model: None,
            style: None,
            negative_prompt: None,
            seed: None,
        }
    }

    /// Set the image dimensions.
    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the style preset.
    pub fn with_style(mut self, style: ImageStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Set the model to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the negative prompt.
    pub fn with_negative_prompt(mut self, negative_prompt: impl Into<String>) -> Self {
        self.negative_prompt = Some(negative_prompt.into());
        self
    }

    /// Set the seed for reproducible results.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// The result of an image generation — the visual strike delivered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenResult {
    /// Base64-encoded image data.
    pub image_data: String,
    /// Format of the generated image.
    pub format: ImageFormat,
    /// The prompt as revised/interpreted by the model.
    pub revised_prompt: Option<String>,
    /// Which model produced this image.
    pub model_used: String,
    /// Time taken to generate in milliseconds.
    pub generation_ms: u64,
}

/// Trait for image generation backends — the forge that creates visual weapons.
#[async_trait]
pub trait ImageGenerator: Send + Sync {
    /// Generate an image from the given request.
    async fn generate(&self, request: ImageGenRequest) -> PunchResult<ImageGenResult>;

    /// Return the list of models supported by this generator.
    fn supported_models(&self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_creation() {
        let req = ImageGenRequest::new("a fierce warrior")
            .with_style(ImageStyle::Vivid)
            .with_model("dall-e-3")
            .with_negative_prompt("blurry")
            .with_seed(42);

        assert_eq!(req.prompt, "a fierce warrior");
        assert_eq!(req.style, Some(ImageStyle::Vivid));
        assert_eq!(req.model, Some("dall-e-3".to_string()));
        assert_eq!(req.negative_prompt, Some("blurry".to_string()));
        assert_eq!(req.seed, Some(42));
    }

    #[test]
    fn test_style_serialization() {
        let style = ImageStyle::DigitalArt;
        let json = serde_json::to_string(&style).expect("serialize style");
        assert_eq!(json, "\"digital_art\"");

        let deserialized: ImageStyle = serde_json::from_str(&json).expect("deserialize style");
        assert_eq!(deserialized, ImageStyle::DigitalArt);
    }

    #[test]
    fn test_result_construction() {
        let result = ImageGenResult {
            image_data: "iVBORw0KGgo=".to_string(),
            format: ImageFormat::Png,
            revised_prompt: Some("a fierce warrior in battle".to_string()),
            model_used: "dall-e-3".to_string(),
            generation_ms: 2500,
        };

        assert_eq!(result.model_used, "dall-e-3");
        assert_eq!(result.generation_ms, 2500);
        assert!(result.revised_prompt.is_some());
    }

    #[test]
    fn test_default_dimensions() {
        let req = ImageGenRequest::new("test");
        assert_eq!(req.width, 1024);
        assert_eq!(req.height, 1024);

        let req = req.with_dimensions(512, 768);
        assert_eq!(req.width, 512);
        assert_eq!(req.height, 768);
    }

    #[test]
    fn test_format_variants() {
        let formats = vec![ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp];
        for fmt in &formats {
            let json = serde_json::to_string(fmt).expect("serialize format");
            let deserialized: ImageFormat =
                serde_json::from_str(&json).expect("deserialize format");
            assert_eq!(&deserialized, fmt);
        }

        assert_eq!(
            serde_json::to_string(&ImageFormat::Png).expect("png"),
            "\"png\""
        );
        assert_eq!(
            serde_json::to_string(&ImageFormat::Jpeg).expect("jpeg"),
            "\"jpeg\""
        );
        assert_eq!(
            serde_json::to_string(&ImageFormat::Webp).expect("webp"),
            "\"webp\""
        );
    }
}
