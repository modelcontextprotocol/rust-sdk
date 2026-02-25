use std::{borrow::Cow, pin::Pin, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    ErrorData,
    handler::server::{
        common::schema_for_empty_input,
        tool::{schema_for_output, schema_for_type},
        wrapper::{Json, Parameters},
    },
    model::{Icon, JsonObject, Meta, ToolAnnotations, ToolExecution},
    schemars::JsonSchema,
};

/// Base trait to define attributes of a tool.
///
/// Tools implementing [`SyncTool`] or [`AsyncTool`] must implement this trait first.
///
/// All methods are consistent with fields of [`Tool`][crate::model::Tool].
pub trait ToolBase {
    /// Parameter type, will used in the invoke parameter of [`SyncTool`] or [`AsyncTool`] trait
    ///
    /// If the tool does not have any parameters, you **MUST** override [`input_schema`][Self::input_schema]
    /// method. See its documentation for more details.
    type Parameter: for<'de> Deserialize<'de> + JsonSchema + Send + Default + 'static;
    /// Output type, will used in the invoke output of [`SyncTool`] or [`AsyncTool`] trait
    ///
    /// If the tool does not have any output, you **MUST** override [`output_schema`][Self::output_schema]
    /// method. See its documentation for more details.
    type Output: Serialize + JsonSchema + Send + 'static;
    /// Error type, will used in the invoke output of [`SyncTool`] or [`AsyncTool`] trait
    type Error: Into<ErrorData> + Send + 'static;

    fn name() -> Cow<'static, str>;

    fn title() -> Option<String> {
        None
    }
    fn description() -> Option<Cow<'static, str>> {
        None
    }

    /// Json schema for tool input.
    ///
    /// The default implementation generates schema based on [`Self::Parameter`] type.
    ///
    /// If the tool does not have any parameters, you should override this methods to return [`None`],
    /// and when invoked, the parameter will get default values.
    fn input_schema() -> Option<Arc<JsonObject>> {
        Some(schema_for_type::<Parameters<Self::Parameter>>())
    }

    /// Json schema for tool output.
    ///
    /// The default implementation generates schema based on [`Self::Output`] type.
    ///
    /// If the tool does not have any output, you should override this methods to return [`None`].
    fn output_schema() -> Option<Arc<JsonObject>> {
        Some(schema_for_output::<Self::Output>().unwrap_or_else(|e| {
            panic!(
                "Invalid output schema for ToolBase::Output type `{0}`: {1}",
                std::any::type_name::<Self::Output>(),
                e,
            );
        }))
    }

    fn annotations() -> Option<ToolAnnotations> {
        None
    }
    fn execution() -> Option<ToolExecution> {
        None
    }
    fn icons() -> Option<Vec<Icon>> {
        None
    }
    fn meta() -> Option<Meta> {
        None
    }
}

/// Synchronous version of a tool.
///
/// Consider using [`AsyncTool`] if your workflow involves asynchronous operations.
/// Examples are shown in [the module-level documentation][crate::handler::server::router::tool].
pub trait SyncTool<S: Sync + Send + 'static>: ToolBase {
    fn invoke(service: &S, param: Self::Parameter) -> Result<Self::Output, Self::Error>;
}

/// Asynchronous version of a tool.
///
/// Consider using [`SyncTool`] if your workflow does not involve asynchronous operations.
/// Examples are shown in [the module-level documentation][crate::handler::server::router::tool].
pub trait AsyncTool<S: Sync + Send + 'static>: ToolBase {
    fn invoke(
        service: &S,
        param: Self::Parameter,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}

pub(crate) fn tool_attribute<T: ToolBase>() -> crate::model::Tool {
    crate::model::Tool {
        name: T::name(),
        title: T::title(),
        description: T::description(),
        input_schema: T::input_schema().unwrap_or_else(schema_for_empty_input),
        output_schema: T::output_schema(),
        annotations: T::annotations(),
        execution: T::execution(),
        icons: T::icons(),
        meta: T::meta(),
    }
}

pub(crate) fn sync_tool_wrapper<S: Sync + Send + 'static, T: SyncTool<S>>(
    service: &S,
    Parameters(params): Parameters<T::Parameter>,
) -> Result<Json<T::Output>, ErrorData> {
    T::invoke(service, params).map(Json).map_err(Into::into)
}

pub(crate) fn sync_tool_wrapper_with_empty_params<S: Sync + Send + 'static, T: SyncTool<S>>(
    service: &S,
) -> Result<Json<T::Output>, ErrorData> {
    T::invoke(service, T::Parameter::default())
        .map(Json)
        .map_err(Into::into)
}

#[expect(clippy::type_complexity)]
pub(crate) fn async_tool_wrapper<S: Sync + Send + 'static, T: AsyncTool<S>>(
    service: &S,
    Parameters(params): Parameters<T::Parameter>,
) -> Pin<Box<dyn Future<Output = Result<Json<T::Output>, ErrorData>> + Send + '_>> {
    Box::pin(async move {
        T::invoke(service, params)
            .await
            .map(Json)
            .map_err(Into::into)
    })
}

#[expect(clippy::type_complexity)]
pub(crate) fn async_tool_wrapper_with_empty_params<S: Sync + Send + 'static, T: AsyncTool<S>>(
    service: &S,
) -> Pin<Box<dyn Future<Output = Result<Json<T::Output>, ErrorData>> + Send + '_>> {
    Box::pin(async move {
        T::invoke(service, T::Parameter::default())
            .await
            .map(Json)
            .map_err(Into::into)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as rmcp;
    use crate::tool; // workaround for macros

    #[derive(Deserialize, schemars::JsonSchema, Default)]
    struct AddParameter {
        left: usize,
        right: usize,
    }
    #[derive(Serialize, schemars::JsonSchema, PartialEq, Debug)]
    struct AddOutput {
        sum: usize,
    }

    struct MacroBasedToolServer;

    impl MacroBasedToolServer {
        #[expect(unused)]
        #[tool(name = "adder", description = "Modular add two integers")]
        fn add(
            &self,
            Parameters(AddParameter { left, right }): Parameters<AddParameter>,
        ) -> Json<AddOutput> {
            Json(AddOutput {
                sum: left.wrapping_add(right),
            })
        }

        #[expect(unused)]
        #[tool(name = "empty", description = "Empty tool")]
        fn empty(&self) {}
    }

    struct AddTool;
    impl ToolBase for AddTool {
        type Parameter = AddParameter;
        type Output = AddOutput;
        type Error = ErrorData;

        fn name() -> Cow<'static, str> {
            "adder".into()
        }

        fn description() -> Option<Cow<'static, str>> {
            Some("Modular add two integers".into())
        }
    }
    impl SyncTool<TraitBasedToolServer> for AddTool {
        fn invoke(
            _service: &TraitBasedToolServer,
            AddParameter { left, right }: Self::Parameter,
        ) -> Result<Self::Output, Self::Error> {
            Ok(AddOutput {
                sum: left.wrapping_add(right),
            })
        }
    }
    impl AsyncTool<TraitBasedToolServer> for AddTool {
        async fn invoke(
            _service: &TraitBasedToolServer,
            AddParameter { left, right }: Self::Parameter,
        ) -> Result<Self::Output, Self::Error> {
            Ok(AddOutput {
                sum: left.wrapping_add(right),
            })
        }
    }

    enum EmptyToolCustomError {
        Internal,
        InvalidParams,
    }
    impl From<EmptyToolCustomError> for ErrorData {
        fn from(value: EmptyToolCustomError) -> Self {
            match value {
                EmptyToolCustomError::Internal => Self::internal_error("internal error", None),
                EmptyToolCustomError::InvalidParams => Self::invalid_params("invalid params", None),
            }
        }
    }

    struct EmptyTool;
    impl ToolBase for EmptyTool {
        type Parameter = ();
        type Output = ();
        type Error = EmptyToolCustomError;

        fn name() -> Cow<'static, str> {
            "empty".into()
        }

        fn description() -> Option<Cow<'static, str>> {
            Some("Empty tool".into())
        }

        fn input_schema() -> Option<Arc<JsonObject>> {
            None
        }

        fn output_schema() -> Option<Arc<JsonObject>> {
            None
        }
    }
    impl SyncTool<TraitBasedToolServer> for EmptyTool {
        fn invoke(
            _service: &TraitBasedToolServer,
            _param: Self::Parameter,
        ) -> Result<Self::Output, Self::Error> {
            Err(EmptyToolCustomError::Internal)
        }
    }
    impl AsyncTool<TraitBasedToolServer> for EmptyTool {
        async fn invoke(
            _service: &TraitBasedToolServer,
            _param: Self::Parameter,
        ) -> Result<Self::Output, Self::Error> {
            Err(EmptyToolCustomError::InvalidParams)
        }
    }

    struct TraitBasedToolServer;

    #[test]
    fn test_macro_and_trait_have_same_attrs() {
        let macro_attrs = MacroBasedToolServer::add_tool_attr();
        let trait_attrs = tool_attribute::<AddTool>();
        assert_eq!(macro_attrs, trait_attrs);
    }

    #[test]
    fn test_macro_and_trait_have_same_attrs_for_empty_tool() {
        let macro_attrs = MacroBasedToolServer::empty_tool_attr();
        let trait_attrs = tool_attribute::<EmptyTool>();
        assert_eq!(macro_attrs, trait_attrs);
    }

    #[test]
    fn test_sync_tool_wrapper_happy_path() {
        let left = 1;
        let right = 2;
        let result = sync_tool_wrapper::<_, AddTool>(
            &TraitBasedToolServer,
            Parameters(AddParameter { left, right }),
        );
        assert!(result.is_ok());
        if let Ok(result) = result {
            assert_eq!(result.0, AddOutput { sum: 3 });
        }
    }

    #[tokio::test]
    async fn test_async_tool_wrapper_happy_path() {
        let left = 1;
        let right = 2;
        let result = async_tool_wrapper::<_, AddTool>(
            &TraitBasedToolServer,
            Parameters(AddParameter { left, right }),
        )
        .await;
        assert!(result.is_ok());
        if let Ok(result) = result {
            assert_eq!(result.0, AddOutput { sum: 3 });
        }
    }

    #[test]
    fn test_sync_tool_wrapper_error_conversion() {
        let result = sync_tool_wrapper::<_, EmptyTool>(&TraitBasedToolServer, Parameters(()));
        assert!(result.is_err());
        if let Err(result) = result {
            assert_eq!(result, ErrorData::internal_error("internal error", None));
        }
    }

    #[tokio::test]
    async fn test_async_tool_wrapper_error_conversion() {
        let result =
            async_tool_wrapper::<_, EmptyTool>(&TraitBasedToolServer, Parameters(())).await;
        assert!(result.is_err());
        if let Err(result) = result {
            assert_eq!(result, ErrorData::invalid_params("invalid params", None));
        }
    }
}
