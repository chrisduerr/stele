//! Stele IPC message format.
//!
//! This library defines the IPC message format used by Stele.

use std::fmt::{self, Display, Formatter};
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

#[cfg(feature = "serde")]
use serde::de::Error as DeError;
#[cfg(feature = "serde")]
use serde::ser::Error as SerError;
#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "sctk")]
use smithay_client_toolkit::shell::wlr_layer::{Anchor, Layer as SctkLayer};
#[cfg(feature = "vulkano")]
use vulkano::format::ClearValue;

/// Send a message to the Catacomb IPC socket.
#[cfg(feature = "send_message")]
pub fn send_message(socket_path: &Path, module: &Module) -> Result<(), IoError> {
    // Provide improved error for missing socket.
    if !socket_path.exists() {
        let msg = format!("socket {socket_path:?} does not exist, make sure Stele is running");
        return Err(IoError::new(IoErrorKind::NotFound, msg));
    }

    let mut stream = UnixStream::connect(socket_path)?;

    // Write message to socket.
    let json = serde_json::to_string(&module)?;
    stream.write_all(json.as_bytes())?;
    stream.flush()?;

    Ok(())
}

/// IPC message format.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "lowercase"))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum IpcMessage {
    /// Defaults and non-module configuration options.
    Config(Config),
    /// Module state control.
    Module(Module),
}

/// Defaults and non-module configuration options.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Copy, Clone, Default, Debug)]
pub struct Config {
    /// Size of the bar in logical pixels.
    #[cfg_attr(feature = "serde", serde(default))]
    pub size: Option<u32>,
    /// Screen edge position.
    #[cfg_attr(feature = "serde", serde(default))]
    pub edge: Edge,
    /// Layer shell z-position.
    #[cfg_attr(feature = "serde", serde(default))]
    pub layer: Layer,
    /// Bar background.
    #[cfg_attr(feature = "serde", serde(default))]
    pub background: Option<Color>,
}

/// Bar module component.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Module {
    /// Unique ID identifying this module.
    pub id: String,
    /// Module index within the alignment.
    ///
    /// Modules are positioned to the right of all other modules with equal
    /// alignment and smaller index.
    ///
    /// Modules with equal alignment and index are positioned based on the
    /// chronological order in which they were defined.
    #[cfg_attr(feature = "serde", serde(default))]
    pub index: u8,
    /// Horizontal module alignment in the bar.
    pub alignment: Alignment,
    /// List of content layers rendered in this module.
    pub layers: Vec<ModuleLayer>,
    /// Program to execute on click.
    #[serde(default)]
    pub onclick: Option<Program>,
}

/// Single content layer in the module.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ModuleLayer {
    /// Renderable layer data.
    pub content: LayerContent,
}

/// Renderable layer data.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum LayerContent {
    Color(Color),
    Path(PathBuf),
    Text(String),
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
            return Ok(Self::Path(PathBuf::from(text)));
        }

        // Use plain text as fallback
        Ok(Self::Text(text))
    }
}

/// Alignment within a container.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
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
    program: String,
    #[cfg_attr(feature = "serde", serde(default))]
    args: Vec<String>,
}

/// RGB color.
#[derive(Hash, PartialOrd, Ord, PartialEq, Eq, Default, Copy, Clone, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
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
        Color::from_str(&text).map_err(|err| DeError::custom(err))
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
        if digits != 6 {
            return Err(format!("color {s:?} has {digits} digits; expected 6"));
        }

        match u32::from_str_radix(channels, 16) {
            Ok(mut color) => {
                let b = (color & 0xFF) as u8;
                color >>= 8;
                let g = (color & 0xFF) as u8;
                color >>= 8;
                let r = color as u8;

                Ok(Color::new(r, g, b))
            },
            Err(_) => Err(format!("color {s:?} contains non-hex digits")),
        }
    }
}

impl Display for Color {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "#{:0>2x}{:0>2x}{:0>2x}", self.r, self.g, self.b)
    }
}

#[cfg(feature = "vulkano")]
impl From<Color> for ClearValue {
    fn from(color: Color) -> Self {
        let Color { r, g, b } = color;
        ClearValue::Float([r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.])
    }
}

/// Layer shell z-position.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
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
