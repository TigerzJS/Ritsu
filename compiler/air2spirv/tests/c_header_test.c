/* Compiles the public header as strict C99 and exercises it through a C translation unit.
 * Exit code 0 = pass. Registered as ctest test air2spirv.c_header.compiles_and_links_as_c99. */
#include "air2spirv/air2spirv.h"

#include "a2s_fixture_minimal_bc.h" /* generated: tests/fixtures/bitcode/minimal.bc */

#include <stdio.h>
#include <string.h>

static int failures = 0;

#define C_CHECK(cond)                                                                              \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #cond);               \
            ++failures;                                                                            \
        }                                                                                          \
    } while (0)

int main(void) {
    a2s_context_desc desc;
    a2s_compile_options options;
    a2s_input input;
    a2s_context* context = NULL;
    a2s_output* output = NULL;
    a2s_reflection reflection;
    a2s_binding binding;
    a2s_function_constant constant;
    a2s_status status;
    size_t word_count = 1;

    /* All enumerators and macros must be usable from C. */
    a2s_stage stage = A2S_STAGE_KERNEL;
    a2s_resource_kind kind = A2S_RESOURCE_BUFFER;
    a2s_constant_type type = A2S_CONSTANT_TYPE_FLOAT;
    uint64_t features = A2S_FEATURE_BUFFER_DEVICE_ADDRESS | A2S_FEATURE_SHADER_FLOAT16;
    uint32_t stage_mask = A2S_STAGE_BIT(A2S_STAGE_VERTEX) | A2S_STAGE_BIT(A2S_STAGE_FRAGMENT);

    C_CHECK(a2s_get_api_version() == A2S_API_VERSION);
    C_CHECK(a2s_get_version_string() != NULL);
    C_CHECK(strcmp(a2s_status_to_string(A2S_OK), "A2S_OK") == 0);
    C_CHECK(stage == 3 && kind == 1 && type == 12 && features != 0 && stage_mask == 6u);
    C_CHECK(sizeof(a2s_status) == 4);

    a2s_context_desc_init(&desc);
    C_CHECK(desc.struct_size == sizeof(desc));
    C_CHECK(a2s_context_create(&desc, &context) == A2S_OK);
    C_CHECK(context != NULL);

    a2s_compile_options_init(&options);
    options.features = features;
    a2s_input_init(&input);
    memset(&constant, 0, sizeof(constant));
    constant.index = 0;
    constant.type = A2S_CONSTANT_TYPE_UINT32;
    constant.component_count = 1;
    input.data = a2s_fixture_minimal_bc;
    input.size = sizeof(a2s_fixture_minimal_bc);
    input.entry_point = "main0";
    input.options = &options;
    input.function_constants = &constant;
    input.function_constant_count = 1;

    status = a2s_compile(context, &input, &output);
    C_CHECK(status == A2S_ERROR_UNIMPLEMENTED);
    C_CHECK(output != NULL);
    C_CHECK(a2s_output_get_status(output) == status);
    C_CHECK(strstr(a2s_output_get_diagnostics(output), "error: ") != NULL);
    C_CHECK(a2s_output_get_spirv(output, &word_count) == NULL && word_count == 0);

    memset(&reflection, 0, sizeof(reflection));
    reflection.struct_size = sizeof(reflection);
    C_CHECK(a2s_output_get_reflection(output, &reflection) == A2S_ERROR_UNIMPLEMENTED);
    memset(&binding, 0, sizeof(binding));
    binding.struct_size = sizeof(binding);
    C_CHECK(a2s_output_get_binding(output, 0, &binding) == A2S_ERROR_UNIMPLEMENTED);

    a2s_output_free(output);
    a2s_context_destroy(context);

    if (failures != 0) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    printf("[ OK ] compiles_and_links_as_c99\n");
    return 0;
}
