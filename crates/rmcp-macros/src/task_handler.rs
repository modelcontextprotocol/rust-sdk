use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ImplItem, ItemImpl};

use crate::common::{has_method, has_sibling_handler};

#[derive(FromMeta)]
#[darling(default)]
struct TaskHandlerAttribute {
    processor: Expr,
}

impl Default for TaskHandlerAttribute {
    fn default() -> Self {
        Self {
            processor: syn::parse2(quote! { self.processor }).expect("default processor expr"),
        }
    }
}

pub fn task_handler(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let TaskHandlerAttribute { processor } = TaskHandlerAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;

    if !has_method("list_tasks", &item_impl) {
        let list_fn = quote! {
            async fn list_tasks(
                &self,
                _request: Option<rmcp::model::PaginatedRequestParams>,
                _: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::ListTasksResult, McpError> {
                let running_ids = (#processor).lock().await.list_running();
                let total = running_ids.len() as u64;
                let tasks = running_ids
                    .into_iter()
                    .map(|task_id| {
                        let timestamp = rmcp::task_manager::current_timestamp();
                        rmcp::model::Task::new(
                            task_id,
                            rmcp::model::TaskStatus::Working,
                            timestamp.clone(),
                            timestamp,
                        )
                    })
                    .collect::<Vec<_>>();

                Ok(rmcp::model::ListTasksResult::new(tasks))
            }
        };
        item_impl.items.push(syn::parse2::<ImplItem>(list_fn)?);
    }

    if !has_method("enqueue_task", &item_impl) {
        let enqueue_fn = quote! {
            async fn enqueue_task(
                &self,
                request: rmcp::model::CallToolRequestParams,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::CreateTaskResult, McpError> {
                use rmcp::task_manager::{
                    current_timestamp, OperationDescriptor, OperationMessage, OperationResultTransport,
                    ToolCallTaskResult,
                };
                let task_id = context.id.to_string();
                let operation_name = request.name.to_string();
                let future_request = request.clone();
                let future_context = context.clone();
                let server = self.clone();

                let descriptor = OperationDescriptor::new(task_id.clone(), operation_name)
                    .with_context(context)
                    .with_client_request(rmcp::model::ClientRequest::CallToolRequest(
                        rmcp::model::Request::new(request),
                    ));

                let task_result_id = task_id.clone();
                let future = Box::pin(async move {
                    let result = server.call_tool(future_request, future_context).await;
                    Ok(
                        Box::new(ToolCallTaskResult::new(task_result_id, result))
                            as Box<dyn OperationResultTransport>,
                    )
                });

                (#processor)
                    .lock()
                    .await
                    .submit_operation(OperationMessage::new(descriptor, future))
                    .map_err(|err| rmcp::ErrorData::internal_error(
                        format!("failed to enqueue task: {err}"),
                        None,
                    ))?;

                let timestamp = current_timestamp();
                let task = rmcp::model::Task::new(
                    task_id,
                    rmcp::model::TaskStatus::Working,
                    timestamp.clone(),
                    timestamp,
                ).with_status_message("Task accepted");

                Ok(rmcp::model::CreateTaskResult::new(task))
            }
        };
        item_impl.items.push(syn::parse2::<ImplItem>(enqueue_fn)?);
    }

    if !has_method("get_task_info", &item_impl) {
        let get_info_fn = quote! {
            async fn get_task_info(
                &self,
                request: rmcp::model::GetTaskInfoParams,
                _context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::GetTaskResult, McpError> {
                use rmcp::task_manager::current_timestamp;
                let task_id = request.task_id.clone();
                let mut processor = (#processor).lock().await;

                // Check completed results first
                let completed = processor.peek_completed().iter().rev().find(|r| r.descriptor.operation_id == task_id);
                if let Some(completed_result) = completed {
                    // Determine Finished vs Failed
                    let status = match &completed_result.result {
                        Ok(boxed) => {
                            if let Some(tool) = boxed.as_any().downcast_ref::<rmcp::task_manager::ToolCallTaskResult>() {
                                match &tool.result {
                                    Ok(_) => rmcp::model::TaskStatus::Completed,
                                    Err(_) => rmcp::model::TaskStatus::Failed,
                                }
                            } else {
                                rmcp::model::TaskStatus::Completed
                            }
                        }
                        Err(_) => rmcp::model::TaskStatus::Failed,
                    };
                    let timestamp = current_timestamp();
                    let mut task = rmcp::model::Task::new(
                        task_id,
                        status,
                        timestamp.clone(),
                        timestamp,
                    );
                    if let Some(ttl) = completed_result.descriptor.ttl {
                        task = task.with_ttl(ttl);
                    }
                    return Ok(rmcp::model::GetTaskResult { meta: None, task });
                }

                // If not completed, check running
                let running = processor.list_running();
                if running.into_iter().any(|id| id == task_id) {
                    let timestamp = current_timestamp();
                    let task = rmcp::model::Task::new(
                        task_id,
                        rmcp::model::TaskStatus::Working,
                        timestamp.clone(),
                        timestamp,
                    );
                    return Ok(rmcp::model::GetTaskResult { meta: None, task });
                }

                Err(McpError::resource_not_found(format!("task not found: {}", task_id), None))
            }
        };
        item_impl.items.push(syn::parse2::<ImplItem>(get_info_fn)?);
    }

    if !has_method("get_task_result", &item_impl) {
        let get_result_fn = quote! {
            async fn get_task_result(
                &self,
                request: rmcp::model::GetTaskResultParams,
                _context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::GetTaskPayloadResult, McpError> {
                use std::time::Duration;
                let task_id = request.task_id.clone();

                loop {
                    // Scope the lock so we can await outside if needed
                    {
                        let mut processor = (#processor).lock().await;

                        if let Some(task_result) = processor.take_completed_result(&task_id) {
                            match task_result.result {
                                Ok(boxed) => {
                                    if let Some(tool) = boxed.as_any().downcast_ref::<rmcp::task_manager::ToolCallTaskResult>() {
                                        match &tool.result {
                                            Ok(call_tool) => {
                                                let value = ::rmcp::serde_json::to_value(call_tool).unwrap_or_default();
                                                return Ok(rmcp::model::GetTaskPayloadResult::new(value));
                                            }
                                            Err(err) => return Err(McpError::internal_error(
                                                format!("task failed: {}", err),
                                                None,
                                            )),
                                        }
                                    } else {
                                        return Err(McpError::internal_error("unsupported task result transport", None));
                                    }
                                }
                                Err(err) => return Err(McpError::internal_error(
                                    format!("task execution error: {}", err),
                                    None,
                                )),
                            }
                        }

                        // Not completed yet: if not running, return not found
                        let running = processor.list_running();
                        if !running.iter().any(|id| id == &task_id) {
                            return Err(McpError::resource_not_found(format!("task not found: {}", task_id), None));
                        }
                    }

                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        };
        item_impl
            .items
            .push(syn::parse2::<ImplItem>(get_result_fn)?);
    }

    if !has_method("cancel_task", &item_impl) {
        let cancel_fn = quote! {
            async fn cancel_task(
                &self,
                request: rmcp::model::CancelTaskParams,
                _context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::CancelTaskResult, McpError> {
                use rmcp::task_manager::current_timestamp;
                let task_id = request.task_id;
                let mut processor = (#processor).lock().await;

                if processor.cancel_task(&task_id) {
                    let timestamp = current_timestamp();
                    let task = rmcp::model::Task::new(
                        task_id,
                        rmcp::model::TaskStatus::Cancelled,
                        timestamp.clone(),
                        timestamp,
                    );
                    return Ok(rmcp::model::CancelTaskResult { meta: None, task });
                }

                // If already completed, signal it's not cancellable
                let exists_completed = processor.peek_completed().iter().any(|r| r.descriptor.operation_id == task_id);
                if exists_completed {
                    return Err(McpError::invalid_request(format!("task already completed: {}", task_id), None));
                }

                Err(McpError::resource_not_found(format!("task not found: {}", task_id), None))
            }
        };
        item_impl.items.push(syn::parse2::<ImplItem>(cancel_fn)?);
    }

    // Auto-generate get_info() if not already provided and no sibling tool/prompt handler
    // will generate it (they take priority since they run as outer attributes).
    if !has_method("get_info", &item_impl)
        && !has_sibling_handler(&item_impl, "tool_handler")
        && !has_sibling_handler(&item_impl, "prompt_handler")
    {
        let get_info_fn = crate::tool_handler::build_get_info(
            &item_impl,
            None,
            None,
            None,
            crate::tool_handler::CallerCapability::Tasks,
        )?;
        item_impl.items.push(get_info_fn);
    }

    Ok(item_impl.into_token_stream())
}
