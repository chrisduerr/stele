# Stele

Stele is a Wayland bar/dock/panel with a Vulkan renderer and an IPC-based
configuration interface.

Instead of using a configuration file to customize pre-defined modules, Stele is
only responsible for rendering. IPC and Rust interfaces are provided to write
your own modules in whichever language you prefer.

## Examples

While Stele is all about making it your own, the following examples show off
some of what's possible:

#### [Stele Undead](https://github.com/chrisduerr/stele_undead)

<img width="2560" height="72" alt="tmp" src="https://github.com/user-attachments/assets/6f4cfb56-bd79-47cf-b8a1-df0e43f21019" />

## Configuration / IPC Interface

The full IPC interface of Stele is documented [here](docs/design.md).

A good way to get started with writing your own modules is looking at the
[examples](./examples).

## Building from Source

Stele is compiled with cargo, which creates a binary at `target/release/stele`:

```bash
cargo build --release
```

To run Stele, the following requirements must be met:
 - GPU with Vulkan support
 - Wayland with [wlr-layer-shell] support
 - Pango/Cairo dependency

[wlr-layer-shell]: https://wayland.app/protocols/wlr-layer-shell-unstable-v1

## Planned Features

The following features are currently **not** supported, but will be added in the
future:
 - Mouse Input
