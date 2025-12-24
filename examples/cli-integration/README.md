# AI CLI Integration Examples and Guide

This directory contains examples and documentation for integrating Rust MCP servers with AI CLI tools like Claude Code CLI, Codex CLI, and Gemini CLI.

## Contents

### Examples
- **[file_operations_stdio.rs](../servers/file_operations_stdio.rs)** - Complete file operations MCP server example
- **[Cargo.toml](../servers/Cargo.toml)** - Example project configuration

### Documentation
- **[CLI_INTEGRATION_GUIDE.md](CLI_INTEGRATION_GUIDE.md)** - Comprehensive guide for CLI integration

### Scripts
- **[scripts/claude-wrapper.sh](scripts/claude-wrapper.sh)** - Template wrapper script for Claude Code CLI
- **[scripts/gemini-wrapper.sh](scripts/gemini-wrapper.sh)** - Template wrapper script for Gemini CLI  
- **[scripts/setup-cli-integration.sh](scripts/setup-cli-integration.sh)** - Automated setup script for all CLI tools

## Quick Start

1. **Build the file operations example:**
   ```bash
   cargo build --release
   ```

2. **Set up CLI integration:**
   ```bash
   ./scripts/setup-cli-integration.sh file-ops ./target/release/file-operations-mcp
   ```

3. **Add to your preferred CLI tool:**
   ```bash
   # Claude Code CLI
   claude mcp add file-ops ./file-ops-claude.sh
   
   # Codex CLI  
   codex mcp add file-ops -- ./target/release/file-operations-mcp
   
   # Gemini CLI
   gemini mcp add file-ops ./file-ops-gemini.sh
   ```

4. **Test the integration:**
   ```
   "What tools do you have available?"
   "Read the contents of Cargo.toml"
   "Write 'Hello from MCP!' to a file called test.txt"
   ```

## What This Adds to the Rust SDK

The official Rust MCP SDK is excellent but was missing practical examples and documentation for real-world AI CLI integration. This contribution adds:

### 1. Practical File Operations Example
- Real-world tools that AI assistants actually need (file I/O, command execution)
- Proper error handling and user-friendly error messages
- Security considerations and input validation examples

### 2. Comprehensive CLI Integration Guide
- Step-by-step setup instructions for all major AI CLI tools
- Troubleshooting section with common issues and solutions
- Manual testing procedures for debugging
- Security considerations and best practices

### 3. Ready-to-Use Scripts and Templates
- Wrapper script templates for CLI tools that need them
- Automated setup script for streamlined integration
- Example Cargo.toml with correct dependencies

### 4. Missing Documentation
- How different CLI tools handle MCP server integration
- Why wrapper scripts are needed for some CLI tools
- Real-world testing procedures
- Performance and security considerations

## Benefits for the Community

1. **Faster Adoption** - Developers can quickly set up working AI tool integrations
2. **Reduced Support Burden** - Common integration issues are documented with solutions
3. **Best Practices** - Shows proper error handling, logging, and security considerations
4. **Real-World Examples** - File operations are what developers actually need for AI tools

## Testing

The file operations example has been tested with:
- ✅ Claude Code CLI - Working with wrapper script
- ✅ Codex CLI - Working with direct binary execution
- ✅ Gemini CLI - Working with wrapper script

All three tools can successfully:
- Initialize the MCP connection
- List available tools
- Execute file read/write operations
- Execute shell commands
- Handle errors gracefully

## Contributing

To contribute to this or make improvements:
1. Test with additional AI CLI tools
2. Add more practical examples (database operations, API calls, etc.)
3. Improve error handling and security considerations
4. Enhance documentation with more troubleshooting scenarios