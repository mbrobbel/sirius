vcpkg_check_linkage(ONLY_STATIC_LIBRARY)

vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO rapidsai/rmm
    REF branch-25.10
    SHA512 ed3e150df9220cf08d8302c225c2b2bb43d0ae656f15c36e28af0c6e80142d1432431106bb870405f0e142ea7a9c7f7ba975c7bd2b8c8920971074c65dc02d2f
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_CMAKE_PATH
    REPO rapidsai/rapids-cmake
    REF v25.10.00
    SHA512 30e36f73a81a2b71137401835a322279c4f2d03d21a8c1e03126deb6a35dae51476a06cc40d0f719db2ea53db656d4bf10dfe3a9264856ae571e67c4b2b63cc2
    HEAD_REF branch-25.10
)

vcpkg_from_github(
    OUT_SOURCE_PATH RAPIDS_LOGGER_PATH
    REPO rapidsai/rapids-logger
    REF 46070bb255482f0782ca840ae45de9354380e298
    SHA512 f9ac098cce0339c0a958e5a476b2a7461bb68ef1fe702d7b1bd2a427a0decf2b5e87ac195297933fa6c838c9cebfdc0bc8de3f79df75a1baa9c0e85f03ce51ae
    HEAD_REF branch-25.10
)

# Patch rapids_logger to use vcpkg's spdlog::spdlog target instead of spdlog
vcpkg_replace_string("${RAPIDS_LOGGER_PATH}/CMakeLists.txt"
    "set_target_properties(spdlog PROPERTIES POSITION_INDEPENDENT_CODE ON)"
    "set_target_properties(spdlog::spdlog PROPERTIES POSITION_INDEPENDENT_CODE ON)"
)

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}/cpp"
    OPTIONS
        -DFETCHCONTENT_SOURCE_DIR_RAPIDS-CMAKE=${RAPIDS_CMAKE_PATH}
        -DCPM_rapids_logger_SOURCE=${RAPIDS_LOGGER_PATH}
        -DBUILD_TESTS=OFF
        -DBUILD_BENCHMARKS=OFF
        -DCMAKE_CUDA_ARCHITECTURES=RAPIDS
)

vcpkg_cmake_install()

# rapids_logger cmake config is generated but installed to a non-standard location
# We need to manually install it
file(GLOB RAPIDS_LOGGER_CMAKE_FILES
    "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/_deps/rapids_logger-build/rapids_logger-*.cmake"
    "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/_deps/rapids_logger-build/CMakeFiles/Export/*/rapids_logger-targets*.cmake"
    "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/_deps/rapids_logger-build/create_logger_macros.cmake"
)
file(INSTALL ${RAPIDS_LOGGER_CMAKE_FILES} DESTINATION "${CURRENT_PACKAGES_DIR}/share/rapids_logger")

# Fix paths in rapids_logger cmake config
file(READ "${CURRENT_PACKAGES_DIR}/share/rapids_logger/rapids_logger-config.cmake" _config_content)
string(REPLACE "${CURRENT_BUILDTREES_DIR}/${TARGET_TRIPLET}-rel/_deps/rapids_logger-build" "\${CMAKE_CURRENT_LIST_DIR}/../.." _config_content "${_config_content}")
file(WRITE "${CURRENT_PACKAGES_DIR}/share/rapids_logger/rapids_logger-config.cmake" "${_config_content}")

# Fix rapids_logger-targets.cmake - change from 4 parent dirs to 3 (share/rapids_logger -> package root)
# The original file goes up 4 directories (assuming lib/cmake/rapids_logger/ layout)
# But vcpkg uses share/rapids_logger/, so we need to go up 3 directories
# Delete line 53 which is one extra get_filename_component call
execute_process(
    COMMAND sed -i "53d" "${CURRENT_PACKAGES_DIR}/share/rapids_logger/rapids_logger-targets.cmake"
)

vcpkg_cmake_config_fixup(
    PACKAGE_NAME rmm
    CONFIG_PATH lib/cmake/rmm
)

file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include")
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/share")

vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
