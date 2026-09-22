# Prepared source archives carry the configure-time and Cargo dependencies.
set(_sirius_vendor "${CMAKE_CURRENT_SOURCE_DIR}/packaging/vendor")
if(EXISTS "${_sirius_vendor}/corrosion/CMakeLists.txt")
  set(FETCHCONTENT_SOURCE_DIR_CORROSION "${_sirius_vendor}/corrosion")
  set(FETCHCONTENT_SOURCE_DIR_CUCO "${_sirius_vendor}/cuco")
  set(FETCHCONTENT_FULLY_DISCONNECTED ON)
endif()
