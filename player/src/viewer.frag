#version 330 core
uniform sampler2D image_texture;
uniform float image_gamma;
uniform int channel;
uniform int rotation;
in vec2 texture_position;
out vec4 fragment_color;
void main() {
    vec2 position = texture_position;
    if (rotation == -90) {
        position = vec2(1.0 - texture_position.y, texture_position.x);
    } else if (rotation == 90) {
        position = vec2(texture_position.y, 1.0 - texture_position.x);
    }
    vec4 pixel = texture(image_texture, position);
    vec3 color = pow(max(pixel.rgb, vec3(0.0)), vec3(1.0 / image_gamma));
    if (channel > 0 && channel < 4) {
        color = vec3(color[channel - 1]);
    } else if (channel == 4) {
        color = vec3(pixel.a);
    }
    fragment_color = vec4(color, 1.0);
}
