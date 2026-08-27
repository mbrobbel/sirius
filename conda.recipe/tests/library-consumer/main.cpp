#include <sirius_ffi.hpp>

int main()
{
  auto name = sirius::ffi::stream_view_name(7);
  return name && *name == "sirius_stream_7" ? 0 : 1;
}
