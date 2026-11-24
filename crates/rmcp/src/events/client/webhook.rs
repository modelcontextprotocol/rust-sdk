use super::*;
use crate::{
    events::webhook::{
        SUBSCRIBE, SubscribeDelivery, SubscribeMode, SubscribeParams, UNSUBSCRIBE,
        UnsubscribeDelivery, UnsubscribeParams, WebhookOptions, WebhookSubscription,
    },
    model::{JsonObject, ResultType},
};

impl EventsClient {
    /// Subscribe or refresh the same `(principal, url, name, arguments)` grant.
    ///
    /// Stage the secret at the endpoint before calling. Reuse the last safe
    /// checkpoint and refresh before the returned deadline. This low-level
    /// method does not run a receiver, schedule refreshes or persist cursors.
    pub async fn subscribe_webhook(
        &self,
        name: impl Into<String>,
        arguments: Option<JsonObject>,
        options: WebhookOptions,
    ) -> Result<WebhookSubscription, EventsError> {
        let params = SubscribeParams {
            name: name.into(),
            arguments,
            delivery: SubscribeDelivery {
                mode: SubscribeMode::Webhook,
                url: options.url,
                secret: options.secret.encode(),
            },
            cursor: options.cursor,
            max_age_ms: options.max_age_ms,
            ttl_ms: options.ttl,
        };
        decode_result(
            self.webhook_request(SUBSCRIBE, &params.name, &params)
                .await?,
        )
    }

    /// End the webhook grant selected by the same key used to subscribe.
    pub async fn unsubscribe_webhook(
        &self,
        name: impl Into<String>,
        arguments: Option<JsonObject>,
        url: impl Into<String>,
    ) -> Result<(), EventsError> {
        let params = UnsubscribeParams {
            name: name.into(),
            arguments,
            delivery: UnsubscribeDelivery { url: url.into() },
        };
        unsubscribe_result(
            self.webhook_request(UNSUBSCRIBE, &params.name, &params)
                .await?,
        )
    }

    async fn webhook_request(
        &self,
        method: &'static str,
        name: &str,
        params: &impl Serialize,
    ) -> Result<ServerResult, EventsError> {
        self.require_advertisement()?;
        let body = encode_params(params)?;
        self.with_route(name, |descriptor| {
            let body = body.clone();
            async move {
                if method == SUBSCRIBE && !descriptor.delivery.contains(&DeliveryMode::Webhook) {
                    return Err(unsupported(DeliveryMode::Webhook));
                }
                self.request(route(method, body, descriptor.input_schema))
                    .await
            }
        })
        .await
    }
}

fn unsubscribe_result(result: ServerResult) -> Result<(), EventsError> {
    #[derive(serde::Deserialize)]
    struct Complete {
        #[serde(
            rename = "resultType",
            default,
            deserialize_with = "crate::events::wire::complete"
        )]
        _result_type: ResultType,
    }
    let value = match result {
        ServerResult::EmptyResult(_) => return Ok(()),
        ServerResult::CustomResult(result) => result.0,
        result => serde_json::to_value(result).map_err(|_| EventsError::UnexpectedResponse)?,
    };
    // Struct deserialization can also accept arrays; only Result objects are valid.
    if !value.is_object() {
        return Err(EventsError::UnexpectedResponse);
    }
    serde_json::from_value::<Complete>(value)
        .map(|_| ())
        .map_err(|_| EventsError::UnexpectedResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_complete_result_objects() {
        for wire in [
            json!({}),
            json!({"_meta":{"hint":true}}),
            json!({"resultType":"complete"}),
            json!({"resultType":"complete", "_meta":{"hint":true}}),
            json!({"resultType":"complete","extra":42}),
            json!({"extra":42}),
            json!({"action":"accept"}),
        ] {
            assert!(unsubscribe_result(serde_json::from_value(wire.clone()).unwrap()).is_ok());
            assert!(
                unsubscribe_result(ServerResult::CustomResult(crate::model::CustomResult(wire)))
                    .is_ok()
            );
        }
        for wire in [json!({"resultType":"task"}), json!({"resultType":null})] {
            let result: ServerResult = serde_json::from_value(wire.clone()).unwrap();
            assert!(
                matches!(
                    unsubscribe_result(result.clone()),
                    Err(EventsError::UnexpectedResponse)
                ),
                "{wire}: {result:?}"
            );
        }
    }
}
