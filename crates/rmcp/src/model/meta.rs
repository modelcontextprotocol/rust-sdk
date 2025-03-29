use super::{ClientRequest, RequestMeta, ServerRequest};

pub trait WithMeta<M> {
    fn get_meta_mut(&mut self) -> Option<&mut M>;
    fn get_meta(&self) -> Option<&M>;
}

impl WithMeta<RequestMeta> for ClientRequest {
    fn get_meta_mut(&mut self) -> Option<&mut RequestMeta> {
        #[allow(clippy::single_match)]
        match self {
            ClientRequest::CallToolRequest(req) => Some(&mut req.params._meta),
            _ => None,
        }
    }

    fn get_meta(&self) -> Option<&RequestMeta> {
        #[allow(clippy::single_match)]
        match self {
            ClientRequest::CallToolRequest(req) => Some(&req.params._meta),
            _ => None,
        }
    }
}

impl WithMeta<RequestMeta> for ServerRequest {
    fn get_meta(&self) -> Option<&RequestMeta> {
        None
    }

    fn get_meta_mut(&mut self) -> Option<&mut RequestMeta> {
        None
    }
}
