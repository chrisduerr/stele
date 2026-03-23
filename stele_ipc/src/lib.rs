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
use serde::ser::Error as SerError;
#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

/// Bar module component.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Module {
    /// Unique ID identifying this module.
    id: String,
    /// Module index within the alignment (default: 0).
    ///
    /// Modules are positioned to the right of all other modules with equal
    /// alignment and smaller index.
    ///
    /// Modules with equal alignment and index are positioned based on the
    /// chronological order in which they were defined.
    #[cfg_attr(feature = "serde", serde(default))]
    index: u8,
    /// Horizontal module alignment in the bar.
    alignment: Alignment,
    /// List of content layers rendered in this module.
    layers: Vec<ModuleLayer>,
    /// Program to execute on click (default: none).
    #[serde(default)]
    onclick: Option<Program>,
}

/// Single content layer in the module.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ModuleLayer {
    /// Renderable layer data.
    content: LayerContent,
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
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
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
