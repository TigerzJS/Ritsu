add_executable(air2spirv-cli main.cpp)
target_link_libraries(air2spirv-cli PRIVATE air2spirv::air2spirv air2spirv_warnings)
