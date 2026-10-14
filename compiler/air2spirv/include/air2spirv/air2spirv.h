/*
 * air2spirv - Apple AIR (LLVM bitcode dialect) to Vulkan SPIR-V translator.
 *
 * Public C ABI consumed by ritsu-gpu-host (Rust). Design: docs/subsystems/shader-compiler.md.
 *
 * Rules for this header (docs/process/coding-standards.md):
 *   - Must compile as C99 and as C++20. No C++ types cross this boundary.
 *   - Every caller-filled struct starts with `struct_size` for ABI evolution. Callers set it to
 *     sizeof(the struct as they know it), ideally via the matching *_init() function.
 *     The library accepts smaller sizes down to the first published version of the struct and
 *     larger sizes as long as the unknown trailing bytes are all zero.
 *   - No function ever throws or aborts on bad input; failures are reported via a2s_status and
 *     human-readable diagnostics.
 *
 * Stability: API major version 0 means "unstable": any 0.x bump may break source and binary
 * compatibility. The library therefore requires an exact API version match while major == 0.
 *
 * Thread safety:
 *   - a2s_context is immutable after creation. a2s_compile() may be called concurrently from any
 *     number of threads on the same context.
 *   - a2s_output objects are independent of each other and of their context; an output may outlive
 *     the context that produced it. A single output must not be freed while another thread reads
 * it.
 *   - All functions that take no context are thread-safe.
 */
#ifndef AIR2SPIRV_AIR2SPIRV_H
#define AIR2SPIRV_AIR2SPIRV_H

#include <stddef.h>
#include <stdint.h>

#if defined(__GNUC__) || defined(__clang__)
#define A2S_API __attribute__((visibility("default")))
#else
#define A2S_API
#endif

/* Casts usable in macros without tripping C++ -Wold-style-cast in consumers. */
#ifdef __cplusplus
#define A2S_CAST(type, value) (static_cast<type>(value))
#else
#define A2S_CAST(type, value) ((type)(value))
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ---------------------------------------------------------------------------------------------
 * Versioning
 * ------------------------------------------------------------------------------------------- */

#define A2S_MAKE_API_VERSION(major, minor)                                                         \
    ((A2S_CAST(uint32_t, major) << 16) | A2S_CAST(uint32_t, minor))
#define A2S_API_VERSION_GET_MAJOR(version) (A2S_CAST(uint32_t, version) >> 16)
#define A2S_API_VERSION_GET_MINOR(version) (A2S_CAST(uint32_t, version) & 0xFFFFu)

#define A2S_API_VERSION_MAJOR 0
#define A2S_API_VERSION_MINOR 1
/* The API version this header describes. Pass it in a2s_context_desc::api_version. */
#define A2S_API_VERSION A2S_MAKE_API_VERSION(A2S_API_VERSION_MAJOR, A2S_API_VERSION_MINOR)

/* Vulkan version encoding, bit-identical to VK_MAKE_API_VERSION(0, major, minor, 0), so the host
 * can pass VkPhysicalDeviceProperties::apiVersion directly (the patch number is ignored). */
#define A2S_VULKAN_VERSION(major, minor)                                                           \
    ((A2S_CAST(uint32_t, major) << 22) | (A2S_CAST(uint32_t, minor) << 12))

/* Default upper bound for a2s_input::size if a2s_context_desc::max_input_size is 0. */
#define A2S_DEFAULT_MAX_INPUT_SIZE (UINT64_C(256) * 1024u * 1024u)

/* Maximum length (excluding the terminating NUL) of a2s_input::entry_point. */
#define A2S_MAX_ENTRY_POINT_LENGTH 1024u

/* Maximum number of entries in a2s_input::function_constants. */
#define A2S_MAX_FUNCTION_CONSTANTS 65536u

/* Marker for "no value" in uint32_t reflection fields. */
#define A2S_NONE 0xFFFFFFFFu

/* ---------------------------------------------------------------------------------------------
 * Enumerations
 * ------------------------------------------------------------------------------------------- */

typedef enum a2s_status {
    A2S_OK = 0,
    /* The caller violated the API contract (NULL pointer, bad struct_size, reserved bits set...).
     */
    A2S_ERROR_INVALID_ARGUMENT = 1,
    /* The input bytes are structurally broken (truncated, inconsistent sizes, bad magic...). */
    A2S_ERROR_MALFORMED_INPUT = 2,
    /* The input is well-formed but uses something this build will never or does not yet accept,
     * e.g. an unknown container format, a Metal feature without a Vulkan mapping, or a
     * requested target the library does not support. */
    A2S_ERROR_UNSUPPORTED = 3,
    /* The input is valid and in scope, but the code path is not written yet. */
    A2S_ERROR_UNIMPLEMENTED = 4,
    /* A bug in air2spirv (invariant violated, unexpected exception). Please report. */
    A2S_ERROR_INTERNAL = 5,
    /* Memory allocation failed. */
    A2S_ERROR_OUT_OF_MEMORY = 6,
    A2S_STATUS_MAX_ENUM = 0x7FFFFFFF
} a2s_status;

typedef enum a2s_stage {
    A2S_STAGE_UNKNOWN = 0,
    A2S_STAGE_VERTEX = 1,
    A2S_STAGE_FRAGMENT = 2,
    A2S_STAGE_KERNEL = 3,       /* Metal compute kernel -> GLCompute */
    A2S_STAGE_OBJECT = 4,       /* Metal object shader -> TaskEXT */
    A2S_STAGE_MESH = 5,         /* Metal mesh shader -> MeshEXT */
    A2S_STAGE_TILE = 6,         /* Metal tile shader (no Vulkan equivalent) */
    A2S_STAGE_VISIBLE = 7,      /* [[visible]] function (callable, no own pipeline stage) */
    A2S_STAGE_INTERSECTION = 8, /* ray intersection function */
    A2S_STAGE_MAX_ENUM = 0x7FFFFFFF
} a2s_stage;

/* Bit for a stage in stage masks. */
#define A2S_STAGE_BIT(stage) (1u << A2S_CAST(uint32_t, stage))

typedef enum a2s_resource_kind {
    A2S_RESOURCE_UNKNOWN = 0,
    A2S_RESOURCE_BUFFER = 1,             /* [[buffer(n)]] (device or constant address space) */
    A2S_RESOURCE_TEXTURE = 2,            /* [[texture(n)]] */
    A2S_RESOURCE_SAMPLER = 3,            /* [[sampler(n)]] */
    A2S_RESOURCE_THREADGROUP_MEMORY = 4, /* [[threadgroup(n)]], size set at encode time */
    A2S_RESOURCE_ARGUMENT_BUFFER = 5,    /* buffer whose contents are resource references */
    A2S_RESOURCE_ACCELERATION_STRUCTURE = 6,
    A2S_RESOURCE_FUNCTION_TABLE = 7, /* visible / intersection function table */
    A2S_RESOURCE_KIND_MAX_ENUM = 0x7FFFFFFF
} a2s_resource_kind;

typedef enum a2s_constant_type {
    A2S_CONSTANT_TYPE_INVALID = 0,
    A2S_CONSTANT_TYPE_BOOL = 1, /* 1 byte per component, 0 or 1 */
    A2S_CONSTANT_TYPE_INT8 = 2,
    A2S_CONSTANT_TYPE_UINT8 = 3,
    A2S_CONSTANT_TYPE_INT16 = 4,
    A2S_CONSTANT_TYPE_UINT16 = 5,
    A2S_CONSTANT_TYPE_INT32 = 6,
    A2S_CONSTANT_TYPE_UINT32 = 7,
    A2S_CONSTANT_TYPE_INT64 = 8,
    A2S_CONSTANT_TYPE_UINT64 = 9,
    A2S_CONSTANT_TYPE_HALF = 10,
    A2S_CONSTANT_TYPE_BFLOAT = 11,
    A2S_CONSTANT_TYPE_FLOAT = 12,
    A2S_CONSTANT_TYPE_MAX_ENUM = 0x7FFFFFFF
} a2s_constant_type;

/* Access flags in a2s_binding::access. */
#define A2S_ACCESS_READ 0x1u
#define A2S_ACCESS_WRITE 0x2u

/* ---------------------------------------------------------------------------------------------
 * Device feature flags (a2s_compile_options::features)
 *
 * The host sets a bit if the corresponding Vulkan feature is supported AND enabled on the
 * VkDevice the pipeline will be created on. Unknown bits are ignored (a newer host may pass
 * features this library does not know; not using them is always safe).
 * ------------------------------------------------------------------------------------------- */

#define A2S_FEATURE_SHADER_FLOAT16 (UINT64_C(1) << 0)
#define A2S_FEATURE_SHADER_INT8 (UINT64_C(1) << 1)
#define A2S_FEATURE_SHADER_INT16 (UINT64_C(1) << 2)
#define A2S_FEATURE_SHADER_INT64 (UINT64_C(1) << 3)
#define A2S_FEATURE_STORAGE_8BIT (UINT64_C(1) << 4)
#define A2S_FEATURE_STORAGE_16BIT (UINT64_C(1) << 5)
#define A2S_FEATURE_BUFFER_DEVICE_ADDRESS (UINT64_C(1) << 6)
#define A2S_FEATURE_DESCRIPTOR_INDEXING (UINT64_C(1) << 7)
#define A2S_FEATURE_DESCRIPTOR_BUFFER (UINT64_C(1) << 8)
#define A2S_FEATURE_VULKAN_MEMORY_MODEL (UINT64_C(1) << 9)
#define A2S_FEATURE_SUBGROUP_SIZE_CONTROL (UINT64_C(1) << 10)
#define A2S_FEATURE_SHADER_ATOMIC_INT64 (UINT64_C(1) << 11)
#define A2S_FEATURE_SHADER_ATOMIC_FLOAT (UINT64_C(1) << 12)
#define A2S_FEATURE_SHADER_ATOMIC_FLOAT2 (UINT64_C(1) << 13)
#define A2S_FEATURE_SHADER_IMAGE_ATOMIC_INT64 (UINT64_C(1) << 14)
#define A2S_FEATURE_FRAGMENT_SHADER_INTERLOCK (UINT64_C(1) << 15)
#define A2S_FEATURE_DYNAMIC_RENDERING_LOCAL_READ (UINT64_C(1) << 16)
#define A2S_FEATURE_MESH_SHADER (UINT64_C(1) << 17)
#define A2S_FEATURE_RAY_QUERY (UINT64_C(1) << 18)
#define A2S_FEATURE_SHADER_FLOAT_CONTROLS2 (UINT64_C(1) << 19)
#define A2S_FEATURE_SHADER_BFLOAT16 (UINT64_C(1) << 20)
#define A2S_FEATURE_WORKGROUP_MEMORY_EXPLICIT_LAYOUT (UINT64_C(1) << 21)
#define A2S_FEATURE_SHADER_MAXIMAL_RECONVERGENCE (UINT64_C(1) << 22)
#define A2S_FEATURE_STORAGE_IMAGE_WITHOUT_FORMAT (UINT64_C(1) << 23)

/* ---------------------------------------------------------------------------------------------
 * Compile flags (a2s_compile_options::flags). Unknown bits are rejected.
 * ------------------------------------------------------------------------------------------- */

#define A2S_COMPILE_FLAG_OPTIMIZE 0x1u        /* run spirv-opt on the result */
#define A2S_COMPILE_FLAG_DEBUG_INFO 0x2u      /* emit OpName/OpLine debug information */
#define A2S_COMPILE_FLAG_SKIP_VALIDATION 0x4u /* skip spirv-val (never in debug/CI builds) */

/* ---------------------------------------------------------------------------------------------
 * Opaque handles
 * ------------------------------------------------------------------------------------------- */

typedef struct a2s_context a2s_context;
typedef struct a2s_output a2s_output;

/* ---------------------------------------------------------------------------------------------
 * Caller-filled structs
 * ------------------------------------------------------------------------------------------- */

typedef struct a2s_context_desc {
    uint32_t struct_size;    /* sizeof(a2s_context_desc) */
    uint32_t api_version;    /* A2S_API_VERSION the caller was compiled against */
    uint64_t max_input_size; /* bytes; 0 = A2S_DEFAULT_MAX_INPUT_SIZE */
    uint32_t flags;          /* reserved, must be 0 */
    uint32_t reserved;       /* must be 0 */
} a2s_context_desc;

typedef struct a2s_compile_options {
    uint32_t struct_size;           /* sizeof(a2s_compile_options) */
    uint32_t target_vulkan_version; /* A2S_VULKAN_VERSION(1, 3) or newer; 0 = 1.3 */
    uint64_t features;              /* A2S_FEATURE_* */
    /* Subgroup properties of the target device (VkPhysicalDeviceSubgroupSizeControlProperties).
     * 0 = unknown. Must be powers of two in [1, 128] with min <= max. */
    uint32_t min_subgroup_size;
    uint32_t max_subgroup_size;
    /* A2S_STAGE_BIT mask of stages for which the device supports a required subgroup size. */
    uint32_t required_subgroup_size_stages;
    uint32_t flags; /* A2S_COMPILE_FLAG_* */
} a2s_compile_options;

/* Value for one Metal function constant ([[function_constant(index)]]).
 * PROVISIONAL: shape may change until the AIR function-constant encoding is specified. */
typedef struct a2s_function_constant {
    uint32_t index;           /* function constant index */
    uint32_t type;            /* a2s_constant_type */
    uint32_t component_count; /* 1..4 */
    uint32_t reserved;        /* must be 0 */
    uint8_t data[32];         /* components packed little-endian, tightly, from data[0] */
} a2s_function_constant;

typedef struct a2s_input {
    uint32_t struct_size;             /* sizeof(a2s_input) */
    uint32_t function_constant_count; /* entries in function_constants */
    const uint8_t* data;              /* input bytes (bitcode, wrapped bitcode, later .metallib) */
    size_t size;                      /* number of bytes at data */
    /* NUL-terminated entry point name, at most A2S_MAX_ENTRY_POINT_LENGTH bytes.
     * NULL = the input must contain exactly one entry point. */
    const char* entry_point;
    const a2s_compile_options* options; /* NULL = defaults (see a2s_compile_options_init) */
    const a2s_function_constant* function_constants;
} a2s_input;

/* ---------------------------------------------------------------------------------------------
 * Library-filled reflection structs (caller sets struct_size before the call).
 * PROVISIONAL: the binding model is pending an ADR (docs/subsystems/shader-compiler.md §5).
 * ------------------------------------------------------------------------------------------- */

typedef struct a2s_reflection {
    uint32_t struct_size;
    uint32_t stage; /* a2s_stage */
    /* Fixed workgroup size, or 0 per dimension if it is chosen at dispatch time through the
     * specialization constants listed in workgroup_size_spec_ids (A2S_NONE = not used). */
    uint32_t workgroup_size[3];
    uint32_t workgroup_size_spec_ids[3];
    uint32_t required_subgroup_size; /* 0 = none required */
    uint32_t push_constant_size;     /* bytes */
    uint32_t binding_count;          /* valid indices for a2s_output_get_binding */
    uint32_t reserved;
} a2s_reflection;

typedef struct a2s_binding {
    uint32_t struct_size;
    uint32_t kind;           /* a2s_resource_kind */
    uint32_t metal_index;    /* n in [[buffer(n)]], [[texture(n)]], [[sampler(n)]], ... */
    uint32_t access;         /* A2S_ACCESS_* */
    uint32_t descriptor_set; /* A2S_NONE if not bound through a descriptor */
    uint32_t binding;        /* A2S_NONE if not bound through a descriptor */
    uint32_t table_offset;   /* byte offset in the argument table, A2S_NONE if unused */
    uint32_t reserved;
} a2s_binding;

/* ---------------------------------------------------------------------------------------------
 * Functions
 * ------------------------------------------------------------------------------------------- */

/* API version implemented by the loaded library (compare with A2S_API_VERSION). */
A2S_API uint32_t a2s_get_api_version(void);

/* Compiler version + build identity. Part of every cache key: different strings may produce
 * different SPIR-V for the same input. Never NULL, static storage. */
A2S_API const char* a2s_get_version_string(void);

/* Stable identifier-like name for a status ("A2S_OK", ...). Never NULL, static storage. */
A2S_API const char* a2s_status_to_string(a2s_status status);

/* Fill a struct with defaults and the correct struct_size. NULL is ignored. */
A2S_API void a2s_context_desc_init(a2s_context_desc* desc);
A2S_API void a2s_compile_options_init(a2s_compile_options* options);
A2S_API void a2s_input_init(a2s_input* input);

/* Create a context. desc may be NULL (defaults, current API version).
 * On success *out_context receives the context; on failure it is set to NULL. */
A2S_API a2s_status a2s_context_create(const a2s_context_desc* desc, a2s_context** out_context);

/* Destroy a context. NULL is ignored. Outputs created from it stay valid. */
A2S_API void a2s_context_destroy(a2s_context* context);

/* Translate one entry point of the input to SPIR-V.
 *
 * out_output must not be NULL. On return *out_output is either NULL or an output object that
 * the caller must release with a2s_output_free(), regardless of the returned status. The
 * output exists whenever diagnostics are available, in particular on most failures, so the
 * caller can show the reason. The returned status equals a2s_output_get_status(*out_output). */
A2S_API a2s_status a2s_compile(a2s_context* context, const a2s_input* input,
                               a2s_output** out_output);

/* Release an output. NULL is ignored. */
A2S_API void a2s_output_free(a2s_output* output);

/* Status of the compilation that produced this output (A2S_ERROR_INVALID_ARGUMENT for NULL). */
A2S_API a2s_status a2s_output_get_status(const a2s_output* output);

/* All diagnostics as UTF-8 text, one per line, each prefixed with "error: ", "warning: " or
 * "note: ". Empty string if there are none, never NULL (also for a NULL output).
 * Valid until the output is freed. */
A2S_API const char* a2s_output_get_diagnostics(const a2s_output* output);

/* SPIR-V words (host byte order) of a successful compilation, valid until the output is freed.
 * Returns NULL and stores 0 in *out_word_count if there is no module or an argument is NULL. */
A2S_API const uint32_t* a2s_output_get_spirv(const a2s_output* output, size_t* out_word_count);

/* Copy reflection data into *out_reflection (caller sets struct_size first).
 * Returns the output's status if the compilation failed. */
A2S_API a2s_status a2s_output_get_reflection(const a2s_output* output,
                                             a2s_reflection* out_reflection);

/* Copy binding `index` (< a2s_reflection::binding_count) into *out_binding (caller sets
 * struct_size first). Returns the output's status if the compilation failed. */
A2S_API a2s_status a2s_output_get_binding(const a2s_output* output, uint32_t index,
                                          a2s_binding* out_binding);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* AIR2SPIRV_AIR2SPIRV_H */
