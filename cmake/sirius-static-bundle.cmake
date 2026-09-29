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
  set(links "")
  foreach(property LINK_LIBRARIES INTERFACE_LINK_LIBRARIES
                   INTERFACE_LINK_LIBRARIES_DIRECT)
    get_target_property(value "${target}" "${property}")
    if(value)
      list(APPEND links ${value})
    endif()
  endforeach()
  # Discover possible targets; the generated records select active branches.
  string(REGEX REPLACE "\\$<[A-Za-z_0-9]+:" "" target_names "${links}")
  string(REGEX MATCHALL "[A-Za-z_][A-Za-z_0-9:+.-]*" candidates
               "${target_names}")
  foreach(dependency IN LISTS candidates)
    if(TARGET "${dependency}")
      sirius_record_bundle_dependency("${dependency}" "${directory}")
    endif()
  endforeach()
  # Archive membership does not depend on final-link features.
  string(REPLACE "$<LINK_ONLY:" "$<1:" links "${links}")
  string(REPLACE "$<COMPILE_ONLY:" "$<0:" links "${links}")
  string(REGEX REPLACE "\\$<LINK_(LIBRARY|GROUP):[^,>]+," "$<1:" links
                       "${links}")
  string(SHA256 key "${target}")
  file(
    GENERATE
    OUTPUT "${directory}/$<CONFIG>/${key}.cmake"
    CONTENT
      "set(artifact [==[${artifact}]==])\nset(links [==[$<TARGET_GENEX_EVAL:${target},${links}>]==])\n"
  )
endfunction()

function(sirius_add_static_bundle target output)
  set(directory "${CMAKE_CURRENT_BINARY_DIR}/${target}-dependencies")
  set(script "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/sirius-combine-static.cmake")
  set_property(GLOBAL PROPERTY SIRIUS_BUNDLE_TARGETS "")
  foreach(dependency IN LISTS ARGN)
    sirius_record_bundle_dependency("${dependency}" "${directory}")
  endforeach()
  file(
    GENERATE
    OUTPUT "${directory}/$<CONFIG>/roots.cmake"
    CONTENT "set(ROOTS [==[${ARGN}]==])\n")
  # The script checks input timestamps before merging. Depending on the roots
  # lets CMake build only the dependencies selected for this configuration.
  add_custom_target(
    ${target} ALL
    COMMAND "${CMAKE_COMMAND}" "-DARCHIVER=${CMAKE_AR}"
            "-DGRAPH=${directory}/$<CONFIG>" "-DOUTPUT=${output}" -P "${script}"
    BYPRODUCTS "${output}" "${output}.cmake"
    DEPENDS ${ARGN} "${script}"
    VERBATIM)
endfunction()
