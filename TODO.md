Add support for:

- `Tool.outputSchema`
- `CallToolResult.structuredContent`

## Motivation and Context

Implements https://github.com/modelcontextprotocol/rust-sdk/issues/312

First step toward MCP 2025-06-18 support.

## How Has This Been Tested?

Comprehensive unit tests for the new structured output features we implemented. The tests cover:

  - `CallToolResult::structured()` and `CallToolResult::structured_error()` methods
  - Tool `output_schema` field functionality
  - `IntoCallToolResult` trait implementation for `Structured<T>`
  - Mutual exclusivity validation between `content` and `structured_content`
  - Schema generation and serialization/deserialization

  The tests are located in `tests/test_structured_output.rs` and provide good coverage of the core functionality we added.

## Breaking Changes

Both `Tool.outputSchema` and `CallToolResult.structuredContent` are optional.

The only breaking change being that `CallToolResult.content` is now optional to support mutual exclusivity with `structured_content`.

## Types of changes

- [ ] Bug fix (non-breaking change which fixes an issue)
- [X] New feature (non-breaking change which adds functionality)
- [x] Breaking change (fix or feature that would cause existing functionality to change)
- [ ] Documentation update

## Checklist

- [X] I have read the [MCP Documentation](https://modelcontextprotocol.io)
- [X] My code follows the repository's style guidelines
- [x] New and existing tests pass locally
- [x] I have added appropriate error handling
- [x] I have added or updated documentation as needed

## Additional context

None for now.

## Task List

### Core Data Structures
- [x] Add `output_schema: Option<Arc<JsonObject>>` field to Tool struct
- [x] Add `structured_content: Option<Value>` field to CallToolResult struct
- [x] Implement validation for mutually exclusive content/structuredContent fields
- [x] Add `CallToolResult::structured()` constructor method
- [x] Add `CallToolResult::structured_error()` constructor method

### Macro Support
- [x] Parse function return types in #[tool] macro to generate output schemas
- [x] Support explicit `output_schema` attribute for manual schema specification
- [x] Generate schema using schemars for structured return types
- [x] Store output schema in generated tool metadata
- [x] Update tool_attr generation to include output_schema

### Type Conversion Infrastructure
- [x] Create `Structured<T>` wrapper type for structured results
- [x] Implement `IntoCallToolResult` for `Structured<T>`
- [x] Implement `IntoCallToolResult` for types that should produce structured content
- [x] Add automatic JSON serialization for structured types
- [x] Implement schema validation in conversion logic

### Tool Handler Updates
- [x] Update tool invocation to check for output_schema
- [x] Implement validation of structured output against schema
- [x] Handle conversion between Rust types and JSON values
- [x] Update error propagation for validation failures
- [x] Cache output schemas similar to input schemas
- [x] Update tool listing to include output schemas

### Testing
- [x] Test Tool serialization/deserialization with output_schema
- [x] Test CallToolResult with structured_content
- [x] Test mutual exclusivity validation
- [x] Test schema validation for structured outputs
- [x] Test #[tool] macro with various return types
- [x] Test error cases (schema violations, invalid types)
- [x] Test backward compatibility with existing tools
- [x] Add integration tests for end-to-end scenarios

### Documentation and Examples
- [x] Document Tool.outputSchema field usage
- [x] Document CallToolResult.structuredContent usage
- [x] Create example: simple tool with structured output
- [x] Create example: complex nested structures
- [x] Create example: error handling with structured content
- [ ] Write migration guide for existing tools
- [x] Update API documentation
- [x] Add inline code documentation

### Validation Improvements
- [x] Enforce structured_content usage when output_schema is defined
- [x] Forbid content field when output_schema is present
- [x] Ensure errors also use structured_content for tools with output_schema
- [x] Add comprehensive validation tests for the new strict behavior
- [ ] Update IntoCallToolResult implementations for consistent error handling

## Technical Considerations

### Backward Compatibility
- All changes must be backward compatible
- Tools without output_schema continue to work as before
- Clients that don't understand structured_content can still use content field

### Performance
- Schema generation should be cached
- Validation should be efficient
- Consider lazy evaluation where appropriate

### Error Handling
- Clear error messages for schema violations
- Proper error propagation through the macro system
- Graceful degradation when schemas can't be generated

## Dependencies
- schemars 1.0 for schema generation
- serde_json for JSON manipulation
- Existing MCP types and traits

## Timeline Estimate
- Core data structure updates: 2-3 hours
- Macro enhancements: 4-6 hours
- Type conversion and validation: 3-4 hours
- Testing: 3-4 hours
- Documentation: 2-3 hours

Total estimated time: 14-20 hours

## References
- [MCP Specification](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-06-18/schema.json)
- [PR #371: RFC for structured tool output](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/371)
- [Issue #312: Structured tool output + schema](https://github.com/modelcontextprotocol/rust-sdk/issues/312)

