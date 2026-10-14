// The single include point for the Khronos SPIRV-Headers C++ enums.
//
// SPV_ENABLE_UTILITY_CODE adds inline helpers (spv::HasResultAndType, ...). It changes which
// inline functions the header defines, so every translation unit must see the same setting:
// include the Khronos header only through this file.
#pragma once

#ifndef SPV_ENABLE_UTILITY_CODE
#define SPV_ENABLE_UTILITY_CODE 1
#endif

#include <spirv/unified1/spirv.hpp11>
