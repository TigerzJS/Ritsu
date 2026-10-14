// Own test source for the air2spirv bitcode fixtures (WP-SC-02). Written for this project; no
// third-party or Apple code. Compiled with clang-18 to LLVM bitcode by ../generate.sh.
// Covers loops with break/continue, a switch and a helper function, so the parsed module has
// several functions with non-trivial control flow.
typedef unsigned int u32;

static u32 mix(u32 x) {
    x ^= x >> 16;
    x *= 0x7feb352dU;
    x ^= x >> 15;
    return x;
}

u32 classify(u32 v) {
    switch (v & 3U) {
    case 0:
        return mix(v);
    case 1:
        return v + 7U;
    case 2:
        return v * 3U;
    default:
        return 0;
    }
}

u32 checksum(const u32* data, u32 count) {
    u32 acc = 0;
    for (u32 i = 0; i < count; ++i) {
        if (data[i] == 0) {
            continue;
        }
        if (data[i] == 0xFFFFFFFFU) {
            break;
        }
        acc += classify(data[i]);
    }
    return acc;
}
