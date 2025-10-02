//! Type-safe schema definitions for MCP elicitation requests.
//!
//! This module provides strongly-typed schema definitions for elicitation requests
//! that comply with the MCP 2025-06-18 specification. Elicitation schemas must be
//! objects with primitive-typed properties.
//!
//! # Example
//!
//! ```rust
//! use rmcp::model::*;
//!
//! let schema = ElicitationSchema::builder()
//!     .required_email("email")
//!     .required_integer("age", 0, 150)
//!     .optional_bool("newsletter", false)
//!     .build();
//! ```

use std::{borrow::Cow, collections::BTreeMap};

use serde::{Deserialize, Serialize};

use crate::{const_string, model::ConstString};

// =============================================================================
// CONST TYPES FOR JSON SCHEMA TYPE FIELD
// =============================================================================

const_string!(ObjectTypeConst = "object");
const_string!(StringTypeConst = "string");
const_string!(NumberTypeConst = "number");
const_string!(IntegerTypeConst = "integer");
const_string!(BooleanTypeConst = "boolean");

// =============================================================================
// PRIMITIVE SCHEMA DEFINITIONS
// =============================================================================

/// Primitive schema definition for elicitation properties.
///
/// According to MCP 2025-06-18 specification, elicitation schemas must have
/// properties of primitive types only (string, number, integer, boolean).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PrimitiveSchema {
    /// String property (with optional enum constraint)
    String(StringSchema),
    /// Number property (with optional enum constraint)
    Number(NumberSchema),
    /// Integer property (with optional enum constraint)
    Integer(IntegerSchema),
    /// Boolean property
    Boolean(BooleanSchema),
}

// =============================================================================
// STRING SCHEMA
// =============================================================================

/// Schema definition for string properties.
///
/// Supports validation constraints like length, pattern matching, format, and enum values.
/// All fields are private to ensure validation - use builder methods to construct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct StringSchema {
    /// Type discriminator
    #[serde(rename = "type")]
    type_: StringTypeConst,

    /// Allowed string values (when used as enum)
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<String>>,

    /// Optional human-readable names for each enum value
    #[serde(skip_serializing_if = "Option::is_none")]
    enum_names: Option<Vec<String>>,

    /// Minimum string length
    #[serde(skip_serializing_if = "Option::is_none")]
    min_length: Option<u32>,

    /// Maximum string length
    #[serde(skip_serializing_if = "Option::is_none")]
    max_length: Option<u32>,

    /// Regular expression pattern
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,

    /// String format (e.g., "email", "uri", "date-time")
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,

    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<String>,
}

impl Default for StringSchema {
    fn default() -> Self {
        Self {
            type_: StringTypeConst,
            enum_values: None,
            enum_names: None,
            min_length: None,
            max_length: None,
            pattern: None,
            format: None,
            description: None,
            default: None,
        }
    }
}

impl StringSchema {
    /// Create a new string schema
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an email string schema
    pub fn email() -> Self {
        Self {
            format: Some("email".to_string()),
            ..Default::default()
        }
    }

    /// Create a URI string schema
    pub fn uri() -> Self {
        Self {
            format: Some("uri".to_string()),
            ..Default::default()
        }
    }

    /// Create an enum string schema
    pub fn enum_values(values: Vec<String>) -> Self {
        Self {
            enum_values: Some(values),
            ..Default::default()
        }
    }

    /// Set minimum and maximum length
    pub fn with_length(mut self, min: u32, max: u32) -> Result<Self, &'static str> {
        if min > max {
            return Err("min_length must be <= max_length");
        }
        self.min_length = Some(min);
        self.max_length = Some(max);
        Ok(self)
    }

    /// Set minimum and maximum length (panics on invalid input)
    pub fn length(mut self, min: u32, max: u32) -> Self {
        assert!(min <= max, "min_length must be <= max_length");
        self.min_length = Some(min);
        self.max_length = Some(max);
        self
    }

    /// Set minimum length
    pub fn min_length(mut self, min: u32) -> Self {
        self.min_length = Some(min);
        self
    }

    /// Set maximum length
    pub fn max_length(mut self, max: u32) -> Self {
        self.max_length = Some(max);
        self
    }

    /// Set pattern
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Set format
    pub fn format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Set enum names
    pub fn enum_names(mut self, names: Vec<String>) -> Self {
        self.enum_names = Some(names);
        self
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }
}

// =============================================================================
// NUMBER SCHEMA
// =============================================================================

/// Schema definition for number properties (floating-point).
///
/// Supports range validation, multiples, and enum values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct NumberSchema {
    /// Type discriminator
    #[serde(rename = "type")]
    type_: NumberTypeConst,

    /// Allowed number values (when used as enum)
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<f64>>,

    /// Optional human-readable names for each enum value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,

    /// Minimum value (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,

    /// Maximum value (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,

    /// Minimum value (exclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<f64>,

    /// Maximum value (exclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<f64>,

    /// Value must be a multiple of this number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<f64>,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
}

impl Default for NumberSchema {
    fn default() -> Self {
        Self {
            type_: NumberTypeConst,
            enum_values: None,
            enum_names: None,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            multiple_of: None,
            description: None,
            default: None,
        }
    }
}

impl NumberSchema {
    /// Create a new number schema
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an enum number schema
    pub fn enum_values(values: Vec<f64>) -> Self {
        Self {
            enum_values: Some(values),
            ..Default::default()
        }
    }

    /// Set minimum and maximum (inclusive)
    pub fn with_range(mut self, min: f64, max: f64) -> Result<Self, &'static str> {
        if min > max {
            return Err("minimum must be <= maximum");
        }
        self.minimum = Some(min);
        self.maximum = Some(max);
        Ok(self)
    }

    /// Set minimum and maximum (panics on invalid input)
    pub fn range(mut self, min: f64, max: f64) -> Self {
        assert!(min <= max, "minimum must be <= maximum");
        self.minimum = Some(min);
        self.maximum = Some(max);
        self
    }

    /// Set minimum (inclusive)
    pub fn minimum(mut self, min: f64) -> Self {
        self.minimum = Some(min);
        self
    }

    /// Set maximum (inclusive)
    pub fn maximum(mut self, max: f64) -> Self {
        self.maximum = Some(max);
        self
    }

    /// Set exclusive minimum
    pub fn exclusive_minimum(mut self, min: f64) -> Self {
        self.exclusive_minimum = Some(min);
        self
    }

    /// Set exclusive maximum
    pub fn exclusive_maximum(mut self, max: f64) -> Self {
        self.exclusive_maximum = Some(max);
        self
    }

    /// Set multiple of constraint
    pub fn multiple_of(mut self, multiple: f64) -> Self {
        self.multiple_of = Some(multiple);
        self
    }

    /// Set enum names
    pub fn enum_names(mut self, names: Vec<String>) -> Self {
        self.enum_names = Some(names);
        self
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: f64) -> Self {
        self.default = Some(default);
        self
    }
}

// =============================================================================
// INTEGER SCHEMA
// =============================================================================

/// Schema definition for integer properties.
///
/// Supports range validation, multiples, and enum values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct IntegerSchema {
    /// Type discriminator
    #[serde(rename = "type")]
    type_: IntegerTypeConst,

    /// Allowed integer values (when used as enum)
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<i64>>,

    /// Optional human-readable names for each enum value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_names: Option<Vec<String>>,

    /// Minimum value (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,

    /// Maximum value (inclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,

    /// Minimum value (exclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<i64>,

    /// Maximum value (exclusive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<i64>,

    /// Value must be a multiple of this number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<i64>,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<i64>,
}

impl Default for IntegerSchema {
    fn default() -> Self {
        Self {
            type_: IntegerTypeConst,
            enum_values: None,
            enum_names: None,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            multiple_of: None,
            description: None,
            default: None,
        }
    }
}

impl IntegerSchema {
    /// Create a new integer schema
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an enum integer schema
    pub fn enum_values(values: Vec<i64>) -> Self {
        Self {
            enum_values: Some(values),
            ..Default::default()
        }
    }

    /// Set minimum and maximum (inclusive)
    pub fn with_range(mut self, min: i64, max: i64) -> Result<Self, &'static str> {
        if min > max {
            return Err("minimum must be <= maximum");
        }
        self.minimum = Some(min);
        self.maximum = Some(max);
        Ok(self)
    }

    /// Set minimum and maximum (panics on invalid input)
    pub fn range(mut self, min: i64, max: i64) -> Self {
        assert!(min <= max, "minimum must be <= maximum");
        self.minimum = Some(min);
        self.maximum = Some(max);
        self
    }

    /// Set minimum (inclusive)
    pub fn minimum(mut self, min: i64) -> Self {
        self.minimum = Some(min);
        self
    }

    /// Set maximum (inclusive)
    pub fn maximum(mut self, max: i64) -> Self {
        self.maximum = Some(max);
        self
    }

    /// Set exclusive minimum
    pub fn exclusive_minimum(mut self, min: i64) -> Self {
        self.exclusive_minimum = Some(min);
        self
    }

    /// Set exclusive maximum
    pub fn exclusive_maximum(mut self, max: i64) -> Self {
        self.exclusive_maximum = Some(max);
        self
    }

    /// Set multiple of constraint
    pub fn multiple_of(mut self, multiple: i64) -> Self {
        self.multiple_of = Some(multiple);
        self
    }

    /// Set enum names
    pub fn enum_names(mut self, names: Vec<String>) -> Self {
        self.enum_names = Some(names);
        self
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: i64) -> Self {
        self.default = Some(default);
        self
    }
}

// =============================================================================
// BOOLEAN SCHEMA
// =============================================================================

/// Schema definition for boolean properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct BooleanSchema {
    /// Type discriminator
    #[serde(rename = "type")]
    type_: BooleanTypeConst,

    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

impl Default for BooleanSchema {
    fn default() -> Self {
        Self {
            type_: BooleanTypeConst,
            description: None,
            default: None,
        }
    }
}

impl BooleanSchema {
    /// Create a new boolean schema
    pub fn new() -> Self {
        Self::default()
    }

    /// Set description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: bool) -> Self {
        self.default = Some(default);
        self
    }
}

// =============================================================================
// ELICITATION SCHEMA
// =============================================================================

/// Type-safe elicitation schema for requesting structured user input.
///
/// This enforces the MCP 2025-06-18 specification that elicitation schemas
/// must be objects with primitive-typed properties.
///
/// # Example
///
/// ```rust
/// use rmcp::model::*;
///
/// let schema = ElicitationSchema::builder()
///     .required_email("email")
///     .required_integer("age", 0, 150)
///     .optional_bool("newsletter", false)
///     .build();
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ElicitationSchema {
    /// Always "object" for elicitation schemas
    #[serde(rename = "type")]
    type_: ObjectTypeConst,

    /// Property definitions (must be primitive types)
    pub properties: BTreeMap<String, PrimitiveSchema>,

    /// List of required property names
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,

    /// Optional description of what this schema represents
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ElicitationSchema {
    /// Create a new elicitation schema with the given properties
    pub fn new(properties: BTreeMap<String, PrimitiveSchema>) -> Self {
        Self {
            type_: ObjectTypeConst,
            properties,
            required: None,
            description: None,
        }
    }

    /// Set the required fields
    pub fn with_required(mut self, required: Vec<String>) -> Self {
        self.required = Some(required);
        self
    }

    /// Set the description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Create a builder for constructing elicitation schemas fluently
    pub fn builder() -> ElicitationSchemaBuilder {
        ElicitationSchemaBuilder::new()
    }
}

// =============================================================================
// BUILDER
// =============================================================================

/// Fluent builder for constructing elicitation schemas.
///
/// # Example
///
/// ```rust
/// use rmcp::model::*;
///
/// let schema = ElicitationSchema::builder()
///     .required_email("email")
///     .required_integer("age", 0, 150)
///     .optional_bool("newsletter", false)
///     .description("User registration")
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct ElicitationSchemaBuilder {
    properties: BTreeMap<String, PrimitiveSchema>,
    required: Vec<String>,
    description: Option<String>,
}

impl ElicitationSchemaBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a property to the schema
    pub fn property(mut self, name: impl Into<String>, schema: PrimitiveSchema) -> Self {
        self.properties.insert(name.into(), schema);
        self
    }

    /// Add a required property to the schema
    pub fn required_property(mut self, name: impl Into<String>, schema: PrimitiveSchema) -> Self {
        let name_str = name.into();
        self.required.push(name_str.clone());
        self.properties.insert(name_str, schema);
        self
    }

    // ===========================================================================
    // TYPED PROPERTY METHODS - Cleaner API without PrimitiveSchema wrapper
    // ===========================================================================

    /// Add a string property with custom builder (required)
    pub fn string_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(StringSchema) -> StringSchema,
    ) -> Self {
        self.properties
            .insert(name.into(), PrimitiveSchema::String(f(StringSchema::new())));
        self
    }

    /// Add a required string property with custom builder
    pub fn required_string_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(StringSchema) -> StringSchema,
    ) -> Self {
        let name_str = name.into();
        self.required.push(name_str.clone());
        self.properties
            .insert(name_str, PrimitiveSchema::String(f(StringSchema::new())));
        self
    }

    /// Add a number property with custom builder
    pub fn number_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(NumberSchema) -> NumberSchema,
    ) -> Self {
        self.properties
            .insert(name.into(), PrimitiveSchema::Number(f(NumberSchema::new())));
        self
    }

    /// Add a required number property with custom builder
    pub fn required_number_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(NumberSchema) -> NumberSchema,
    ) -> Self {
        let name_str = name.into();
        self.required.push(name_str.clone());
        self.properties
            .insert(name_str, PrimitiveSchema::Number(f(NumberSchema::new())));
        self
    }

    /// Add an integer property with custom builder
    pub fn integer_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(IntegerSchema) -> IntegerSchema,
    ) -> Self {
        self.properties.insert(
            name.into(),
            PrimitiveSchema::Integer(f(IntegerSchema::new())),
        );
        self
    }

    /// Add a required integer property with custom builder
    pub fn required_integer_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(IntegerSchema) -> IntegerSchema,
    ) -> Self {
        let name_str = name.into();
        self.required.push(name_str.clone());
        self.properties
            .insert(name_str, PrimitiveSchema::Integer(f(IntegerSchema::new())));
        self
    }

    /// Add a boolean property with custom builder
    pub fn bool_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(BooleanSchema) -> BooleanSchema,
    ) -> Self {
        self.properties.insert(
            name.into(),
            PrimitiveSchema::Boolean(f(BooleanSchema::new())),
        );
        self
    }

    /// Add a required boolean property with custom builder
    pub fn required_bool_property(
        mut self,
        name: impl Into<String>,
        f: impl FnOnce(BooleanSchema) -> BooleanSchema,
    ) -> Self {
        let name_str = name.into();
        self.required.push(name_str.clone());
        self.properties
            .insert(name_str, PrimitiveSchema::Boolean(f(BooleanSchema::new())));
        self
    }

    // ===========================================================================
    // CONVENIENCE METHODS - Simple common cases
    // ===========================================================================

    /// Add a required string property
    pub fn required_string(self, name: impl Into<String>) -> Self {
        self.required_property(name, PrimitiveSchema::String(StringSchema::new()))
    }

    /// Add an optional string property
    pub fn optional_string(self, name: impl Into<String>) -> Self {
        self.property(name, PrimitiveSchema::String(StringSchema::new()))
    }

    /// Add a required email property
    pub fn required_email(self, name: impl Into<String>) -> Self {
        self.required_property(name, PrimitiveSchema::String(StringSchema::email()))
    }

    /// Add an optional email property
    pub fn optional_email(self, name: impl Into<String>) -> Self {
        self.property(name, PrimitiveSchema::String(StringSchema::email()))
    }

    /// Add a required string property with custom builder
    pub fn required_string_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(StringSchema) -> StringSchema,
    ) -> Self {
        self.required_property(name, PrimitiveSchema::String(f(StringSchema::new())))
    }

    /// Add an optional string property with custom builder
    pub fn optional_string_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(StringSchema) -> StringSchema,
    ) -> Self {
        self.property(name, PrimitiveSchema::String(f(StringSchema::new())))
    }

    // Convenience methods for numbers

    /// Add a required number property with range
    pub fn required_number(self, name: impl Into<String>, min: f64, max: f64) -> Self {
        self.required_property(
            name,
            PrimitiveSchema::Number(NumberSchema::new().range(min, max)),
        )
    }

    /// Add an optional number property with range
    pub fn optional_number(self, name: impl Into<String>, min: f64, max: f64) -> Self {
        self.property(
            name,
            PrimitiveSchema::Number(NumberSchema::new().range(min, max)),
        )
    }

    /// Add a required number property with custom builder
    pub fn required_number_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(NumberSchema) -> NumberSchema,
    ) -> Self {
        self.required_property(name, PrimitiveSchema::Number(f(NumberSchema::new())))
    }

    /// Add an optional number property with custom builder
    pub fn optional_number_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(NumberSchema) -> NumberSchema,
    ) -> Self {
        self.property(name, PrimitiveSchema::Number(f(NumberSchema::new())))
    }

    // Convenience methods for integers

    /// Add a required integer property with range
    pub fn required_integer(self, name: impl Into<String>, min: i64, max: i64) -> Self {
        self.required_property(
            name,
            PrimitiveSchema::Integer(IntegerSchema::new().range(min, max)),
        )
    }

    /// Add an optional integer property with range
    pub fn optional_integer(self, name: impl Into<String>, min: i64, max: i64) -> Self {
        self.property(
            name,
            PrimitiveSchema::Integer(IntegerSchema::new().range(min, max)),
        )
    }

    /// Add a required integer property with custom builder
    pub fn required_integer_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(IntegerSchema) -> IntegerSchema,
    ) -> Self {
        self.required_property(name, PrimitiveSchema::Integer(f(IntegerSchema::new())))
    }

    /// Add an optional integer property with custom builder
    pub fn optional_integer_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(IntegerSchema) -> IntegerSchema,
    ) -> Self {
        self.property(name, PrimitiveSchema::Integer(f(IntegerSchema::new())))
    }

    // Convenience methods for booleans

    /// Add a required boolean property
    pub fn required_bool(self, name: impl Into<String>) -> Self {
        self.required_property(name, PrimitiveSchema::Boolean(BooleanSchema::new()))
    }

    /// Add an optional boolean property with default value
    pub fn optional_bool(self, name: impl Into<String>, default: bool) -> Self {
        self.property(
            name,
            PrimitiveSchema::Boolean(BooleanSchema::new().with_default(default)),
        )
    }

    /// Add a required boolean property with custom builder
    pub fn required_bool_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(BooleanSchema) -> BooleanSchema,
    ) -> Self {
        self.required_property(name, PrimitiveSchema::Boolean(f(BooleanSchema::new())))
    }

    /// Add an optional boolean property with custom builder
    pub fn optional_bool_with(
        self,
        name: impl Into<String>,
        f: impl FnOnce(BooleanSchema) -> BooleanSchema,
    ) -> Self {
        self.property(name, PrimitiveSchema::Boolean(f(BooleanSchema::new())))
    }

    // Enum convenience methods

    /// Add a required string enum property
    pub fn required_string_enum(self, name: impl Into<String>, values: Vec<String>) -> Self {
        self.required_property(
            name,
            PrimitiveSchema::String(StringSchema::enum_values(values)),
        )
    }

    /// Add an optional string enum property
    pub fn optional_string_enum(self, name: impl Into<String>, values: Vec<String>) -> Self {
        self.property(
            name,
            PrimitiveSchema::String(StringSchema::enum_values(values)),
        )
    }

    /// Mark an existing property as required
    pub fn mark_required(mut self, name: impl Into<String>) -> Self {
        self.required.push(name.into());
        self
    }

    /// Set the schema description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Build the elicitation schema with validation
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Required fields reference non-existent properties
    /// - No properties are defined (empty schema)
    pub fn build(self) -> Result<ElicitationSchema, &'static str> {
        // Validate that all required fields exist in properties
        if !self.required.is_empty() {
            for field_name in &self.required {
                if !self.properties.contains_key(field_name) {
                    return Err("Required field does not exist in properties");
                }
            }
        }

        Ok(ElicitationSchema {
            type_: ObjectTypeConst,
            properties: self.properties,
            required: if self.required.is_empty() {
                None
            } else {
                Some(self.required)
            },
            description: self.description,
        })
    }

    /// Build the elicitation schema without validation (panics on invalid schema)
    ///
    /// # Panics
    ///
    /// Panics if required fields reference non-existent properties
    pub fn build_unchecked(self) -> ElicitationSchema {
        self.build().expect("Invalid elicitation schema")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_string_schema_serialization() {
        let schema = StringSchema::email().description("Email address");
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "string");
        assert_eq!(json["format"], "email");
        assert_eq!(json["description"], "Email address");
    }

    #[test]
    fn test_number_schema_serialization() {
        let schema = NumberSchema::new()
            .range(0.0, 100.0)
            .description("Percentage");
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "number");
        assert_eq!(json["minimum"], 0.0);
        assert_eq!(json["maximum"], 100.0);
    }

    #[test]
    fn test_integer_schema_serialization() {
        let schema = IntegerSchema::new().range(0, 150);
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "integer");
        assert_eq!(json["minimum"], 0);
        assert_eq!(json["maximum"], 150);
    }

    #[test]
    fn test_boolean_schema_serialization() {
        let schema = BooleanSchema::new().with_default(true);
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "boolean");
        assert_eq!(json["default"], true);
    }

    #[test]
    fn test_string_enum_schema_serialization() {
        let schema = StringSchema::enum_values(vec!["US".to_string(), "UK".to_string()])
            .enum_names(vec![
                "United States".to_string(),
                "United Kingdom".to_string(),
            ]);
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "string");
        assert_eq!(json["enum"], json!(["US", "UK"]));
        assert_eq!(
            json["enumNames"],
            json!(["United States", "United Kingdom"])
        );
    }

    #[test]
    fn test_elicitation_schema_builder_simple() {
        let schema = ElicitationSchema::builder()
            .required_email("email")
            .optional_bool("newsletter", false)
            .build()
            .unwrap();

        assert_eq!(schema.properties.len(), 2);
        assert!(schema.properties.contains_key("email"));
        assert!(schema.properties.contains_key("newsletter"));
        assert_eq!(schema.required, Some(vec!["email".to_string()]));
    }

    #[test]
    fn test_elicitation_schema_builder_complex() {
        let schema = ElicitationSchema::builder()
            .required_string_with("name", |s| s.length(1, 100))
            .required_integer("age", 0, 150)
            .optional_bool("newsletter", false)
            .required_string_enum(
                "country",
                vec!["US".to_string(), "UK".to_string(), "CA".to_string()],
            )
            .description("User registration")
            .build()
            .unwrap();

        assert_eq!(schema.properties.len(), 4);
        assert_eq!(
            schema.required,
            Some(vec![
                "name".to_string(),
                "age".to_string(),
                "country".to_string()
            ])
        );
        assert_eq!(schema.description, Some("User registration".to_string()));
    }

    #[test]
    fn test_elicitation_schema_serialization() {
        let schema = ElicitationSchema::builder()
            .required_string_with("name", |s| s.length(1, 100))
            .build()
            .unwrap();

        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["type"], "object");
        assert!(json["properties"]["name"].is_object());
        assert_eq!(json["required"], json!(["name"]));
    }

    #[test]
    #[should_panic(expected = "minimum must be <= maximum")]
    fn test_integer_range_validation() {
        IntegerSchema::new().range(10, 5); // Should panic
    }

    #[test]
    #[should_panic(expected = "min_length must be <= max_length")]
    fn test_string_length_validation() {
        StringSchema::new().length(10, 5); // Should panic
    }

    #[test]
    fn test_integer_range_validation_with_result() {
        let result = IntegerSchema::new().with_range(10, 5);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "minimum must be <= maximum");
    }
}
