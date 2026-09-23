#!/usr/bin/env bash

set -euo pipefail

case "${1:-all}" in
    all|--tpch-only) ;;
    *) echo "Usage: $0 [--tpch-only]" >&2; exit 2 ;;
esac

cd "$(dirname "${BASH_SOURCE[0]}")/test_datasets"

if [ ! -f tpch-dbgen/s1/customer.tbl ]; then
    unzip -n tpch-dbgen.zip
    cd tpch-dbgen
    make -f Makefile dbgen
    ./dbgen -f -s 1 && mkdir -p s1 && mv *.tbl s1
    cd ..
fi

if [ "${1:-all}" != --tpch-only ] && [ ! -f hits_0.parquet ]; then
    wget https://datasets.clickhouse.com/hits_compatible/athena_partitioned/hits_0.parquet
fi
