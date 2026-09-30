cmake_minimum_required(VERSION 3.31 FATAL_ERROR)

project(sirius-duckdb)

find_package(sirius CONFIG REQUIRED COMPONENTS static)

set(EXTENSION_SOURCES src/sirius/sirius_extension.cpp)

build_static_extension(sirius ${EXTENSION_SOURCES})
build_loadable_extension(sirius CPP ${EXTENSION_SOURCES})

# Keep the bundled compiler runtimes visible for exceptions crossing into
# DuckDB.
get_target_property(sirius_link_libraries sirius_loadable_extension
                    LINK_LIBRARIES)
list(REMOVE_ITEM sirius_link_libraries "-Wl,--exclude-libs,ALL")
set_property(TARGET sirius_loadable_extension
             PROPERTY LINK_LIBRARIES "${sirius_link_libraries}")
target_link_options(
  sirius_loadable_extension
  PRIVATE
  "LINKER:--exclude-libs,libduckdb_static.a:libdummy_static_extension_loader.a:libsirius.a"
)

foreach(target IN ITEMS sirius_extension sirius_loadable_extension)
  target_link_libraries(${target} sirius::sirius_static)
  set_target_properties(${target} PROPERTIES CXX_STANDARD 20
                                             CXX_STANDARD_REQUIRED ON)
  target_include_directories(${target}
                             PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/src/include)
endforeach()

install(
  TARGETS sirius_extension
  EXPORT "${DUCKDB_EXPORT_SET}"
  LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
  ARCHIVE DESTINATION "${INSTALL_LIB_DIR}")
