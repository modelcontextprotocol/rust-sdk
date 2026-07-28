//! Guards for the v3 feature-flag taxonomy (#1067).
//!
//! Building with every feature at once cannot catch an alias that stopped
//! forwarding, because the replacement is enabled separately anyway. The
//! static assertions below check the forwarding itself, so a typo like
//! `transport-io = []` fails the build instead of silently becoming a no-op.

use rmcp::model::{ContentBlock, ElicitRequestParams, PromptMessage, Role};

#[cfg(feature = "transport-io")]
const _: () = assert!(
    cfg!(feature = "transport-stdio"),
    "`transport-io` must forward to `transport-stdio`"
);

#[cfg(feature = "which-command")]
const _: () = assert!(
    cfg!(feature = "transport-child-process"),
    "`which-command` must forward to `transport-child-process`"
);

#[cfg(feature = "reqwest")]
const _: () = assert!(
    cfg!(feature = "tls-rustls"),
    "`reqwest` must forward to `tls-rustls`"
);

#[cfg(feature = "reqwest-native-tls")]
const _: () = assert!(
    cfg!(feature = "tls-native"),
    "`reqwest-native-tls` must forward to `tls-native`"
);

#[cfg(feature = "reqwest-tls-no-provider")]
const _: () = assert!(
    cfg!(feature = "tls-no-provider"),
    "`reqwest-tls-no-provider` must forward to `tls-no-provider`"
);

#[cfg(feature = "local")]
const _: () = assert!(cfg!(feature = "unsync"), "`local` must forward to `unsync`");

/// The `elicitation` flag is now a no-op, so the model must build unconditionally.
#[test]
fn elicitation_params_need_no_feature_flag() {
    let params = ElicitRequestParams::UrlElicitationParams {
        meta: None,
        message: "Please confirm at the following URL".to_string(),
        url: "https://example.com/confirm".to_string(),
        elicitation_id: "elicit_123".to_string(),
    };

    assert!(matches!(
        params,
        ElicitRequestParams::UrlElicitationParams { .. }
    ));
}

/// The `base64` flag is now a no-op, so the image helpers must build unconditionally.
#[test]
fn image_prompt_helper_needs_no_feature_flag() {
    let message = PromptMessage::new_image(Role::User, b"rmcp", "image/png", None, None);

    assert!(matches!(message.content, ContentBlock::Image(_)));
}
