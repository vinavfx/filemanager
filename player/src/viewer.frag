#version 330 core
uniform sampler2D image_texture;
uniform float image_gamma;
uniform vec3 display_controls;
uniform int channel;
uniform int rotation;
uniform bool smooth_preview;
in vec2 texture_position;
out vec4 fragment_color;

vec4 notch_weights(float t) {
    float squared = t * t;
    float inverse_squared = (1.0 - t) * (1.0 - t);
    return vec4(inverse_squared, 2.0 - squared, 2.0 - inverse_squared, squared) * 0.25;
}

vec4 notch_sample(vec2 position) {
    vec2 size = vec2(textureSize(image_texture, 0));
    vec2 pixel = position * size - 0.5;
    vec2 base = floor(pixel);
    vec4 wx = notch_weights(fract(pixel.x));
    vec4 wy = notch_weights(fract(pixel.y));
    vec2 gx = vec2(wx.x + wx.y, wx.z + wx.w);
    vec2 gy = vec2(wy.x + wy.y, wy.z + wy.w);
    vec2 x = (base.x + vec2(-0.5, 1.5) + vec2(wx.y, wx.w) / gx) / size.x;
    vec2 y = (base.y + vec2(-0.5, 1.5) + vec2(wy.y, wy.w) / gy) / size.y;
    return mix(
        mix(texture(image_texture, vec2(x.x, y.x)),
            texture(image_texture, vec2(x.y, y.x)), gx.y),
        mix(texture(image_texture, vec2(x.x, y.y)),
            texture(image_texture, vec2(x.y, y.y)), gx.y),
        gy.y
    );
}

void main() {
    vec2 position = texture_position;
    if (rotation == -90) {
        position = vec2(1.0 - texture_position.y, texture_position.x);
    } else if (rotation == 90) {
        position = vec2(texture_position.y, 1.0 - texture_position.x);
    }
    vec4 pixel = smooth_preview
        ? notch_sample(position)
        : texture(image_texture, position);
    vec3 source = channel == 4 ? vec3(pixel.a) : pixel.rgb;
    float source_gamma = channel == 4 ? 1.0 : image_gamma;
    vec3 color = pow(max(source * display_controls.x, vec3(0.0)), vec3(1.0 / source_gamma));
    color = pow(color, vec3(1.0 / display_controls.y));
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luminance), color, display_controls.z);
    if (channel > 0 && channel < 4) {
        color = vec3(color[channel - 1]);
    }
    fragment_color = vec4(color, 1.0);
}
