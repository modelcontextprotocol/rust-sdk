//! Skill handler support — analogous to `tool.rs` for tools.
//!
//! Pedagogy: The skill call context is like a hall pass. When a skill
//! request comes in, the router checks the gradebook, finds the matching
//! route, and hands the handler a "hall pass" (SkillCallContext) that
//! says "you are allowed to run skill X with URI Y."

use crate::{
    model::skills::SkillEntry,
    service::{MaybeBoxFuture, RequestContext, RoleServer},
};

/// Context passed to a skill handler when invoked.
#[non_exhaustive]
pub struct SkillCallContext<'a, S> {
    pub service: &'a S,
    pub uri: String,
    pub context: RequestContext<RoleServer>,
}

impl<'a, S> SkillCallContext<'a, S> {
    pub fn new(service: &'a S, uri: String, context: RequestContext<RoleServer>) -> Self {
        Self {
            service,
            uri,
            context,
        }
    }

    /// Get the skill path from the URI (e.g. "acme/billing/refunds").
    pub fn skill_path(&self) -> Option<String> {
        crate::model::skills::skill_path_from_uri(&self.uri)
    }
}

/// A skill handler fn: takes a service reference and call context,
/// returns the skill's metadata.
pub trait CallSkillHandler<S, A>: Send + Sync + Clone + 'static {
    fn call(
        &self,
        service: &S,
        context: SkillCallContext<S>,
    ) -> ::std::pin::Pin<
        Box<
            dyn ::std::future::Future<Output = Result<SkillEntry, crate::ErrorData>>
                + Send
                + 'static,
        >,
    >;
}

/// Type-erased skill call handler for storage in SkillRouter.
pub type DynCallSkillHandler<S> = dyn for<'a> Fn(SkillCallContext<'a, S>) -> MaybeBoxFuture<'a, Result<SkillEntry, crate::ErrorData>>
    + Send
    + Sync;
