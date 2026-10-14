// Own test source for the air2spirv bitcode fixtures (WP-SC-02). Written for this project; no
// third-party or Apple code. Compiled with clang-18 to LLVM bitcode by ../generate.sh.
// Aggregates, pointer arithmetic, a global constant table and vector types.
typedef float float4 __attribute__((ext_vector_type(4)));

struct Vertex {
    float4 position;
    float4 color;
    unsigned id;
};

static const float kWeights[4] = {0.125f, 0.25f, 0.25f, 0.375f};

float4 blend(const struct Vertex* v, unsigned count) {
    float4 sum = {0.0f, 0.0f, 0.0f, 0.0f};
    for (unsigned i = 0; i < count; ++i) {
        sum += v[i].color * kWeights[v[i].id & 3U];
    }
    return sum;
}

void translate(struct Vertex* v, unsigned count, float4 offset) {
    for (struct Vertex* end = v + count; v != end; ++v) {
        v->position += offset;
    }
}
