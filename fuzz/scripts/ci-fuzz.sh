#!/usr/bin/env bash
# CI fuzzing script for OxiMedia
# Runs all fuzz targets for a limited time to detect crashes

set -euo pipefail

# Configuration
FUZZ_TIME="${FUZZ_TIME:-300}"  # 5 minutes per target by default
MEMORY_LIMIT="${MEMORY_LIMIT:-2048}"  # 2GB memory limit
JOBS="${JOBS:-1}"  # Number of parallel jobs
ARTIFACTS_DIR="${ARTIFACTS_DIR:-artifacts}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Change to fuzz directory
cd "$(dirname "$0")/.."

echo "========================================="
echo "OxiMedia Fuzzing Test Suite"
echo "========================================="
echo "Time per target: ${FUZZ_TIME}s"
echo "Memory limit: ${MEMORY_LIMIT}MB"
echo "Jobs: ${JOBS}"
echo ""

# Get list of all fuzz targets
TARGETS=$(cargo fuzz list)

if [ -z "$TARGETS" ]; then
    echo -e "${RED}ERROR: No fuzz targets found${NC}"
    exit 1
fi

echo "Found $(echo "$TARGETS" | wc -w) fuzz targets"
echo ""

# Track results
TOTAL=0
PASSED=0
FAILED=0
FAILED_TARGETS=""

# Run each target
for target in $TARGETS; do
    TOTAL=$((TOTAL + 1))
    echo -e "${YELLOW}Running fuzzer: $target${NC}"

    # Determine dictionary file if it exists
    DICT_ARG=""
    case "$target" in
        matroska_parser)
            if [ -f "dictionaries/matroska.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/matroska.dict"
            fi
            ;;
        ogg_parser)
            if [ -f "dictionaries/ogg.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/ogg.dict"
            fi
            ;;
        av1_decoder)
            if [ -f "dictionaries/av1.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/av1.dict"
            fi
            ;;
        vp8_decoder)
            if [ -f "dictionaries/vp8_decoder.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/vp8_decoder.dict"
            fi
            ;;
        hls_parser)
            if [ -f "dictionaries/hls.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/hls.dict"
            fi
            ;;
        dash_parser)
            if [ -f "dictionaries/dash.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/dash.dict"
            fi
            ;;
        rtmp_parser)
            if [ -f "dictionaries/rtmp.dict" ]; then
                DICT_ARG="-- -dict=dictionaries/rtmp.dict"
            fi
            ;;
    esac

    # Run the fuzzer
    if cargo fuzz run "$target" $DICT_ARG \
        -- -max_total_time="$FUZZ_TIME" \
           -rss_limit_mb="$MEMORY_LIMIT" \
           -jobs="$JOBS" \
           -print_final_stats=1; then
        echo -e "${GREEN}✓ $target passed${NC}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}✗ $target failed or found crashes${NC}"
        FAILED=$((FAILED + 1))
        FAILED_TARGETS="$FAILED_TARGETS $target"

        # Check if artifacts were created
        if [ -d "$ARTIFACTS_DIR/$target" ]; then
            CRASH_COUNT=$(find "$ARTIFACTS_DIR/$target" -type f | wc -l)
            if [ "$CRASH_COUNT" -gt 0 ]; then
                echo -e "${RED}  Found $CRASH_COUNT crash(es) in $ARTIFACTS_DIR/$target${NC}"
            fi
        fi
    fi
    echo ""
done

# Print summary
echo "========================================="
echo "Fuzzing Summary"
echo "========================================="
echo "Total targets: $TOTAL"
echo -e "${GREEN}Passed: $PASSED${NC}"
if [ "$FAILED" -gt 0 ]; then
    echo -e "${RED}Failed: $FAILED${NC}"
    echo "Failed targets:$FAILED_TARGETS"
else
    echo "Failed: $FAILED"
fi
echo ""

# Exit with error if any target failed
if [ "$FAILED" -gt 0 ]; then
    echo -e "${RED}Fuzzing detected issues. Please investigate.${NC}"
    exit 1
else
    echo -e "${GREEN}All fuzz targets passed!${NC}"
    exit 0
fi
