#version 450

layout(location = 0) in vec2 tex_coords;
layout(location = 1) in float is_premultiplied;

layout(location = 0) out vec4 f_color;

layout(set = 0, binding = 0) uniform sampler2D tex;

void main() {
    // Get fragment color from texture.
    vec4 texture_color = texture(tex, tex_coords);

    // Calculate both un- and premultiplied alpha colors.
    //
    // To avoid branching we just calculate both,
    // then multiply the one we don't need by zero.
    vec3 original = vec3(texture_color) * is_premultiplied;
    vec3 premult = vec3(texture_color) * texture_color.a * (1. - is_premultiplied);

    f_color = vec4(premult + original, texture_color.a);
}
