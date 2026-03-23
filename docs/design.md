# Design

Stele does not collect any system information by itself. Stele provides an IPC
socket, through which one or more external processes can submit content for
rendering. The modules submitted through IPC are then parsed, laid out, and
rendered to a Wayland layer shell surface using Vulkan.

## IPC Format

The IPC uses JSON messages over a Unix domain socket. Messages consist of a
single module object with the following format:

```text
Module {
    // Unique ID identifying this module.
    "id": string,

    // Module index within the alignment (default: 0).
    //
    // Modules are positioned to the right of all other modules with equal
    // alignment and smaller index.
    //
    // Modules with equal alignment and index are positioned based on the
    // chronological order in which they were defined.
    "index"?: uint,

    // Horizontal module alignment in the bar.
    "alignment": "start" | "center" | "end",

    // List of content layers rendered in this module.
    "layers": [ModuleLayer],

    // Program to execute on click (default: none).
    "onclick"?: {
        "program": string,
        "args"?: [string]
    },
}

// Single content layer in the module.
//
// All fields other than `content` are optional.
ModuleLayer {
    // Renderable layer data.
    //
    // Several different types of content are supported:
    //  - Background color in `#rrggbb` format
    //  - Path to an image or SVG
    //  - Text
    "content": string,

    // Text options (default: none).
    "font"?: {
        // Font family (default: "sans").
        "family"?: string,
        // Text foreground color (default: "#ffffff").
        "color"?: "#rrggbb",
        // Font size (default: 16).
        "size"?: float,
    },

    // Text foreground color (default: "#ffffff").
    //
    // This will only affect layers with text as `content`.
    "foreground"?: "#rrggbb"

    // Module visibilities based on active mode (all default: true).
    "modes"?: {
        // No other mode active.
        "default"?: bool,
        // Mouse cursor hover.
        "hover"?: bool,
        // Mouse button pressed.
        "active"?: bool,
    },

    // Alignment within the module (default: center).
    "alignment"?: "start" | "center" | "end",

    // Layer size (default: 0x0)
    //
    // All non-text items have a size of 0x0. When another layer with a non-zero
    // size is present (either text, or an explicit size), these elements will
    // automatically grow to fill the total module size. If only one dimension
    // is zero, only that dimension will grow dynamically.
    //
    // For background colors this represents the *minimum* size of the layer,
    // while images will be sized to match this size *exactly*.
    "size"?: {
        "width"?: uint,
        "height"?: uint,
    },

    // Reserved space outside of the layer (default: none).
    "margin"?: {
        "left": uint,
        "right": uint,
    },
}
```

## Module Lifecycle

A module is **added** and first drawn when it is sent to the panel through IPC
with a non-empty `layers` array.

A module is **updated** by sending the updated module state with an identical
`id`.

A module is **removed** by sending an empty `layers` array with an identical
`id`.

## Examples

The following module will draw the text `04:01` to the center of the panel:

```json
{
    "id": "clock",
    "alignment": "center",
    "layers": [{ "content": "04:01" }]
}
```

More complicated components like workspaces can be composed out of multiple
separate modules:

```json
{
    "id": "workspace_0",
    "index": 0,
    "alignment": "left",
    "layers": [{
        "content": "#181818"
    }, {
        "content": "#616161",
        "modes": { "default": false }
    }, {
        "content": "/usr/share/icons/hicolor/scalable/apps/firefox.svg",
        "size": {
            "width": 32,
            "height": 32
        },
        "margin": {
            "left": 16,
            "right": 16
        }
    }]
}
```

```json
{
    "id": "workspace_1",
    "index": 1,
    "alignment": "left",
    "layers": [{
        "content": "#282828"
    }, {
        "content": "#616161",
        "modes": { "default": false }
    }, {
        "content": "/usr/share/icons/hicolor/scalable/apps/alacritty.svg",
        "size": {
            "width": 32,
            "height": 32
        },
        "margin": {
            "left": 16,
            "right": 16
        }
    }]
}
```
