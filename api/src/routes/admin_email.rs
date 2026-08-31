// /api/admin/email — port of ui/src/routes/api/admin.email.ts.
// Transactional email config. GET → config with secrets MASKED (set-flags
// only). PUT → config patch (the write); POST { test: true } → send a test
// to the caller.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    NumKind, as_object, nullish_member, optional_boolean_member, optional_enum_member,
    optional_max_string_member, optional_number_member, parse,
};
use crate::email::{EmailConfigPatch, Provider, email_shell, get_email_config, send_email, EmailInput, SendOutcome, set_email_config};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_admin};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

pub async fn get(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    let cfg = get_email_config(&state.pg).await;
    Json(serde_json::json!({
        "config": {
            "provider": match cfg.provider {
                Some(Provider::Smtp) => Value::String("smtp".into()),
                Some(Provider::Resend) => Value::String("resend".into()),
                None => Value::Null,
            },
            "from": cfg.from,
            "smtp": {
                "host": cfg.smtp.host,
                "port": cfg.smtp.port,
                "secure": cfg.smtp.secure,
                "user": cfg.smtp.user,
                "passSet": cfg.smtp.pass_enc.is_some(),
            },
            "resend": { "apiKeySet": cfg.resend.api_key_enc.is_some() },
        }
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let actor = actor_of(&user);
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // z.object({ provider: z.enum(['smtp','resend']).nullable().optional(),
    //             from: z.string().max(200).optional(),
    //             smtp: z.object({ host: max(200) opt, port: int(1..65535)
    //                               opt, secure: bool opt, user: max(200) opt,
    //                               pass: max(500) nullable opt }).optional(),
    //             resend: z.object({ apiKey: max(200) nullable opt })
    //         }) — keys in schema order, rejections in zod's own words.
    // provider/pass/apiKey are `.nullable().optional()` — setEmailConfig's
    // tri-state: undefined keeps, null clears, a string replaces.
    let provider = match nullish_member(obj, "provider", |o, k| {
        optional_enum_member(o, k, &["smtp", "resend"]).map(|p| {
            p.map(|p| if p == "smtp" { Provider::Smtp } else { Provider::Resend })
        })
    }) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let from = match optional_max_string_member(obj, "from", 200) {
        Ok(f) => f,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let (smtp_host, smtp_port, smtp_secure, smtp_user, smtp_pass) =
        match obj.get("smtp") {
            None => (None, None, None, None, None),
            Some(v) => {
                let s = match v.as_object() {
                    Some(s) => s,
                    None => {
                        return house_error(
                            StatusCode::BAD_REQUEST,
                            &crate::body::object_msg(crate::body::zod_type_name(v)),
                        )
                    }
                };
                let host = match optional_max_string_member(s, "host", 200) {
                    Ok(h) => h,
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
                let port = match optional_number_member(s, "port", NumKind::Int, 1.0, 65535.0) {
                    Ok(p) => p.map(|f| f as u16),
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
                let secure = match optional_boolean_member(s, "secure") {
                    Ok(b) => b,
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
                let user = match optional_max_string_member(s, "user", 200) {
                    Ok(u) => u,
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
                let pass = match nullish_member(s, "pass", |o, k| {
                    optional_max_string_member(o, k, 500)
                }) {
                    Ok(p) => p,
                    Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                };
                (host, port, secure, user, pass)
            }
        };
    let resend_api_key = match obj.get("resend") {
        None => None,
        Some(v) => {
            let r = match v.as_object() {
                Some(r) => r,
                None => {
                    return house_error(
                        StatusCode::BAD_REQUEST,
                        &crate::body::object_msg(crate::body::zod_type_name(v)),
                    )
                }
            };
            match nullish_member(r, "apiKey", |o, k| optional_max_string_member(o, k, 200)) {
                Ok(k) => k,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            }
        }
    };
    let patch = EmailConfigPatch {
        provider,
        from,
        smtp_host,
        smtp_port,
        smtp_secure,
        smtp_user,
        smtp_pass,
        resend_api_key,
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/email] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    if let Err(e) = set_email_config(&state.pg, &sb, &patch).await {
        tracing::error!("[admin/email] config write failed: {e}");
        return thrown_internal_error();
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action: "email.config",
            target_type: "email",
            target_id: Some("config"),
            target_label: None,
            before: None,
            after: Some(serde_json::json!({
                "provider": obj.get("provider").cloned().unwrap_or(Value::Null),
            })),
        },
    )
    .await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub async fn post(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.object({ test: z.literal(true) })
    if obj.get("test") != Some(&Value::Bool(true)) {
        return house_error(StatusCode::BAD_REQUEST, "Invalid input: expected true");
    }
    let Some(to) = user.email.clone() else {
        return house_error(
            StatusCode::BAD_REQUEST,
            "your account has no email to test against",
        );
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => {
            tracing::error!("[admin/email] secretbox unavailable: {e}");
            return thrown_internal_error();
        }
    };
    let html = email_shell(
        "It works",
        "<p>Your transactional email configuration delivers. This is a test message from Talaria.</p>",
        crate::email::DEFAULT_FOOTER,
    );
    match send_email(
        &state.pg,
        &sb,
        &EmailInput {
            to,
            subject: "Talaria test email".into(),
            html,
            text: Some("Your transactional email configuration delivers.".into()),
            headers: Vec::new(),
        },
    )
    .await
    {
        SendOutcome::Sent => Json(serde_json::json!({ "ok": true })).into_response(),
        SendOutcome::Failed(e) => house_error(StatusCode::BAD_GATEWAY, &e),
    }
}
