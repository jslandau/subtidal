//! Static source-scan guards. These tests read the crate's own source files
//! and fail when an invariant that cannot be expressed in the type system is
//! violated — e.g. an RT callback regressing to direct unchecked slicing, a
//! CF Create-rule result re-wrapped with `from_void`, or a startup path
//! short-circuiting model verification on bare existence.
//!
//! Each guard names the finding it pins (F-numbers from the remediation plan).

use std::path::Path;

fn src(rel: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    std::fs::read_to_string(Path::new(manifest).join(rel))
        .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

/// Strip `//` line comments so guards match code, not discussion of the code.
fn strip_comments(code: &str) -> String {
    code.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// F2: the PipeWire RT process closure must route through the validated
/// `push_pw_chunk` wrapper and must not contain a direct unchecked
/// `&bytes[offset..offset + size]` slice or a panicking `bytemuck::cast_slice`.
#[test]
fn static_pw_callback_uses_push_pw_chunk() {
    let code = src("src/audio/impl_linux.rs");
    assert!(
        code.contains("push_pw_chunk("),
        "PipeWire process callback must call push_pw_chunk (src/audio/impl_linux.rs)"
    );
    assert!(
        !code.contains("&bytes[offset..offset + size]"),
        "PipeWire RT callback must not slice chunk bytes directly"
    );
    assert!(
        !code.contains("bytemuck::cast_slice"),
        "PipeWire RT callback must not use panicking bytemuck::cast_slice; \
         use the validated wrapper instead"
    );
}

/// F7: Core Audio Get-rule results (+1 Create-rule references) must be adopted
/// with `wrap_under_create_rule`, never `from_void` (whose `ItemRef` never
/// releases, leaking one reference per call).
#[test]
fn static_no_from_void_on_get_rule_results() {
    for rel in [
        "src/audio/impl_macos/tap.rs",
        "src/audio/impl_macos/tap_processes.rs",
        "src/audio/impl_macos/mod.rs",
        "src/audio/impl_macos/notify.rs",
    ] {
        let code = strip_comments(&src(rel));
        assert!(
            !code.contains("from_void"),
            "{rel}: from_void must not be used on Get-rule CF results; \
             use wrap_under_create_rule to adopt the +1 reference"
        );
    }
}

/// F5: startup must always run the digest-aware `ensure_*` path before any
/// engine construction — no existence-only short-circuit in front of it
/// (that would load unverified pre-planted model files).
#[test]
fn static_startup_always_ensures_models() {
    for rel in ["src/main_linux.rs", "src/main_macos.rs"] {
        let code = strip_comments(&src(rel));
        assert!(
            code.contains("ensure_nemotron_models"),
            "{rel}: startup must call models::ensure_nemotron_models unconditionally"
        );
        assert!(
            code.contains("ensure_diarization_models"),
            "{rel}: startup must call models::ensure_diarization_models unconditionally"
        );
        for gone in ["nemotron_models_present", "diarization_models_present"] {
            assert!(
                !code.contains(gone),
                "{rel}: existence-only `{gone}` short-circuit must not gate model verification"
            );
        }
    }
}

/// Extract the body of a named function from a source file (crude but
/// sufficient for guards): from `fn <name>` to the next line that is exactly
/// `}` at zero indentation (or `}` ending the item).
fn fn_body<'a>(code: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}");
    let start = code
        .find(&needle)
        .unwrap_or_else(|| panic!("function {name} not found"));
    let end = code[start..]
        .find("\n}")
        .map(|i| start + i)
        .unwrap_or(code.len());
    &code[start..end]
}

/// F12: `exit_without_atexit` must flush stdout and stderr before the raw
/// `libc::_exit`, which skips stdio flush (buffered output would be lost).
#[test]
fn static_exit_flushes_before_raw_exit() {
    let code = strip_comments(&src("src/main_linux.rs"));
    let body = fn_body(&code, "exit_without_atexit");
    let flush_out = body.find("stdout().flush()").expect("stdout flush");
    let flush_err = body.find("stderr().flush()").expect("stderr flush");
    let exit = body.find("libc::_exit").expect("raw _exit");
    assert!(
        flush_out < exit && flush_err < exit,
        "both flushes must precede libc::_exit"
    );
}

/// F19: `post_user_notification` must route through the dispatch2 main-queue
/// hop (UNUserNotificationCenter is main-thread-only), not call the center
/// directly from an arbitrary thread.
#[test]
fn static_notify_posts_via_dispatch2() {
    let code = strip_comments(&src("src/audio/impl_macos/notify.rs"));
    let body = fn_body(&code, "post_user_notification");
    assert!(
        body.contains("dispatch2::DispatchQueue::main().exec_async"),
        "post_user_notification must hop to the main queue via dispatch2"
    );
    // The actual center access lives in the main-thread-only helper.
    let post_body = fn_body(&code, "post_user_notification_on_main");
    assert!(
        post_body.contains("MainThreadMarker"),
        "the posting helper must assert main-thread affinity"
    );
    assert!(
        post_body.contains("addNotificationRequest_withCompletionHandler"),
        "the posting helper performs the center access"
    );
}

/// F20: the aggregate device dictionary must be created with the CFType
/// callbacks (not NULL), the result null-checked, and the aggregate UID must
/// include the process-global atomic counter (rebuild collision window).
#[test]
fn static_aggregate_dict_uses_type_callbacks() {
    let code = strip_comments(&src("src/audio/impl_macos/tap.rs"));
    let body = fn_body(&code, "create_aggregate_device");
    assert!(
        body.contains("kCFTypeDictionaryKeyCallBacks")
            && body.contains("kCFTypeDictionaryValueCallBacks"),
        "CFDictionaryCreateMutable must receive the kCFType callbacks"
    );
    assert!(
        !body.contains("std::ptr::null(),\n        std::ptr::null(),"),
        "CFDictionaryCreateMutable must not pass NULL callbacks"
    );
    assert!(
        body.contains("agg_dict.is_null()"),
        "the created dictionary must be null-checked"
    );
    assert!(
        body.contains("AGGREGATE_COUNTER"),
        "the aggregate UID must include the process-global counter"
    );
}
