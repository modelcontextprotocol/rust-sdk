/// Macro to implement a handler trait for smart pointer types (Box, Arc, etc.)
///
/// This dramatically reduces boilerplate by automatically generating delegating implementations
/// for all trait methods.
///
/// # Usage
/// ```ignore
/// impl_handler_for_smart_ptr! {
///     impl ClientHandler for Box {
///         // Methods that return futures with parameters
///         async fn ping(&self, context: RequestContext<RoleClient>) -> Result<(), McpError>;
///         async fn create_message(&self, params: CreateMessageRequestParam, context: RequestContext<RoleClient>) -> Result<CreateMessageResult, McpError>;
///         
///         // Methods that return futures without extra parameters
///         async fn list_roots(&self, context: RequestContext<RoleClient>) -> Result<ListRootsResult, McpError>;
///         
///         // Synchronous methods
///         fn get_info(&self) -> ClientInfo;
///     }
/// }
/// ```
#[macro_export]
macro_rules! impl_handler_for_smart_ptr {
    (
        impl $trait:ident for $ptr:ty {
            $(
                $(#[$meta:meta])*
                async fn $method:ident(&self $(, $param:ident: $param_ty:ty)*) -> $ret:ty;
            )*
            $(
                $(#[$sync_meta:meta])*
                fn $sync_method:ident(&self $(, $sync_param:ident: $sync_param_ty:ty)*) -> $sync_ret:ty;
            )*
        }
    ) => {
        impl<H: $trait> $trait for $ptr {
            $(
                $(#[$meta])*
                fn $method(
                    &self
                    $(, $param: $param_ty)*
                ) -> impl Future<Output = $ret> + Send + '_ {
                    (**self).$method($($param),*)
                }
            )*

            $(
                $(#[$sync_meta])*
                fn $sync_method(
                    &self
                    $(, $sync_param: $sync_param_ty)*
                ) -> $sync_ret {
                    (**self).$sync_method($($sync_param),*)
                }
            )*
        }
    };
}

/// Convenience macro to implement a handler for both Box and Arc
#[macro_export]
macro_rules! impl_handler_for_box_and_arc {
    (
        impl $trait:ident {
            $(
                async fn $method:ident(&self $(, $param:ident: $param_ty:ty)*) -> $ret:ty;
            )*
            $(
                fn $sync_method:ident(&self $(, $sync_param:ident: $sync_param_ty:ty)*) -> $sync_ret:ty;
            )*
        }
    ) => {
        $crate::impl_handler_for_smart_ptr! {
            impl $trait for Box<H> {
                $(
                    async fn $method(&self $(, $param: $param_ty)*) -> $ret;
                )*
                $(
                    fn $sync_method(&self $(, $sync_param: $sync_param_ty)*) -> $sync_ret;
                )*
            }
        }

        $crate::impl_handler_for_smart_ptr! {
            impl $trait for std::sync::Arc<H> {
                $(
                    async fn $method(&self $(, $param: $param_ty)*) -> $ret;
                )*
                $(
                    fn $sync_method(&self $(, $sync_param: $sync_param_ty)*) -> $sync_ret;
                )*
            }
        }
    };
}
