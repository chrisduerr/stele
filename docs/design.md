# Design

Stele does not collect any system information by itself. Stele provides an IPC
socket, through which one or more external processes can submit content for
rendering. The modules submitted through IPC are then parsed, laid out, and
rendered to a Wayland layer shell surface using Vulkan.

## IPC Format

The IPC uses JSON messages over a Unix domain socket.

The default socket location is
`${XDG_RUNTIME_DIR:-/run/user/$UID}/stele-$STELE_PID.sock`.

Message types are identified using the mandatory `type` field.

### `Config` Message

The config message expects a single JSON object, controlling defaults and
non-module configuration options.

```text
Config {
    // Message type.
    "type": "config",

    // Size of the bar in logical pixels (default: 35).
    "size"?: uint,

    // Name of the output the bar should be placed on.
    "output"?: string,

    // Screen edge position (default "top").
    "edge"?: "top" | "right" | "bottom" | "left",

    // Layer shell z-position (default: "bottom").
    "layer"?: "background" | "bottom" | "top" | "overlay",

    // Bar background layers (default: "#000000").
    //
    // Several different types of background are supported:
    //  - Background color in `#rrggbb(aa)` format
    //  - Path to an image or SVG
    "backgrounds"?: [string],
}
```

#### Initialization

To avoid modules popping in one by one during startup, nothing is rendered
unless the first `config` message is received. Even if the defaults are used,
this message must be sent for rendering to begin.

After the initial draw, there is no synchronization between modules. All module
changes are applied immediately and further `config` messages are only required
to update the configuration.

#### Examples

```json
{
    "size": 35,
    "layer": "top"
}
```

### `Module` Message

The module message expects a single JSON object, which is used to create,
update, or delete a module.

```text
Module {
    // Message type.
    "type": "module",

    // Unique ID identifying this module.
    "id": string,

    // Module index within the alignment (default: 0).
    //
    // Modules are positioned to the right of all other modules with equal
    // alignment and smaller index.
    //
    // Modules with equal alignment and index are ordered arbitrarily.
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
    //  - Background color in `#rrggbb(aa)` format
    //  - Path to an image or SVG
    //  - Text
    "content": string,

    // Text options (default: none).
    "font"?: {
        // Font family (default: "sans").
        "family"?: string,
        // Text foreground color (default: "#ffffff").
        "color"?: "#rrggbb(aa)",
        // Font size (default: 16).
        "size"?: float,
    },

    // Text foreground color (default: "#ffffff").
    //
    // This will only affect layers with text as `content`.
    "foreground"?: "#rrggbb(aa)"

    // Module visibilities, based on active mode (all default: true).
    "modes"?: {
        // No other mode active.
        "default"?: bool,
        // Mouse cursor hover.
        "hover"?: bool,
        // Mouse button pressed.
        "active"?: bool,
    },

    // Alignment within the module (default: "center").
    "alignment"?: "start" | "center" | "end",

    // Layer size (default: 0x0)
    //
    // A dimension other than `0` for background colors acts as a **minimum**
    // size for the layer, while images and text will be resized or cropped to
    // match it **exactly**.
    //
    // Dimensions equal to `0` are dynamically sized:
    //  - Colors layers dynamically resize to match their children's size
    //  - SVG layers dynamically resize to match their parent's size
    //  - PNG and Text layers use their image/text's size
    "size"?: {
        "width"?: uint,
        "height"?: uint,
    },

    // Reserved space outside of the layer (default: all 0).
    "margin"?: {
        "top"?: uint,
        "right"?: uint,
        "bottom"?: uint,
        "left"?: uint,
    },
}
```

#### Module Lifecycle

A module is **added** and first drawn when it is sent to the panel through IPC
with a non-empty `layers` array.

A module is **updated** by sending the updated module state with an identical
`id`.

A module is **removed** by sending an empty `layers` array with an identical
`id`.

#### Units

All lengths and sizes, including font size, are in logical units. This means a
module with a width of `32` would be `64` pixels wide when rendered on an output
with a scale of `2`.

#### Examples

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
