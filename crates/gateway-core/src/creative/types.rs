//! Shared types for creative tool abstraction

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Clip identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipId(pub String);

impl From<&str> for ClipId {
    fn from(s: &str) -> Self {
        ClipId(s.to_string())
    }
}

impl From<String> for ClipId {
    fn from(s: String) -> Self {
        ClipId(s)
    }
}

/// Timeline identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimelineId(pub String);

/// Node identifier (for color grading nodes)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

/// Timecode representation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub frame_rate: f32,
}

impl Timecode {
    pub fn new(hours: u8, minutes: u8, seconds: u8, frames: u8, frame_rate: f32) -> Self {
        Self {
            hours,
            minutes,
            seconds,
            frames,
            frame_rate,
        }
    }

    pub fn from_frames(total_frames: u64, frame_rate: f32) -> Self {
        let fps = frame_rate as u64;
        let frames = (total_frames % fps) as u8;
        let total_seconds = total_frames / fps;
        let seconds = (total_seconds % 60) as u8;
        let total_minutes = total_seconds / 60;
        let minutes = (total_minutes % 60) as u8;
        let hours = (total_minutes / 60) as u8;

        Self {
            hours,
            minutes,
            seconds,
            frames,
            frame_rate,
        }
    }

    pub fn to_frames(&self) -> u64 {
        let fps = self.frame_rate as u64;
        (self.hours as u64 * 3600 + self.minutes as u64 * 60 + self.seconds as u64) * fps
            + self.frames as u64
    }

    pub fn to_seconds(&self) -> f64 {
        self.to_frames() as f64 / self.frame_rate as f64
    }
}

impl std::fmt::Display for Timecode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// Clip information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipInfo {
    pub id: ClipId,
    pub name: String,
    pub file_path: Option<String>,
    pub duration: Timecode,
    pub start_timecode: Timecode,
    pub end_timecode: Timecode,
    pub resolution: Resolution,
    pub frame_rate: f32,
    pub codec: Option<String>,
    pub has_audio: bool,
    pub has_video: bool,
    pub tags: Vec<String>,
    pub markers: Vec<Marker>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Video resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn is_4k(&self) -> bool {
        self.width >= 3840 && self.height >= 2160
    }

    pub fn is_hd(&self) -> bool {
        self.width >= 1920 && self.height >= 1080
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.width as f32 / self.height as f32
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Marker on a clip or timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub id: String,
    pub name: String,
    pub timecode: Timecode,
    pub duration: Option<Timecode>,
    pub color: MarkerColor,
    pub notes: Option<String>,
}

/// Marker colors
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerColor {
    Blue,
    Cyan,
    Green,
    Yellow,
    Red,
    Pink,
    Purple,
    Fuchsia,
    Rose,
    Lavender,
    Sky,
    Mint,
    Lemon,
    Sand,
    Cocoa,
    Cream,
}

/// Timeline information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineInfo {
    pub id: TimelineId,
    pub name: String,
    pub duration: Timecode,
    pub resolution: Resolution,
    pub frame_rate: f32,
    pub video_tracks: u32,
    pub audio_tracks: u32,
    pub clip_count: u32,
}

/// Color node types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    /// Color wheels (Lift, Gamma, Gain)
    ColorWheels,
    /// RGB curves
    Curves,
    /// HSL qualifiers
    Qualifier,
    /// Power windows
    Window,
    /// LUT application
    Lut,
    /// Color space transform
    ColorSpace,
    /// Custom OpenFX
    OpenFx,
    /// Serial node
    Serial,
    /// Parallel node
    Parallel,
    /// Layer node
    Layer,
}

/// Color wheel adjustments
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ColorWheelAdjustments {
    /// Lift (shadows) adjustments
    pub lift: WheelValues,
    /// Gamma (midtones) adjustments
    pub gamma: WheelValues,
    /// Gain (highlights) adjustments
    pub gain: WheelValues,
    /// Offset adjustments
    pub offset: WheelValues,
}

/// Individual wheel values
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WheelValues {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub master: f32,
}

/// Primary color corrections
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PrimaryCorrections {
    pub temperature: f32,      // -100 to 100
    pub tint: f32,             // -100 to 100
    pub contrast: f32,         // -100 to 100
    pub pivot: f32,            // 0 to 1
    pub saturation: f32,       // 0 to 200
    pub hue: f32,              // -180 to 180
    pub luminance_mix: f32,    // 0 to 100
}

/// Curve point for RGB curves
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f32,  // 0.0 to 1.0
    pub y: f32,  // 0.0 to 1.0
}

/// RGB curves definition
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RgbCurves {
    pub master: Vec<CurvePoint>,
    pub red: Vec<CurvePoint>,
    pub green: Vec<CurvePoint>,
    pub blue: Vec<CurvePoint>,
}

impl RgbCurves {
    /// Create an S-curve for contrast enhancement
    pub fn s_curve(intensity: f32) -> Self {
        let offset = intensity * 0.1;
        Self {
            master: vec![
                CurvePoint { x: 0.0, y: 0.0 },
                CurvePoint { x: 0.25, y: 0.25 - offset },
                CurvePoint { x: 0.75, y: 0.75 + offset },
                CurvePoint { x: 1.0, y: 1.0 },
            ],
            red: vec![],
            green: vec![],
            blue: vec![],
        }
    }
}

/// Audio normalization settings
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AudioNormalization {
    pub target_lufs: f32,        // Target loudness (e.g., -16 LUFS)
    pub true_peak_limit: f32,    // True peak limit (e.g., -1 dBTP)
    pub loudness_range: Option<f32>,  // Target LRA
}

impl Default for AudioNormalization {
    fn default() -> Self {
        Self {
            target_lufs: -16.0,
            true_peak_limit: -1.0,
            loudness_range: None,
        }
    }
}

/// Audio analysis results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioAnalysis {
    pub integrated_lufs: f32,
    pub true_peak_db: f32,
    pub loudness_range: f32,
    pub short_term_max_lufs: f32,
    pub momentary_max_lufs: f32,
}

/// Export format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    ProRes422,
    ProRes422Hq,
    ProRes4444,
    ProResRaw,
    H264,
    H265,
    Dnxhd,
    Dnxhr,
    Cineform,
    Exr,
    Dpx,
    Tiff,
}

/// Export settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSettings {
    pub format: ExportFormat,
    pub resolution: Resolution,
    pub frame_rate: f32,
    pub output_path: String,
    pub video_bitrate: Option<u32>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<u32>,
    pub color_space: Option<String>,
    pub gamma: Option<String>,
    pub render_audio: bool,
    pub render_video: bool,
}

/// Filter for selecting clips
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipFilter {
    pub tags: Vec<String>,
    pub name_contains: Option<String>,
    pub min_duration_seconds: Option<f32>,
    pub max_duration_seconds: Option<f32>,
    pub has_audio: Option<bool>,
    pub has_video: Option<bool>,
    pub markers: Option<Vec<String>>,
}

impl ClipFilter {
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn with_name(mut self, name_contains: impl Into<String>) -> Self {
        self.name_contains = Some(name_contains.into());
        self
    }
}

/// Project information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: Option<String>,
    pub frame_rate: f32,
    pub resolution: Resolution,
    pub color_science: Option<String>,
    pub timeline_count: u32,
    pub clip_count: u32,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
}
