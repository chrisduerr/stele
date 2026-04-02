#version 450

layout(location = 0) in vec2 position;
layout(location = 1) in vec2 uv;
layout(location = 2) in float is_premultiplied;

layout(location = 0) out vec2 tex_coords;
layout(location = 1) out float out_is_premultiplied;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);
    out_is_premultiplied = is_premultiplied;
    tex_coords = uv;
}
