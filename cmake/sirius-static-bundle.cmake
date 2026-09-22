# Record the dependency graph after evaluating configuration-specific
# interfaces.
function(sirius_record_bundle_dependency target directory)
  get_property(visited GLOBAL PROPERTY SIRIUS_BUNDLE_TARGETS)
  if(target IN_LIST visited)
    return()
  endif()
  set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_TARGETS "${target}")
  get_target_property(type "${target}" TYPE)
  set(artifact "")
  if(type MATCHES "^(STATIC|SHARED|UNKNOWN)_LIBRARY$")
    set(artifact "$<TARGET_FILE:${target}>")
  elseif(type STREQUAL "OBJECT_LIBRARY")
    set(artifact "$<TARGET_OBJECTS:${target}>")
  endif()
  if(artifact)
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_FILES "${artifact}")
  endif()
  set(links "")
  foreach(property LINK_LIBRARIES INTERFACE_LINK_LIBRARIES
                   INTERFACE_LINK_LIBRARIES_DIRECT)
    get_target_property(value "${target}" "${property}")
    if(value)
      list(APPEND links ${value})
    endif()
  endforeach()
  # Visit target names inside generator expressions as well as plain items.
  string(REGEX REPLACE "\\$<[A-Za-z_0-9]+:" "" target_names "${links}")
  string(REGEX MATCHALL "[A-Za-z_][A-Za-z_0-9:+.-]*" candidates
               "${target_names}")
  foreach(dependency IN LISTS candidates)
    if(TARGET "${dependency}")
      sirius_record_bundle_dependency("${dependency}" "${directory}")
    endif()
  endforeach()
  # Link features control final linking, not membership of a combined archive.
  string(REPLACE "$<LINK_ONLY:" "$<1:" links "${links}")
  string(REGEX REPLACE "\\$<LINK_(LIBRARY|GROUP):[^,>]+," "$<1:" links
                       "${links}")
  string(SHA256 key "${target}")
  set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_FILES
                                      "${directory}/$<CONFIG>/${key}.txt")
  file(
    GENERATE
    OUTPUT "${directory}/$<CONFIG>/${key}.txt"
    CONTENT
      "name=${target}\nfile=${artifact}\nlinks=$<TARGET_GENEX_EVAL:${target},${links}>\n"
  )
endfunction()

function(sirius_add_static_bundle target output)
  find_package(Python3 REQUIRED COMPONENTS Interpreter)
  set(directory "${CMAKE_CURRENT_BINARY_DIR}/${target}-dependencies")
  set(script "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/../scripts/combine-static.py")
  set_property(GLOBAL PROPERTY SIRIUS_BUNDLE_TARGETS "")
  set_property(GLOBAL PROPERTY SIRIUS_BUNDLE_FILES "")
  foreach(dependency IN LISTS ARGN)
    sirius_record_bundle_dependency("${dependency}" "${directory}")
  endforeach()
  get_property(files GLOBAL PROPERTY SIRIUS_BUNDLE_FILES)
  add_custom_command(
    OUTPUT "${output}" "${output}.cmake" "${output}.json"
    COMMAND "${Python3_EXECUTABLE}" "${script}" --ar "${CMAKE_AR}" --graph
            "${directory}/$<CONFIG>" --output "${output}" --roots ${ARGN}
    DEPENDS ${ARGN} ${files} "${script}"
    VERBATIM)
  add_custom_target(${target} ALL DEPENDS "${output}" "${output}.cmake"
                                          "${output}.json")
endfunction()
