//! Stele IPC message format.
//!
//! This library defines the IPC message format used by Stele.

use std::fmt::{self, Display, Formatter};
#[cfg(feature = "send_message")]
use std::fs;
#[cfg(feature = "send_message")]
use std::io::Write;
#[cfg(feature = "send_message")]
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
#[cfg(feature = "send_message")]
use std::os::unix::net::UnixStream;
#[cfg(feature = "send_message")]
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "clap")]
use clap::{Args, ValueEnum};
#[cfg(feature = "serde")]
use serde::de::Error as DeError;
#[cfg(feature = "serde")]
use serde::ser::Error as SerError;
#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "sctk")]
use smithay_client_toolkit::shell::wlr_layer::{Anchor, Layer as SctkLayer};
#[cfg(feature = "tracing")]
use tracing::error;

/// Atomic for generating layer content IDs.
static NEXT_RESOURCE_ID: AtomicU32 = AtomicU32::new(0);

/// Send a message to all Stele IPC sockets.
#[cfg(feature = "send_message")]
pub fn send_message(message: &IpcMessage) {
    let runtime_dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);

    let read_dir = match fs::read_dir(&runtime_dir) {
        Ok(read_dir) => read_dir,
        Err(_err) => {
            #[cfg(feature = "tracing")]
            error!("Failed to read runtime dir {runtime_dir:?}: {_err}");
            return;
        },
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("stele-") && name.ends_with(".sock"))
            && let Err(_err) = send_message_to(&path, message)
        {
            #[cfg(feature = "tracing")]
            error!("Failed to send IPC message to {path:?}: {_err}");
        }
    }
}

/// Send a message to a specific Stele IPC socket.
#[cfg(feature = "send_message")]
pub fn send_message_to(socket_path: impl AsRef<Path>, message: &IpcMessage) -> Result<(), IoError> {
    // Provide improved error for missing socket.
    let socket_path = socket_path.as_ref();
    if !socket_path.exists() {
        let msg = format!("socket {socket_path:?} does not exist, make sure Stele is running");
        return Err(IoError::new(IoErrorKind::NotFound, msg));
    }

    let mut stream = UnixStream::connect(socket_path)?;

    // Write message to socket.
    let json = serde_json::to_string(&message)?;
    stream.write_all(json.as_bytes())?;
    stream.flush()?;

    Ok(())
}

/// IPC message format.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "lowercase"))]
#[derive(PartialEq, Clone, Debug)]
pub enum IpcMessage {
    /// Defaults and non-module configuration options.
    Config(Config),
    /// Module state control.
    Module(Module),
}

/// Defaults and non-module configuration options.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "clap", derive(Args))]
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct Config {
    /// Size of the bar in logical pixels.
    #[cfg_attr(feature = "clap", arg(long))]
    pub size: Option<u32>,
    /// Screen edge position.
    #[cfg_attr(feature = "serde", serde(default))]
    #[cfg_attr(feature = "clap", arg(long, value_enum, default_value_t))]
    pub edge: Edge,
    /// Layer shell z-position.
    #[cfg_attr(feature = "serde", serde(default))]
    #[cfg_attr(feature = "clap", arg(long, value_enum, default_value_t))]
    pub layer: Layer,
    /// Bar background layers.
    ///
    /// Several different types of background are supported:
    ///  - Background color in '#rrggbb(aa)' format
    ///  - Path to an image or SVG
    #[cfg_attr(feature = "serde", serde(default))]
    #[cfg_attr(feature = "clap", arg(long = "background", num_args = 1.., verbatim_doc_comment))]
    pub backgrounds: Vec<LayerContent>,
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Bar module component.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "clap", derive(Args))]
#[derive(PartialEq, Clone, Debug)]
pub struct Module {
    /// Unique ID identifying this module.
    #[cfg_attr(feature = "clap", arg(long))]
    pub id: Arc<String>,
    /// Module index within the alignment.
    ///
    /// Modules are positioned to the right of all other modules with equal
    /// alignment and smaller index.
    ///
    /// Modules with equal alignment and index are positioned based on the
    /// chronological order in which they were defined.
    #[cfg_attr(feature = "serde", serde(default))]
    #[cfg_attr(feature = "clap", arg(long, default_value = "0"))]
    pub index: u8,
    /// Horizontal module alignment in the bar.
    #[cfg_attr(feature = "clap", arg(long, value_enum))]
    pub alignment: Alignment,
    /// List of content layers rendered in this module.
    ///
    /// If no layer is specified, the module will be removed.
    #[cfg_attr(feature = "clap", arg(long = "layer", num_args = 1..))]
    pub layers: Vec<ModuleLayer>,
    /// Program to execute on click.
    #[cfg_attr(feature = "clap", arg(skip))]
    pub onclick: Option<Program>,
}

impl Module {
    pub fn new(id: impl Into<String>, alignment: Alignment, layers: Vec<ModuleLayer>) -> Self {
        Self {
            alignment,
            layers,
            id: Arc::new(id.into()),
            onclick: Default::default(),
            index: Default::default(),
        }
    }
}

/// Single content layer in the module.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Clone, Debug)]
pub struct ModuleLayer {
    /// Renderable layer data.
    pub content: LayerContent,
    /// Text options.
    #[cfg_attr(feature = "serde", serde(default))]
    pub font: LayerFont,
    /// Text foreground color.
    pub foreground: Option<Color>,
    /// Module visibilities, based on active mode.
    #[cfg_attr(feature = "serde", serde(default))]
    pub modes: LayerModes,
    /// Alignment within the module.
    #[cfg_attr(feature = "serde", serde(default))]
    pub alignment: Alignment,
    /// Layer size.
    ///
    /// All non-text items have a size of 0x0. When another layer with a
    /// non-zero size is present (either text, or an explicit size), these
    /// elements will automatically grow to fill the total module size. If
    /// only one dimension is zero, only that dimension will grow
    /// dynamically.
    ///
    /// For background colors, this represents the **minimum** size of the
    /// layer, while images will be sized to match this size **exactly**.
    #[cfg_attr(feature = "serde", serde(default))]
    pub size: Size,
    /// Reserved space outside of the layer.
    #[cfg_attr(feature = "serde", serde(default))]
    pub margin: Margin,
}

impl ModuleLayer {
    pub fn new(content: impl Into<LayerContent>) -> Self {
        Self {
            content: content.into(),
            foreground: Default::default(),
            alignment: Default::default(),
            margin: Default::default(),
            modes: Default::default(),
            font: Default::default(),
            size: Default::default(),
        }
    }
}

impl FromStr for ModuleLayer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|err| format!("failed to parse layer: {err}"))
    }
}

/// Text options.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Default, Clone, Debug)]
pub struct LayerFont {
    /// Font family.
    pub family: Option<Arc<String>>,
    /// Text foreground color.
    pub color: Option<Color>,
    /// Font size.
    pub size: Option<f64>,
}

/// Module visibilities, based on active mode.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Default, Clone, Debug)]
pub struct LayerModes {
    /// No other mode active.
    pub default: Option<bool>,
    /// Mouse cursor hover.
    pub hover: Option<bool>,
    /// Mouse button pressed.
    pub active: Option<bool>,
}

/// Renderable layer data.
#[rustfmt::skip]
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum LayerContent {
    // IPC layer types.

    /// Background color layer.
    Color(Color),
    /// Path to an image or SVG.
    Path(Arc<PathBuf>),
    /// Text label.
    Text(Arc<String>),

    // Rust API layer types.

    /// Unparsed SVG file content.
    ///
    /// This can **not** be sent through the IPC socket. See [`Self::Path`] for
    /// rendering an SVG using IPC.
    Svg { id: u32, data: &'static [u8] },
    /// Undecoded image file content.
    ///
    /// This can **not** be sent through the IPC socket. See [`Self::Path`] for
    /// rendering an image using IPC.
    Image { id: u32, data: &'static [u8] },
}

impl LayerContent {
    /// Create a new SVG for rendering.
    ///
    /// This SVG can **not** be sent through the IPC socket. See [`Self::Path`]
    /// for rendering an SVG using IPC.
    pub fn svg(data: &'static [u8]) -> Self {
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::Relaxed);
        Self::Svg { id, data }
    }

    /// Create a new image for rendering.
    ///
    /// This image can **not** be sent through the IPC socket. See
    /// [`Self::Path`] for rendering an image using IPC.
    pub fn image(data: &'static [u8]) -> Self {
        let id = NEXT_RESOURCE_ID.fetch_add(1, Ordering::Relaxed);
        Self::Image { id, data }
    }
}

impl From<Color> for LayerContent {
    fn from(color: Color) -> Self {
        Self::Color(color)
    }
}

impl From<PathBuf> for LayerContent {
    fn from(path: PathBuf) -> Self {
        Self::Path(Arc::new(path))
    }
}

impl From<String> for LayerContent {
    fn from(text: String) -> Self {
        Self::Text(Arc::new(text))
    }
}

impl FromStr for LayerContent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(color) = Color::from_str(s) {
            return Ok(Self::Color(color));
        }
        if s.starts_with("/") || s.starts_with("~") {
            return Ok(Self::Path(PathBuf::from(s).into()));
        }
        Ok(Self::Text(s.to_owned().into()))
    }
}

#[cfg(feature = "serde")]
impl Serialize for LayerContent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Color(color) => serializer.serialize_str(&color.to_string()),
            Self::Path(path) => match path.to_str() {
                Some(path) => serializer.serialize_str(path),
                None => Err(SerError::custom(format!("path contains invalid UTF-8: {path:?}"))),
            },
            Self::Text(text) => serializer.serialize_str(text),
            Self::Svg { .. } => Err(SerError::custom("SVGs cannot be shared directly through IPC")),
            Self::Image { .. } => {
                Err(SerError::custom("images cannot be shared directly through IPC"))
            },
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for LayerContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as generic String.
        let text = String::deserialize(deserializer)?;

        // Attempt to parse as color.
        if let Ok(color) = Color::from_str(&text) {
            return Ok(Self::Color(color));
        }

        // Attempt to parse as path.
        if text.starts_with('~') || text.starts_with('/') {
            return Ok(Self::Path(Arc::new(PathBuf::from(text))));
        }

        // Use plain text as fallback
        Ok(Self::Text(Arc::new(text)))
    }
}

/// Alignment within a container.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[cfg_attr(feature = "clap", derive(ValueEnum))]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub enum Alignment {
    Start,
    #[default]
    Center,
    End,
}

/// External program.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Program {
    pub program: String,
    #[cfg_attr(feature = "serde", serde(default))]
    pub args: Vec<String>,
}

/// RGB color.
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self::new_alpha(r, g, b, 255)
    }

    pub const fn new_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

impl From<Color> for [f32; 4] {
    fn from(color: Color) -> Self {
        let Color { r, g, b, a } = color;
        [r as f32 / 255., g as f32 / 255., b as f32 / 255., a as f32 / 255.]
    }
}

impl From<Color> for [f64; 3] {
    fn from(color: Color) -> Self {
        let Color { r, g, b, .. } = color;
        [r as f64 / 255., g as f64 / 255., b as f64 / 255.]
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let channels = match s.strip_prefix('#') {
            Some(channels) => channels,
            None => {
                return Err(format!("color {s:?} is missing leading '#'"));
            },
        };

        let digits = channels.len();
        if digits != 6 && digits != 8 {
            return Err(format!("color {s:?} has {digits} digits; expected 6 or 8"));
        }

        match u32::from_str_radix(channels, 16) {
            Ok(mut color) => {
                let a = if digits == 8 {
                    let a = (color & 0xFF) as u8;
                    color >>= 8;
                    a
                } else {
                    255
                };
                let b = (color & 0xFF) as u8;
                color >>= 8;
                let g = (color & 0xFF) as u8;
                color >>= 8;
                let r = color as u8;

                Ok(Color::new_alpha(r, g, b, a))
            },
            Err(_) => Err(format!("color {s:?} contains non-hex digits")),
        }
    }
}

impl Display for Color {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        if self.a != 255 {
            write!(f, "#{:0>2x}{:0>2x}{:0>2x}{:0>2x}", self.r, self.g, self.b, self.a)
        } else {
            write!(f, "#{:0>2x}{:0>2x}{:0>2x}", self.r, self.g, self.b)
        }
    }
}

#[cfg(feature = "serde")]
impl Serialize for Color {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let text = self.to_string();
        serializer.serialize_str(&text)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Color::from_str(&text).map_err(DeError::custom)
    }
}

/// Layer shell z-position.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[cfg_attr(feature = "clap", derive(ValueEnum))]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub enum Layer {
    Background,
    #[default]
    Bottom,
    Top,
    Overlay,
}

#[cfg(feature = "sctk")]
impl From<Layer> for SctkLayer {
    fn from(layer: Layer) -> Self {
        match layer {
            Layer::Background => Self::Background,
            Layer::Bottom => Self::Bottom,
            Layer::Top => Self::Top,
            Layer::Overlay => Self::Overlay,
        }
    }
}

/// Screen edge position.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[cfg_attr(feature = "clap", derive(ValueEnum))]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub enum Edge {
    #[default]
    Top,
    Right,
    Bottom,
    Left,
}

#[cfg(feature = "sctk")]
impl From<Edge> for Anchor {
    fn from(edge: Edge) -> Self {
        match edge {
            Edge::Top => Anchor::LEFT | Anchor::TOP | Anchor::RIGHT,
            Edge::Right => Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM,
            Edge::Bottom => Anchor::LEFT | Anchor::BOTTOM | Anchor::RIGHT,
            Edge::Left => Anchor::TOP | Anchor::LEFT | Anchor::BOTTOM,
        }
    }
}

/// 2D size.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Margin around a layer's content.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub struct Margin {
    pub left: u32,
    pub right: u32,
}
