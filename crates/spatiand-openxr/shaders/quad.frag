#version 450
layout(binding = 0) uniform texture2D picture;
layout(binding = 1) uniform sampler filtering;
layout(push_constant) uniform Quad {
    mat4 mvp;
    vec4 uv_rect;
    vec4 flags;
} q;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 colour;

void main() {
    vec4 c = texture(sampler2D(picture, filtering), uv);
    if (q.flags.x < 0.5) {
        c.a = 1.0;
    } else if (q.flags.y > 0.5) {
        c.rgb *= c.a;
    }
    colour = c;
}
