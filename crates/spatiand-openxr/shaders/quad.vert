#version 450
// One textured quad from nothing: the six corners come from the vertex index, the placement from
// a matrix pushed per draw. The unit square is centred on the origin and `mvp` carries its size.
layout(push_constant) uniform Quad {
    mat4 mvp;
    vec4 uv_rect;   // u0, v0, u1, v1 of the part of the image to show
    vec4 flags;     // x: 1 if the image's alpha is used, y: 1 if it is not premultiplied
} q;
layout(location = 0) out vec2 uv;

void main() {
    int i = int(gl_VertexIndex);
    float x = (i == 1 || i == 4 || i == 5) ? 1.0 : 0.0;
    float y = (i == 2 || i == 3 || i == 5) ? 1.0 : 0.0;
    uv = vec2(mix(q.uv_rect.x, q.uv_rect.z, x), mix(q.uv_rect.y, q.uv_rect.w, y));
    gl_Position = q.mvp * vec4(x - 0.5, 0.5 - y, 0.0, 1.0);
}
