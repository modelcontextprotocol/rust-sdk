#!/bin/bash
# Gemini CLI wrapper script for Rust MCP servers  
# Usage: Save this as your-server-name.sh and make executable

# Set debug logging if needed (uncomment the next line)
# export RUST_LOG=debug

# Change to the directory containing your binary
cd "$(dirname "$0")"

# Execute the MCP server
# Replace './target/release/your-server-name' with the actual path to your binary
exec ./target/release/your-server-name