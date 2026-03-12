#!/bin/bash

# Simple script to run benchmarks for multi-tier-cache

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== Multi-Tier-Cache Benchmark Runner ===${NC}"

# Function to display help
show_help() {
    echo "Usage: $0 [OPTION]... [BENCHNAME]..."
    echo "Run benchmarks for the multi-tier-cache project."
    echo
    echo "Options:"
    echo "  -h, --help      Show this help message"
    echo "  -l, --list      List available benchmarks"
    echo "  -q, --quiet     Run cargo bench with --quiet"
    echo "  -- [ARGS]...    Pass additional arguments to the benchmark binary"
    echo
    echo "Example:"
    echo "  $0              Run all benchmarks"
    echo "  $0 multi_tier   Run benchmarks matching 'multi_tier'"
}

# Parse options
QUIET=""
LIST_ONLY=false
CARGO_ARGS=""
BENCH_ARGS=""
PASSTHROUGH=false

while [[ $# -gt 0 ]]; do
    if [[ "$PASSTHROUGH" == "true" ]]; then
        BENCH_ARGS="$BENCH_ARGS $1"
        shift
        continue
    fi

    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        -l|--list)
            LIST_ONLY=true
            shift
            ;;
        -q|--quiet)
            QUIET="--quiet"
            shift
            ;;
        --)
            PASSTHROUGH=true
            shift
            ;;
        *)
            CARGO_ARGS="$CARGO_ARGS $1"
            shift
            ;;
    esac
done

if [[ "$LIST_ONLY" == "true" ]]; then
    echo -e "${YELLOW}Listing available benchmarks...${NC}"
    cargo bench -- --list
    exit 0
fi

echo -e "${GREEN}Running benchmarks...${NC}"
if [[ -n "$BENCH_ARGS" ]]; then
    cargo bench $QUIET $CARGO_ARGS -- $BENCH_ARGS
else
    cargo bench $QUIET $CARGO_ARGS
fi

echo -e "${BLUE}=== Benchmarks Completed ===${NC}"
