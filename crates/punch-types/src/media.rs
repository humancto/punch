//! # Media Understanding — analyzing the battlefield's sights and sounds.
//!
//! This module provides types and traits for analyzing media inputs such as
//! images, audio, video, and documents, extracting intelligence from the field.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PunchResult;

/// MIME type classifications for image media — the visual arsenal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageMimeType {
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// GIF (possibly animated).
    Gif,
    /// WebP image.
    Webp,
    /// SVG vector image.
    Svg,
}

/// MIME type classifications for audio media — the sonic weapons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioMimeType {
    /// MP3 audio.
    Mp3,
    /// WAV audio.
    Wav,
    /// OGG Vorbis audio.
    Ogg,
    /// FLAC lossless audio.
    Flac,
}

/// The type of media being analyzed — identifying the weapon class.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    /// Image with specific MIME type.
    Image(ImageMimeType),
    /// Audio with specific MIME type.
    Audio(AudioMimeType),
    /// Video content.
    Video,
    /// PDF document.
    Pdf,
    /// Other document types.
    Document,
}

/// Input media for analysis — the raw intelligence to process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInput {
    /// Base64-encoded data or a URL pointing to the media.
    pub data: String,
    /// The type of media.
    pub media_type: MediaType,
    /// Source filename or URL (for reference).
    pub source: Option<String>,
}

impl MediaInput {
    /// Create a new media input from base64 data.
    pub fn from_base64(data: impl Into<String>, media_type: MediaType) -> Self {
        Self {
            data: data.into(),
            media_type,
            source: None,
        }
    }

    /// Create a new media input from a URL.
    pub fn from_url(url: impl Into<String>, media_type: MediaType) -> Self {
        let url = url.into();
        Self {
            data: url.clone(),
            media_type,
            source: Some(url),
        }
    }

    /// Set the source filename or URL.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// The result of media analysis — battlefield intelligence extracted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAnalysis {
    /// Human-readable description of the media content.
    pub description: String,
    /// Any text extracted from the media (OCR, transcription, etc.).
    pub extracted_text: Option<String>,
    /// Additional metadata as a JSON value.
    pub metadata: serde_json::Value,
    /// Classification tags for the media.
    pub tags: Vec<String>,
    /// Confidence score for the analysis (0.0 to 1.0).
    pub confidence: f64,
}

impl MediaAnalysis {
    /// Create a new media analysis result.
    pub fn new(description: impl Into<String>, confidence: f64) -> Self {
        Self {
            description: description.into(),
            extracted_text: None,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            tags: Vec::new(),
            confidence,
        }
    }

    /// Set extracted text.
    pub fn with_extracted_text(mut self, text: impl Into<String>) -> Self {
        self.extracted_text = Some(text.into());
        self
    }

    /// Add tags.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Trait for media analysis backends — the intelligence unit that deciphers captured assets.
#[async_trait]
pub trait MediaAnalyzer: Send + Sync {
    /// Analyze the given media input and produce an analysis.
    async fn analyze(&self, input: MediaInput) -> PunchResult<MediaAnalysis>;

    /// Return the media types this analyzer supports.
    fn supported_types(&self) -> Vec<MediaType>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_type_detection() {
        let image_type = MediaType::Image(ImageMimeType::Png);
        let audio_type = MediaType::Audio(AudioMimeType::Mp3);

        let img_json = serde_json::to_string(&image_type).expect("serialize image type");
        let aud_json = serde_json::to_string(&audio_type).expect("serialize audio type");

        let img_deser: MediaType = serde_json::from_str(&img_json).expect("deserialize image type");
        let aud_deser: MediaType = serde_json::from_str(&aud_json).expect("deserialize audio type");

        assert_eq!(img_deser, image_type);
        assert_eq!(aud_deser, audio_type);
    }

    #[test]
    fn test_analysis_construction() {
        let analysis = MediaAnalysis::new("A photo of a boxing ring", 0.95)
            .with_extracted_text("Round 1")
            .with_tags(vec!["sports".to_string(), "boxing".to_string()])
            .with_metadata(serde_json::json!({"width": 1920, "height": 1080}));

        assert_eq!(analysis.description, "A photo of a boxing ring");
        assert_eq!(analysis.confidence, 0.95);
        assert_eq!(analysis.extracted_text, Some("Round 1".to_string()));
        assert_eq!(analysis.tags.len(), 2);
        assert_eq!(analysis.metadata["width"], 1920);
    }

    #[test]
    fn test_mime_types() {
        let image_types = vec![
            ImageMimeType::Png,
            ImageMimeType::Jpeg,
            ImageMimeType::Gif,
            ImageMimeType::Webp,
            ImageMimeType::Svg,
        ];

        for mime in &image_types {
            let json = serde_json::to_string(mime).expect("serialize mime");
            let deser: ImageMimeType = serde_json::from_str(&json).expect("deserialize mime");
            assert_eq!(&deser, mime);
        }

        let audio_types = vec![
            AudioMimeType::Mp3,
            AudioMimeType::Wav,
            AudioMimeType::Ogg,
            AudioMimeType::Flac,
        ];

        for mime in &audio_types {
            let json = serde_json::to_string(mime).expect("serialize audio mime");
            let deser: AudioMimeType = serde_json::from_str(&json).expect("deserialize audio mime");
            assert_eq!(&deser, mime);
        }
    }

    #[test]
    fn test_supported_types() {
        let supported = vec![
            MediaType::Image(ImageMimeType::Png),
            MediaType::Image(ImageMimeType::Jpeg),
            MediaType::Audio(AudioMimeType::Mp3),
            MediaType::Video,
            MediaType::Pdf,
            MediaType::Document,
        ];

        assert_eq!(supported.len(), 6);
        assert!(supported.contains(&MediaType::Video));
        assert!(supported.contains(&MediaType::Pdf));
        assert!(supported.contains(&MediaType::Document));
    }

    #[test]
    fn test_media_input_metadata() {
        let input = MediaInput::from_base64("aGVsbG8=", MediaType::Image(ImageMimeType::Png))
            .with_source("screenshot.png");

        assert_eq!(input.data, "aGVsbG8=");
        assert_eq!(input.media_type, MediaType::Image(ImageMimeType::Png));
        assert_eq!(input.source, Some("screenshot.png".to_string()));

        let url_input = MediaInput::from_url(
            "https://example.com/image.png",
            MediaType::Image(ImageMimeType::Png),
        );
        assert_eq!(
            url_input.source,
            Some("https://example.com/image.png".to_string())
        );
    }
}
