PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXT_NAME=sirius
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

GEN?=ninja
ifeq ($(GEN),ninja)
GENERATOR=-GNinja -DFORCE_COLORED_OUTPUT=1
endif

include extension-ci-tools/makefiles/duckdb_extension.Makefile
