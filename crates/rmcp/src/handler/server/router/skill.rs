//! Skills router — the gradebook that looks up by parsed URI path.
//!
//! Pedagogy: `ToolRouter` is like a phone book (look up by name).
//! `SkillRouter` is like a DMV database — you can't just say "Alice,"
//! you have to hand over the full ID number. The router parses the URI
//! (`skill://acme/billing/refunds/SKILL.md` → skill path `acme/billing/refunds`),
//! then matches against its internal `HashMap`.

use std::{borrow::Cow, sync::Arc};

use crate::{
    handler::server::skill::{CallSkillHandler, DynCallSkillHandler, SkillCallContext},
    model::skills::SkillEntry,
    service::MaybeSend,
};

#[non_exhaustive]
pub struct SkillRoute<S> {
    pub call: Arc<DynCallSkillHandler<S>>,
    pub attr: SkillEntry,
}

impl<S> std::fmt::Debug for SkillRoute<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRoute")
            .field("uri", &self.attr.uri)
            .field("resources", &self.attr.resources)
            .finish()
    }
}

impl<S> Clone for SkillRoute<S> {
    fn clone(&self) -> Self {
        Self {
            call: self.call.clone(),
            attr: self.attr.clone(),
        }
    }
}

impl<S: MaybeSend + 'static> SkillRoute<S> {
    pub fn new<C, A>(attr: impl Into<SkillEntry>, call: C) -> Self
    where
        C: CallSkillHandler<S, A> + MaybeSend + Clone + 'static,
    {
        Self {
            call: Arc::new(move |context: SkillCallContext<S>| {
                let call = call.clone();
                let service = context.service;
                call.call(service, context)
            }),
            attr: attr.into(),
        }
    }

    pub fn uri(&self) -> &str {
        &self.attr.uri
    }

    /// Extract the skill path from this route's URI (e.g. `acme/billing/refunds`).
    pub fn skill_path(&self) -> Option<String> {
        crate::model::skills::skill_path_from_uri(&self.attr.uri)
    }
}

pub trait IntoSkillRoute<S, A> {
    fn into_skill_route(self) -> SkillRoute<S>;
}

impl<S, C, A, T> IntoSkillRoute<S, A> for (T, C)
where
    S: MaybeSend + 'static,
    C: CallSkillHandler<S, A> + MaybeSend + Clone + 'static,
    T: Into<SkillEntry>,
{
    fn into_skill_route(self) -> SkillRoute<S> {
        SkillRoute::new(self.0.into(), self.1)
    }
}

impl<S> IntoSkillRoute<S, ()> for SkillRoute<S>
where
    S: MaybeSend + 'static,
{
    fn into_skill_route(self) -> SkillRoute<S> {
        self
    }
}

#[non_exhaustive]
pub struct SkillAttrGenerateFunctionAdapter;

impl<S, F> IntoSkillRoute<S, SkillAttrGenerateFunctionAdapter> for F
where
    S: MaybeSend + 'static,
    F: Fn() -> SkillRoute<S>,
{
    fn into_skill_route(self) -> SkillRoute<S> {
        (self)()
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct SkillRouter<S> {
    pub map: std::collections::HashMap<Cow<'static, str>, SkillRoute<S>>,
}

impl<S> Default for SkillRouter<S> {
    fn default() -> Self {
        Self {
            map: std::collections::HashMap::new(),
        }
    }
}

impl<S> Clone for SkillRouter<S> {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<S> SkillRouter<S>
where
    S: MaybeSend + 'static,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_route<R, A>(mut self, route: R) -> Self
    where
        R: IntoSkillRoute<S, A>,
    {
        self.add_route(route.into_skill_route());
        self
    }

    pub fn add_route(&mut self, item: SkillRoute<S>) {
        if let Some(path) = item.skill_path() {
            self.map.insert(Cow::Owned(path), item);
        }
    }

    pub fn merge(&mut self, other: SkillRouter<S>) {
        for item in other.map.into_values() {
            self.add_route(item);
        }
    }

    /// Match a request URI to its route, parsing out the skill path first.
    pub fn get_by_uri(&self, uri: &str) -> Option<&SkillRoute<S>> {
        let path = crate::model::skills::skill_path_from_uri(uri)?;
        self.map.get(path.as_str())
    }

    pub fn list_all(&self) -> Vec<SkillEntry> {
        let mut skills: Vec<_> = self.map.values().map(|r| r.attr.clone()).collect();
        skills.sort_by(|a, b| a.uri.cmp(&b.uri));
        skills
    }
}
