//! JSON shared by the editor and the `pirosman-ofxhost` process.
//!
//! - `pirosman-ofxhost scan <binary.ofx>` prints one [`ScanReport`].
//! - `pirosman-ofxhost render <spec.json>` reads [`RenderSpec`], then streams
//!   `frames` RGBA frames (`rgba64le`, top row first) from stdin to stdout,
//!   each run through the effect chain in order.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Version of this protocol; the editor refuses a host that reports another.
pub const PROTOCOL: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub protocol: u32,
    pub binary: String,
    pub plugins: Vec<PluginInfo>,
    /// Set when the binary could not be loaded at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Index passed to `OfxGetPlugin`.
    pub index: u32,
    pub id: String,
    pub version: [u32; 2],
    pub label: String,
    /// `kOfxImageEffectPluginPropGrouping`, e.g. "Sapphire Blur+Sharpen" or "BCC/Blur".
    pub grouping: String,
    #[serde(default)]
    pub description: String,
    pub contexts: Vec<String>,
    /// The context Pirosman instantiates (filter, else general), if any.
    #[serde(default)]
    pub context: Option<String>,
    pub depths: Vec<String>,
    pub params: Vec<ParamInfo>,
    /// False when the plugin needs something this host does not provide.
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamInfo {
    pub name: String,
    pub label: String,
    /// double, int, bool, choice, rgb, rgba, double2d, int2d, double3d, int3d,
    /// string, group, page, button, custom, parametric, bytes.
    pub kind: String,
    /// Number, array of numbers, string or bool, matching `kind`.
    pub default: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub min: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub max: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_min: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_max: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent: String,
    #[serde(default)]
    pub secret: bool,
    /// Spatial values the editor stores as fractions of the frame.
    #[serde(default)]
    pub normalised: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderSpec {
    pub protocol: u32,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub frame_rate: f64,
    pub effects: Vec<EffectSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectSpec {
    pub binary: String,
    pub index: u32,
    pub id: String,
    /// Values by parameter name; missing ones use the plugin default.
    #[serde(default)]
    pub params: Map<String, Value>,
}
