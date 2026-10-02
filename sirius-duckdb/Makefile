PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

DISABLE_SANITIZER ?= 1

SIRIUS_DUCKDB_LINKAGE ?= shared
override EXT_FLAGS += -DSIRIUS_DUCKDB_LINKAGE=$(SIRIUS_DUCKDB_LINKAGE)
ifeq ($(SIRIUS_DUCKDB_LINKAGE),static)
export VCPKG_CUDA_ARCHITECTURES ?= $(CUDAARCHS)
override EXT_FLAGS += -DVCPKG_MANIFEST_FEATURES=static
endif

EXT_NAME=sirius
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

GEN?=ninja
ifeq ($(GEN),ninja)
GENERATOR=-GNinja -DFORCE_COLORED_OUTPUT=1
endif

include extension-ci-tools/makefiles/duckdb_extension.Makefile
