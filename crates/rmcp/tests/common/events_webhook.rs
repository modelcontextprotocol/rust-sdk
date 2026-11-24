use std::{convert::Infallible, sync::atomic::AtomicBool};

use axum::body::{Body, Bytes, to_bytes};
use http::{HeaderMap, Request, Response, StatusCode};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use rmcp::events::{
    server::WebhookEgress,
    webhook::{
        WebhookMessage, WebhookOptions, WebhookSecret, WebhookSubscription, WebhookTtl,
        challenge_response, verify_webhook,
    },
};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

use super::*;

// Regenerate in the fixture directory with OpenSSL 3.6 (zsh/bash):
// openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj '/CN=rmcp webhook fixture CA' -keyout ca.key.pem -out ca.csr
// openssl x509 -req -in ca.csr -signkey ca.key.pem -not_before 20200101000000Z -not_after 20500101000000Z -out ca.pem -extfile <(printf '%s\n' 'basicConstraints=critical,CA:TRUE' 'keyUsage=critical,keyCertSign,cRLSign')
// openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj '/CN=localhost' -keyout server.key.pem -out server.csr
// openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key.pem -not_before 20260501000000Z -not_after 20280723000000Z -out server.pem -extfile <(printf '%s\n' 'basicConstraints=critical,CA:FALSE' 'keyUsage=critical,digitalSignature' 'extendedKeyUsage=serverAuth' 'subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1')
// Checked-in expiry (UTC): CA 2050-01-01; leaf 2028-07-23.
const CA: &[u8] = include_bytes!("../fixtures/events-webhook/ca.pem");
const KEY: &[u8] = include_bytes!("../fixtures/events-webhook/server.key.pem");
const CERT: &[u8] = include_bytes!("../fixtures/events-webhook/server.pem");

struct Post {
    path: String,
    headers: HeaderMap,
    body: Bytes,
    reply: oneshot::Sender<Response<Body>>,
}

impl Post {
    fn verify(&self, secret: &WebhookSecret) -> Result<WebhookMessage, EventsError> {
        Ok(verify_webhook(
            &self.headers,
            &self.body,
            std::slice::from_ref(secret),
            chrono::Utc::now(),
        )?
        .message)
    }
    fn check(&self, secret: &WebhookSecret) -> WebhookMessage {
        self.verify(secret).unwrap()
    }
    fn respond(self, status: StatusCode) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = status;
        self.reply.send(response).unwrap();
    }
    fn echo(self, secret: &WebhookSecret) {
        let WebhookMessage::Verification { challenge } = self.check(secret) else {
            panic!("application delivery before verification");
        };
        assert!(
            self.headers["webhook-id"]
                .to_str()
                .unwrap()
                .starts_with("msg_verification_")
        );
        self.reply
            .send(Response::new(Body::from(
                serde_json::to_vec(&challenge_response(&challenge)).unwrap(),
            )))
            .unwrap();
    }
}

async fn receive(posts: &mut mpsc::Receiver<Post>) -> Post {
    tokio::time::timeout(Duration::from_secs(8), posts.recv())
        .await
        .expect("callback timed out")
        .expect("callback channel closed")
}

async fn receiver() -> (String, mpsc::Receiver<Post>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(16);
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(CERT).unwrap()],
            PrivateKeyDer::from_pem_slice(KEY).unwrap(),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}/hook", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let tls = TlsAcceptor::from(Arc::new(tls));
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (socket, _) = tokio::select! {
                _ = connections.join_next(), if !connections.is_empty() => continue,
                incoming = listener.accept() => incoming.unwrap(),
            };
            let (tls, tx) = (tls.clone(), tx.clone());
            connections.spawn(async move {
                let Ok(stream) = tls.accept(socket).await else {
                    return;
                };
                let service = service_fn(move |request: Request<Incoming>| {
                    let tx = tx.clone();
                    async move {
                        assert_eq!(request.method(), "POST");
                        let (parts, body) = request.into_parts();
                        assert_eq!(&parts.headers["content-type"], "application/json");
                        let body = to_bytes(Body::new(body), 256 * 1024).await.unwrap();
                        let (reply, response) = oneshot::channel();
                        let post = Post {
                            path: parts.uri.path().into(),
                            headers: parts.headers,
                            body,
                            reply,
                        };
                        let _ = tx.send(post).await;
                        Ok::<_, Infallible>(
                            response
                                .await
                                .unwrap_or_else(|_| Response::new(Body::empty())),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (url, rx, task)
}

fn configured_egress(allow_loopback: bool) -> WebhookEgress {
    let mut egress = WebhookEgress::default();
    if allow_loopback {
        egress
            .additional_allowed_networks
            .push(("127.0.0.1".parse().unwrap(), 32));
    }
    egress
        .additional_root_certificates
        .push(CertificateDer::from_pem_slice(CA).unwrap().to_vec());
    egress
}

fn runtime(authenticated: bool, allow_loopback: bool) -> EventsServer {
    EventsServer::new()
        .with_webhooks(
            move |extensions| {
                (authenticated
                    && extensions
                        .get::<axum::http::request::Parts>()
                        .and_then(|parts| parts.extensions.get::<Authenticated>())
                        .is_some())
                .then(|| "fixture-tenant".into())
            },
            configured_egress(allow_loopback),
        )
        .unwrap()
}

fn descriptor() -> Descriptor {
    #[derive(schemars::JsonSchema, serde::Deserialize)]
    struct Arguments {
        #[serde(rename = "region")]
        #[schemars(extend("x-mcp-header" = "Region"))]
        _region: String,
    }
    Descriptor::typed::<Arguments, JsonObject>("source", "", [DeliveryMode::Webhook])
}

fn args() -> Option<JsonObject> {
    Some(serde_json::from_value(json!({"region": "test"})).unwrap())
}

async fn activate(
    api: &EventsClient,
    posts: &mut mpsc::Receiver<Post>,
    options: &WebhookOptions,
    before_echo: impl FnOnce(),
) -> WebhookSubscription {
    let request = subscribe(api, options.clone());
    tokio::pin!(request);
    let post = tokio::select! {
        result = &mut request => panic!("subscribe completed before challenge: {result:?}"),
        post = receive(posts) => post,
    };
    before_echo();
    post.echo(&options.secret);
    request.await.unwrap()
}

async fn subscribe(
    api: &EventsClient,
    options: WebhookOptions,
) -> Result<WebhookSubscription, EventsError> {
    api.subscribe_webhook("source", args(), options).await
}

async fn access(
    runtime: EventsServer,
) -> (
    EventsClient,
    CancellationToken,
    tokio::task::JoinHandle<()>,
    rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
) {
    let (url, stop, server) = server(App(Some(runtime), false)).await;
    let peer = connect(url).await;
    (EventsClient::new(peer.peer().clone()), stop, server, peer)
}

async fn close(
    peer: rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<()>,
) {
    peer.cancel().await.unwrap();
    stop.cancel();
    server.await.unwrap();
}

async fn refresh_until(
    api: &EventsClient,
    options: &WebhookOptions,
    predicate: impl Fn(&rmcp::events::webhook::WebhookSubscription) -> bool,
) -> WebhookSubscription {
    let grant = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let grant = subscribe(api, options.clone()).await.unwrap();
            if predicate(&grant) {
                return grant;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    grant.expect("refresh condition not reached")
}

#[tokio::test]
async fn authenticated_delivery_rotation_watermark_skip_and_removal() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let auth = Arc::new(AtomicUsize::new(0));
    let authorizations = auth.clone();
    let emit = runtime
        .register_emit(descriptor().authorize(move |ctx, params, mode| {
            assert_eq!(mode, DeliveryMode::Webhook);
            assert_eq!(params["region"], "test");
            let parts = ctx.extensions.get::<axum::http::request::Parts>().unwrap();
            assert_eq!(parts.headers["mcp-name"], "source");
            assert_eq!(parts.headers["mcp-param-region"], "test");
            authorizations.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(()))
        }))
        .unwrap();
    let (api, stop, service, peer) = access(runtime.clone()).await;
    assert_eq!(
        api.list_page(None).await.unwrap().events[0].delivery,
        [DeliveryMode::Webhook]
    );
    let mut managed = SubscriptionOptions::default();
    managed.mode = Some(DeliveryMode::Webhook);
    assert!(matches!(
        api.subscribe("source", args(), managed).await,
        Err(EventsError::Unsupported { value: Some(mode), .. }) if mode == "webhook"
    ));
    assert!(matches!(
        api.subscribe("source", args(), SubscriptionOptions::default())
            .await,
        Err(EventsError::Unsupported { value: None, .. })
    ));
    let secret = WebhookSecret::generate();
    let mut options = WebhookOptions::new(&url, secret.clone());
    options.ttl = WebhookTtl::NoExpiry;
    let grant = activate(&api, &mut posts, &options, || {
        assert_eq!(auth.load(Ordering::SeqCst), 1);
    })
    .await;
    assert!(grant.refresh_before.unwrap() > chrono::Utc::now());
    assert!(grant.refresh_before.unwrap() < chrono::Utc::now() + Duration::from_secs(3600));
    let initial = grant.cursor.clone().unwrap();
    let new = WebhookSecret::generate();
    options.secret = new.clone();
    options.cursor = Some("must not reposition live grants".into());
    let refreshed = subscribe(&api, options.clone()).await.unwrap();
    assert_eq!(refreshed.id, grant.id);
    assert_eq!(refreshed.cursor.as_deref(), Some(initial.as_str()));
    assert!(refreshed.delivery_status.unwrap().active);

    for id in ["first", "second"] {
        emit.emit(Some(id.into()), chrono::Utc::now(), JsonObject::new())
            .unwrap();
    }
    let (first, second) = {
        let a = receive(&mut posts).await;
        let b = receive(&mut posts).await;
        let WebhookMessage::Occurrence(event) = a.check(&secret) else {
            panic!("event expected")
        };
        if event.event_id == "first" {
            (a, b)
        } else {
            (b, a)
        }
    };
    for (id, post) in [("first", &first), ("second", &second)] {
        assert_eq!(post.headers["x-mcp-subscription-id"], grant.id);
        assert_ne!(post.headers["webhook-id"], id);
        for key in [&secret, &new] {
            assert!(
                matches!(post.check(key), WebhookMessage::Occurrence(event) if event.event_id == id && event.cursor.as_deref() == Some(initial.as_str()))
            );
        }
    }
    second.respond(StatusCode::OK);
    let result = refresh_until(&api, &options, |grant| {
        grant
            .delivery_status
            .as_ref()
            .unwrap()
            .last_delivery_at
            .is_some()
    })
    .await;
    assert_eq!(result.cursor.as_deref(), Some(initial.as_str()));
    first.respond(StatusCode::GONE);
    let result = refresh_until(&api, &options, |grant| {
        grant.cursor.as_deref() != Some(initial.as_str())
    })
    .await;
    assert_eq!(
        result.delivery_status.unwrap().last_error.as_deref(),
        Some("http_4xx")
    );
    emit.emit(
        Some("too-large-at-receiver".into()),
        chrono::Utc::now(),
        JsonObject::new(),
    )
    .unwrap();
    let rejected = receive(&mut posts).await;
    assert!(
        matches!(rejected.check(&new), WebhookMessage::Occurrence(event) if event.event_id == "too-large-at-receiver")
    );
    rejected.respond(StatusCode::PAYLOAD_TOO_LARGE);
    let at = chrono::Utc::now();
    let mut oversized = rmcp::events::wire::EventOccurrence::new(
        "oversized",
        "source",
        at,
        serde_json::from_value(json!({"large": ""})).unwrap(),
    );
    let padding = 256 * 1024 - serde_json::to_vec(&oversized).unwrap().len();
    oversized
        .data
        .insert("large".into(), json!("x".repeat(padding)));
    // The occurrence fits the producer's budget, but the delivery's
    // non-null watermark enlarges its JSON body.
    emit.emit(Some(oversized.event_id), at, oversized.data)
        .unwrap();
    let gap = receive(&mut posts).await;
    assert!(
        gap.headers["webhook-id"]
            .to_str()
            .unwrap()
            .starts_with("msg_gap_")
    );
    assert!(matches!(gap.check(&new), WebhookMessage::Gap { .. }));
    gap.respond(StatusCode::OK);
    let result = refresh_until(&api, &options, |grant| grant.truncated).await;
    assert_ne!(result.cursor.as_deref(), Some(initial.as_str()));

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    emit.emit(
        Some("rotated".into()),
        chrono::Utc::now(),
        JsonObject::new(),
    )
    .unwrap();
    let rotated = receive(&mut posts).await;
    assert!(
        matches!(rotated.check(&new), WebhookMessage::Occurrence(event) if event.event_id == "rotated")
    );
    assert!(rotated.verify(&secret).is_err());
    rotated.respond(StatusCode::OK);

    api.unsubscribe_webhook("source", args(), &url)
        .await
        .unwrap();
    assert!(
        matches!(api.unsubscribe_webhook("source", args(), &url).await, Err(EventsError::NotFound { kind }) if kind.as_deref() == Some("subscription"))
    );
    let again = subscribe(&api, WebhookOptions::new(&url, new.clone()))
        .await
        .unwrap();
    assert_eq!(again.id, grant.id);
    runtime.remove("source").unwrap();
    let terminated = receive(&mut posts).await;
    assert!(
        matches!(terminated.check(&new), WebhookMessage::Terminated { error } if matches!(EventsError::from(error.clone()), EventsError::NotFound { kind } if kind.as_deref() == Some("event")))
    );
    terminated.respond(StatusCode::OK);
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn egress_requires_auth_tls_and_consent_and_never_follows_redirects() {
    let (url, mut posts, callback) = receiver().await;
    for (authenticated, allowed, denied, code) in [
        (false, true, false, -32024),
        (true, true, true, -32024),
        (true, false, false, -32027),
    ] {
        let runtime = runtime(authenticated, allowed);
        let _emit = runtime
            .register_emit(descriptor().authorize(move |_, _, _| async move {
                if denied {
                    Err(EventsError::Forbidden { reason: None })
                } else {
                    Ok(())
                }
            }))
            .unwrap();
        let (api, stop, service, peer) = access(runtime).await;
        let err = subscribe(&api, WebhookOptions::new(&url, WebhookSecret::generate()))
            .await
            .unwrap_err();
        assert_eq!(rmcp::ErrorData::from(err).code.0, code);
        assert!(posts.try_recv().is_err());
        close(peer, stop, service).await;
    }

    let runtime = runtime(true, true);
    let _emit = runtime.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(runtime).await;
    let secret = WebhookSecret::generate();
    let http = url.replacen("https", "http", 1);
    assert!(matches!(
        subscribe(&api, WebhookOptions::new(http, secret.clone())).await,
        Err(EventsError::InvalidParams(_))
    ));
    let (result, ()) = tokio::join!(
        subscribe(&api, WebhookOptions::new(&url, secret.clone())),
        async {
            let post = receive(&mut posts).await;
            assert_eq!(post.path, "/hook");
            assert!(matches!(
                post.check(&secret),
                WebhookMessage::Verification { .. }
            ));
            let redirect = Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header("location", url.replace("/hook", "/redirected"))
                .body(Body::from("endpoint-private"))
                .unwrap();
            post.reply.send(redirect).unwrap();
        }
    );
    let error = rmcp::ErrorData::from(result.unwrap_err());
    assert_eq!(error.code.0, -32027);
    assert_eq!(error.data, Some(json!({"reason":"challenge_failed"})));
    assert!(posts.try_recv().is_err());
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn concurrent_subscribes_share_one_verification() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let _first = runtime.register_emit(descriptor()).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let _second = runtime
        .register_emit(
            Descriptor::typed::<JsonObject, JsonObject>("other", "", [DeliveryMode::Webhook])
                .authorize({
                    let entered = entered.clone();
                    move |_, _, _| {
                        entered.notify_one();
                        std::future::ready(Ok(()))
                    }
                }),
        )
        .unwrap();
    let (api, stop, service, peer) = access(runtime).await;
    let options = WebhookOptions::new(url, WebhookSecret::generate());
    let first = subscribe(&api, options.clone());
    tokio::pin!(first);
    let post = tokio::select! {
        result = &mut first => panic!("first subscribe finished before verification: {result:?}"),
        post = receive(&mut posts) => post,
    };
    let second = api.subscribe_webhook("other", None, options.clone());
    tokio::pin!(second);
    tokio::select! {
        result = &mut second => panic!("second subscribe finished before verification: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(3), entered.notified()) => result.unwrap(),
    }
    post.echo(&options.secret);
    let (first, second) = tokio::join!(first, second);
    assert_ne!(first.unwrap().id, second.unwrap().id);
    assert!(posts.try_recv().is_err());
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn callback_and_rpc_timeouts_have_distinct_errors() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let _emit = runtime.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(runtime.clone()).await;
    let options = WebhookOptions::new(url, WebhookSecret::generate());
    let request = subscribe(&api, options.clone());
    tokio::pin!(request);
    let post = tokio::select! {
        result = &mut request => panic!("subscribe finished before verification: {result:?}"),
        post = receive(&mut posts) => post,
    };
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::resume();
    let err = rmcp::ErrorData::from(request.await.unwrap_err());
    assert_eq!(err.code.0, -32027);
    assert_eq!(err.data, Some(json!({"reason":"timeout"})));
    drop(post);

    let entered = Arc::new(tokio::sync::Notify::new());
    runtime
        .replace(
            descriptor().authorize({
                let entered = entered.clone();
                move |_, _, _| {
                    entered.notify_one();
                    std::future::pending()
                }
            }),
            Replacement::Additive,
        )
        .unwrap();
    let request = subscribe(&api, options);
    tokio::pin!(request);
    tokio::select! {
        result = &mut request => panic!("subscribe finished during authorization: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(3), entered.notified()) => result.unwrap(),
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(12)).await;
    tokio::time::resume();
    let err = rmcp::ErrorData::from(request.await.unwrap_err());
    assert_eq!(err.code.0, -32603);
    assert!(err.data.is_none());
    assert!(posts.try_recv().is_err());
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn configured_dns_and_tls_trust_are_required() {
    let (literal, mut posts, callback) = receiver().await;
    let hostname = literal.replace("127.0.0.1", "localhost");
    let denied = runtime(true, false);
    let _emit = denied.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(denied).await;
    let error = subscribe(
        &api,
        WebhookOptions::new(&hostname, WebhookSecret::generate()),
    )
    .await
    .unwrap_err();
    let error = rmcp::ErrorData::from(error);
    assert_eq!(error.code.0, -32027);
    assert_eq!(error.data, Some(json!({"reason":"connection_refused"})));
    assert!(posts.try_recv().is_err());
    close(peer, stop, service).await;

    let mut no_root = configured_egress(true);
    no_root.additional_root_certificates.clear();
    let untrusted = EventsServer::new()
        .with_webhooks(|_| Some("fixture".into()), no_root)
        .unwrap();
    let _emit = untrusted.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(untrusted).await;
    let error = subscribe(
        &api,
        WebhookOptions::new(&hostname, WebhookSecret::generate()),
    )
    .await
    .unwrap_err();
    assert_eq!(
        rmcp::ErrorData::from(error).data,
        Some(json!({"reason":"tls_error"}))
    );
    assert!(posts.try_recv().is_err());
    close(peer, stop, service).await;

    let trusted = runtime(true, true);
    let _emit = trusted.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(trusted).await;
    let options = WebhookOptions::new(&hostname, WebhookSecret::generate());
    activate(&api, &mut posts, &options, || {}).await;
    api.unsubscribe_webhook("source", args(), &hostname)
        .await
        .unwrap();
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn revocation_on_refresh_terminates_the_grant() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let allowed = Arc::new(AtomicBool::new(true));
    let on_request = allowed.clone();
    let _emit = runtime
        .register_emit(descriptor().authorize(move |_, _, _| {
            std::future::ready(if on_request.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(EventsError::Forbidden {
                    reason: Some("revoked".into()),
                })
            })
        }))
        .unwrap();
    let (api, stop, service, peer) = access(runtime).await;
    let options = WebhookOptions::new(url, WebhookSecret::generate());
    activate(&api, &mut posts, &options, || {}).await;
    allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        subscribe(&api, options.clone()).await,
        Err(EventsError::Forbidden { reason }) if reason.as_deref() == Some("revoked")
    ));
    let post = receive(&mut posts).await;
    assert!(matches!(
        post.check(&options.secret),
        WebhookMessage::Terminated { error } if error.code.0 == -32024 && error.data == Some(json!({"reason":"revoked"}))
    ));
    // Keep the old worker's termination POST in flight during the fresh subscribe.
    allowed.store(true, Ordering::SeqCst);
    let fresh = subscribe(&api, options).await.unwrap();
    assert!(fresh.delivery_status.is_none());
    post.respond(StatusCode::OK);
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn unrelated_changes_do_not_restart_webhook_authorization_or_stall_retries() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let emit = runtime.register_emit(descriptor()).unwrap();
    let (api, stop, service, peer) = access(runtime.clone()).await;
    let options = WebhookOptions::new(url, WebhookSecret::generate());
    activate(&api, &mut posts, &options, || {}).await;
    emit.emit(Some("retry".into()), chrono::Utc::now(), JsonObject::new())
        .unwrap();
    let post = receive(&mut posts).await;
    let message = post.headers["webhook-id"].clone();

    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    runtime
        .replace(
            descriptor().authorize({
                let (calls, entered) = (calls.clone(), entered.clone());
                move |_, _, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    std::future::pending()
                }
            }),
            Replacement::Additive,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let _unrelated = runtime
        .register_emit(Descriptor::typed::<JsonObject, JsonObject>(
            "unrelated",
            "",
            [DeliveryMode::Webhook],
        ))
        .unwrap();
    post.respond(StatusCode::SERVICE_UNAVAILABLE);
    let retry = receive(&mut posts).await;
    assert_eq!(retry.headers["webhook-id"], message);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    retry.respond(StatusCode::OK);
    close(peer, stop, service).await;
    callback.abort();
}

#[tokio::test]
async fn retry_preserves_message_and_expiry_cancels_worker() {
    let (url, mut posts, callback) = receiver().await;
    let runtime = runtime(true, true);
    let (seen_tx, mut seen_rx) = mpsc::channel(8);
    runtime
        .register_check(descriptor(), move |ctx, _, query| {
            let seen = seen_tx.clone();
            async move {
                match query.cursor.as_deref() {
                    None => {
                        let mut batch = CheckedBatch::new("head");
                        batch.next_poll_ms = Some(1);
                        Ok(batch)
                    }
                    Some("head") => {
                        let mut batch = CheckedBatch::new("one");
                        batch.next_poll_ms = Some(1);
                        batch.events.push(rmcp::events::wire::EventOccurrence::new(
                            "upstream",
                            "source",
                            chrono::Utc::now(),
                            JsonObject::new(),
                        ));
                        Ok(batch)
                    }
                    _ => {
                        let _ = seen.try_send(ctx.ct.clone());
                        std::future::pending().await
                    }
                }
            }
        })
        .unwrap();
    let (api, stop, service, peer) = access(runtime).await;
    let secret = WebhookSecret::generate();
    let mut opts = WebhookOptions::new(url, secret.clone());
    opts.ttl = WebhookTtl::Milliseconds(20_000);
    let grant = activate(&api, &mut posts, &opts, || {}).await;
    assert_eq!(grant.cursor.as_deref(), Some("head"));
    let first = receive(&mut posts).await;
    let id = first.headers["webhook-id"].clone();
    let timestamp = first.headers["webhook-timestamp"].clone();
    let signature = first.headers["webhook-signature"].clone();
    let body = first.body.clone();
    first.respond(StatusCode::SERVICE_UNAVAILABLE);
    let retry = receive(&mut posts).await;
    assert_eq!(retry.headers["webhook-id"], id);
    assert_ne!(retry.headers["webhook-timestamp"], timestamp);
    assert_ne!(retry.headers["webhook-signature"], signature);
    assert_eq!(retry.body, body);
    assert!(
        matches!(retry.check(&secret), WebhookMessage::Occurrence(event) if event.event_id == "upstream")
    );
    retry.respond(StatusCode::OK);
    let token = tokio::time::timeout(Duration::from_secs(6), seen_rx.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(20)).await;
    tokio::time::timeout(Duration::from_secs(6), token.cancelled())
        .await
        .unwrap();
    tokio::time::resume();
    assert!(
        matches!(api.unsubscribe_webhook("source", args(), &opts.url).await, Err(EventsError::NotFound { kind }) if kind.as_deref() == Some("subscription"))
    );
    close(peer, stop, service).await;
    callback.abort();
}
