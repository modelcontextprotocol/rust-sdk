use super::{ClientRequest, RequestMeta, ServerRequest};

pub trait WithMeta<M> {
    fn set_meta(&mut self, meta: Option<M>);
    fn get_meta(&self) -> Option<&M>;
}

impl WithMeta<RequestMeta> for ClientRequest {
    fn set_meta(&mut self, meta: Option<RequestMeta>) {
        #[allow(clippy::single_match)]
        match self {
            ClientRequest::CallToolRequest(req) => {
                req.params._meta = meta;
            }
            _ => {}
        }
    }

    fn get_meta(&self) -> Option<&RequestMeta> {
        #[allow(clippy::single_match)]
        match self {
            ClientRequest::CallToolRequest(req) => req.params._meta.as_ref(),
            _ => None,
        }
    }
}

impl WithMeta<RequestMeta> for ServerRequest {
    fn set_meta(&mut self, _meta: Option<RequestMeta>) {}

    fn get_meta(&self) -> Option<&RequestMeta> {
        None
    }
}
