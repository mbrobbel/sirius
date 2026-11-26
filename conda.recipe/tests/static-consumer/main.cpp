#include <sirius-c/version.h>

int main() { return sirius_abi_version() == SIRIUS_ABI_VERSION ? 0 : 1; }
