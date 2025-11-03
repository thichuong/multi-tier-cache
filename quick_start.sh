#!/bin/bash
# Quick Start Script for multi-tier-cache
# This script helps you get started with the library quickly

set -e

echo "================================================"
echo "   multi-tier-cache Quick Start Script"
echo "================================================"
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if Redis is running
echo -n "Checking Redis... "
if redis-cli ping > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Redis is running${NC}"
else
    echo -e "${RED}✗ Redis is not running${NC}"
    echo ""
    echo "Please start Redis first:"
    echo "  sudo systemctl start redis    # Fedora/RHEL/CentOS"
    echo "  brew services start redis     # macOS"
    echo "  sudo service redis start      # Debian/Ubuntu"
    echo ""
    exit 1
fi

# Check Rust installation
echo -n "Checking Rust... "
if command -v cargo &> /dev/null; then
    RUST_VERSION=$(rustc --version | cut -d' ' -f2)
    echo -e "${GREEN}✓ Rust $RUST_VERSION${NC}"
else
    echo -e "${RED}✗ Rust is not installed${NC}"
    echo ""
    echo "Please install Rust from: https://rustup.rs/"
    exit 1
fi

echo ""
echo "================================================"
echo "   Running Examples"
echo "================================================"
echo ""

# Function to run example
run_example() {
    local example=$1
    local description=$2

    echo -e "${YELLOW}► Running: $example${NC}"
    echo "   $description"
    echo ""

    if timeout 15s cargo run --example "$example" 2>&1; then
        echo -e "${GREEN}✓ $example completed${NC}"
    else
        echo -e "${RED}✗ $example failed or timed out${NC}"
    fi
    echo ""
    echo "---"
    echo ""
}

# Run all examples
run_example "basic_usage" "Basic cache operations"
run_example "cache_strategies" "Different caching strategies"
run_example "health_monitoring" "Health checks and statistics"

echo ""
echo "================================================"
echo "   Next Steps"
echo "================================================"
echo ""
echo "1. Read the documentation:"
echo "   cargo doc --open"
echo ""
echo "2. Try more examples:"
echo "   cargo run --example stampede_protection"
echo "   cargo run --example redis_streams"
echo "   cargo run --example advanced_usage"
echo ""
echo "3. Run tests:"
echo "   cargo test"
echo ""
echo "4. Build for production:"
echo "   cargo build --release"
echo ""
echo "5. Read README.md for detailed usage"
echo ""
echo "Happy caching! 🚀"
