// Own test kernel for the air2spirv bitcode fixtures (WP-SC-02). Written for this project; no
// third-party or Apple code. Compiled with clang-18 to LLVM bitcode by ../generate.sh.
__kernel void vector_add(__global const float* a, __global const float* b, __global float* out,
                         uint n) {
    const size_t i = get_global_id(0);
    if (i < n) {
        out[i] = a[i] + b[i];
    }
}

__kernel void scale(__global float* data, float factor) {
    const size_t i = get_global_id(0);
    data[i] = data[i] * factor;
}
