#version 330 core
uniform vec4 image_rect;
out vec2 texture_position;
const vec2 corners[6] = vec2[6](
    vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
);
void main() {
    texture_position = corners[gl_VertexID];
    vec2 position = image_rect.xy + texture_position * image_rect.zw;
    gl_Position = vec4(position.x * 2.0 - 1.0, 1.0 - position.y * 2.0, 0.0, 1.0);
}
