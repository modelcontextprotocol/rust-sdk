#!/bin/bash
# Setup script for integrating a Rust MCP server with AI CLI tools
# Usage: ./setup-cli-integration.sh <server-name> <binary-path>

set -e

SERVER_NAME="${1:-my-mcp-server}"
BINARY_PATH="${2:-./target/release/my-mcp-server}"

if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    echo "Make sure to build your server first with 'cargo build --release'"
    exit 1
fi

echo "Setting up MCP server '$SERVER_NAME' for AI CLI tools..."

# Make binary executable
chmod +x "$BINARY_PATH"

# Create wrapper scripts for Claude Code CLI and Gemini CLI
cat > "${SERVER_NAME}-claude.sh" << EOF
#!/bin/bash
cd "\$(dirname "\$0")"
exec $BINARY_PATH
EOF

cat > "${SERVER_NAME}-gemini.sh" << EOF  
#!/bin/bash
cd "\$(dirname "\$0")"
exec $BINARY_PATH
EOF

# Make wrapper scripts executable
chmod +x "${SERVER_NAME}-claude.sh"
chmod +x "${SERVER_NAME}-gemini.sh"

echo "Created wrapper scripts:"
echo "  - ${SERVER_NAME}-claude.sh (for Claude Code CLI)"
echo "  - ${SERVER_NAME}-gemini.sh (for Gemini CLI)"
echo ""

# Check which CLI tools are available and provide setup commands
echo "Setup commands for available CLI tools:"

if command -v claude >/dev/null 2>&1; then
    echo ""
    echo "Claude Code CLI:"
    echo "  claude mcp add $SERVER_NAME ./${SERVER_NAME}-claude.sh"
fi

if command -v codex >/dev/null 2>&1; then
    echo ""
    echo "Codex CLI:"
    echo "  codex mcp add $SERVER_NAME -- $BINARY_PATH"
fi

if command -v gemini >/dev/null 2>&1; then
    echo ""
    echo "Gemini CLI:"
    echo "  gemini mcp add $SERVER_NAME ./${SERVER_NAME}-gemini.sh"
fi

echo ""
echo "To verify setup after running the commands above:"
echo "  claude mcp list | grep $SERVER_NAME"
echo "  codex mcp list | grep $SERVER_NAME"  
echo "  gemini mcp list | grep $SERVER_NAME"