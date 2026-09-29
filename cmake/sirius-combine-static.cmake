cmake_minimum_required(VERSION 3.30.4)

function(sirius_visit_bundle_item item)
  get_property(seen GLOBAL PROPERTY SIRIUS_BUNDLE_SEEN)
  if(NOT item
     OR item IN_LIST seen
     OR item MATCHES "^::@")
    return()
  endif()
  set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_SEEN "${item}")
  string(SHA256 key "${item}")
  set(record "${GRAPH}/${key}.cmake")
  if(EXISTS "${record}")
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_INPUTS "${record}")
    include("${record}")
    foreach(dependency IN LISTS artifact links)
      sirius_visit_bundle_item("${dependency}")
    endforeach()
    return()
  endif()

  set(name "${item}")
  if(IS_ABSOLUTE "${item}")
    get_filename_component(name "${item}" NAME)
    string(REGEX REPLACE "^lib|\\.so(\\..*)?$|\\.a$" "" name "${name}")
  else()
    string(REGEX REPLACE "^-l" "" name "${name}")
  endif()
  set(platform
      c
      m
      dl
      rt
      pthread
      util
      resolv
      atomic
      gcc_s
      stdc++
      stdc++fs
      cuda
      nvidia-ml)
  if(name IN_LIST platform)
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_SYSTEM "${name}")
  elseif(
    IS_ABSOLUTE "${item}"
    AND item MATCHES "\\.(a|o)$"
    AND EXISTS "${item}")
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_ARTIFACTS "${item}")
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_INPUTS "${item}")
  elseif(item STREQUAL "-pthread")
    set_property(GLOBAL APPEND PROPERTY SIRIUS_BUNDLE_SYSTEM pthread)
  elseif(NOT item MATCHES "^-Wl,--(no-)?as-needed$")
    message(
      FATAL_ERROR
        "Non-static or unsupported dependency in Sirius archive: ${item}")
  endif()
endfunction()

include("${GRAPH}/roots.cmake")
foreach(root IN LISTS ROOTS)
  sirius_visit_bundle_item("${root}")
endforeach()
get_property(artifacts GLOBAL PROPERTY SIRIUS_BUNDLE_ARTIFACTS)
get_property(inputs GLOBAL PROPERTY SIRIUS_BUNDLE_INPUTS)
get_property(system GLOBAL PROPERTY SIRIUS_BUNDLE_SYSTEM)
if(NOT artifacts)
  message(FATAL_ERROR "Cannot create an empty Sirius archive")
endif()
list(REMOVE_DUPLICATES artifacts)
list(REMOVE_DUPLICATES system)

set(rebuild FALSE)
if(NOT EXISTS "${OUTPUT}" OR NOT EXISTS "${OUTPUT}.cmake")
  set(rebuild TRUE)
endif()
list(APPEND inputs "${CMAKE_CURRENT_LIST_FILE}" "${GRAPH}/roots.cmake")
foreach(input IN LISTS inputs)
  if("${input}" IS_NEWER_THAN "${OUTPUT}")
    set(rebuild TRUE)
  endif()
endforeach()
if(NOT rebuild)
  return()
endif()

# MRI preserves duplicate member names. Local symlinks avoid path quoting.
set(work "${OUTPUT}.work")
file(REMOVE_RECURSE "${work}")
file(MAKE_DIRECTORY "${work}")
set(mri "CREATE combined.a\n")
list(REVERSE artifacts)
set(index 0)
foreach(artifact IN LISTS artifacts)
  file(CREATE_LINK "${artifact}" "${work}/input-${index}" SYMBOLIC)
  if(artifact MATCHES "\\.a$")
    string(APPEND mri "ADDLIB input-${index}\n")
  else()
    string(APPEND mri "ADDMOD input-${index}\n")
  endif()
  math(EXPR index "${index} + 1")
endforeach()
string(APPEND mri "SAVE\nEND\n")
file(WRITE "${work}/combine.mri" "${mri}")
execute_process(
  COMMAND "${ARCHIVER}" -M
  INPUT_FILE "${work}/combine.mri"
  WORKING_DIRECTORY "${work}" COMMAND_ERROR_IS_FATAL ANY)
execute_process(COMMAND "${ARCHIVER}" sD combined.a
                WORKING_DIRECTORY "${work}" COMMAND_ERROR_IS_FATAL ANY)
file(RENAME "${work}/combined.a" "${OUTPUT}")
file(WRITE "${OUTPUT}.cmake"
     "set(SIRIUS_STATIC_SYSTEM_LIBRARIES [==[${system}]==])\n")
file(REMOVE_RECURSE "${work}")
