//! macOS user notification helper for audio source disappearance events.
//! Uses UNUserNotificationCenter for posting notifications when a captured
//! application exits (AC3.4 fallback banner).

use anyhow::Result;
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_foundation::NSString;
use objc2_user_notifications::{
    UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NOTIFICATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Post a user notification with the given title and body.
///
/// Uses UNUserNotificationCenter to display a notification banner.
/// If notification permission has not been granted, the notification
/// silently fails to display (but this function still returns Ok).
/// The audio watchdog and on-screen error messages provide fallback
/// notification of source disappearance.
///
/// **Thread safety:** May be called from any thread; UNUserNotificationCenter
/// is main-thread-only, so this hops to the main queue via dispatch2 when
/// called off-main (F19 — the code now honors the contract this doc always
/// promised) and posts directly when already on main.
pub fn post_user_notification(title: &str, body: &str) -> Result<()> {
    let count = NOTIFICATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    if MainThreadMarker::new().is_some() {
        return post_user_notification_on_main(title, body, count);
    }
    let title = title.to_string();
    let body = body.to_string();
    dispatch2::DispatchQueue::main().exec_async(move || {
        if let Err(e) = post_user_notification_on_main(&title, &body, count) {
            eprintln!("warn: failed to post user notification: {e:#}");
        }
    });
    Ok(())
}

/// Main-thread-only implementation of [`post_user_notification`].
/// `count` seeds the unique request identifier.
fn post_user_notification_on_main(title: &str, body: &str, count: u64) -> Result<()> {
    let _mtm = MainThreadMarker::new()
        .expect("UNUserNotificationCenter access must happen on the main thread");

    // Create notification content with title and body.
    let content = UNMutableNotificationContent::new();

    let title_ns = NSString::from_str(title);
    content.setTitle(&*title_ns);

    let body_ns = NSString::from_str(body);
    content.setBody(&*body_ns);

    // Generate a unique identifier for this notification request.
    let identifier = NSString::from_str(&format!("subtidal-notification-{}", count));

    // Create notification request with immediate delivery (trigger = nil).
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &*identifier,
        &content,
        None, // nil trigger = immediate delivery
    );

    // Post the notification via the default center (no completion handler needed).
    let center = UNUserNotificationCenter::currentNotificationCenter();

    // No completion handler needed; we don't care about the result.
    // Pass None to indicate no completion handler.
    center.addNotificationRequest_withCompletionHandler(&request, None);

    Ok(())
}

/// Request user notification authorization (banner permission).
///
/// Called once at startup before audio capture begins. This surfaces the
/// macOS notification permission prompt if the user hasn't previously answered.
/// If the user denies permission, subsequent `post_user_notification` calls
/// silently fail to display (but the function still returns Ok).
///
/// This is best-effort: we don't check the result or retry. The audio watchdog
/// and in-panel captions provide fallback notification of problems.
///
/// **Thread safety:** Must be called on the main thread. Typically called
/// early in `main_macos.rs` startup, before audio workers begin.
pub fn request_authorization_best_effort() {
    let _mtm = MainThreadMarker::new()
        .expect("request_authorization_best_effort must be called on the main thread");

    let center = UNUserNotificationCenter::currentNotificationCenter();

    // Request authorization with .alert option (banner notifications).
    // The Objective-C API copies the completion block for the asynchronous
    // authorization request; keep the Rust owner alive through this call and
    // let normal Drop release it afterward.
    let options = objc2_user_notifications::UNAuthorizationOptions::Alert;

    let block = RcBlock::new(|_, _| {
        // No-op: ignore both result and error
    });

    center.requestAuthorizationWithOptions_completionHandler(options, &block);

    // The system owns its copied completion handler for the authorization flow;
    // dropping the local owner here avoids a permanent process-lifetime leak.
}

#[cfg(test)]
#[path = "notify_test.rs"]
mod tests;
