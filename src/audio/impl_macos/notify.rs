//! macOS user notification helper for audio source disappearance events.
//! Uses UNUserNotificationCenter for posting notifications when a captured
//! application exits (AC3.4 fallback banner).

use anyhow::Result;
use objc2::MainThreadMarker;
use objc2_foundation::NSString;
use objc2_user_notifications::{UNUserNotificationCenter, UNMutableNotificationContent, UNNotificationRequest};
use std::sync::atomic::{AtomicU64, Ordering};
use block2::RcBlock;

static NOTIFICATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Post a user notification with the given title and body.
///
/// Uses UNUserNotificationCenter to display a notification banner.
/// If notification permission has not been granted, the notification
/// silently fails to display (but this function still returns Ok).
/// The audio watchdog and on-screen error messages provide fallback
/// notification of source disappearance.
///
/// **Thread safety:** May be called from any thread; internally marshals
/// to the main thread via dispatch2 if needed (UNUserNotificationCenter
/// is main-thread-only).
pub fn post_user_notification(title: &str, body: &str) -> Result<()> {
    // Create notification content with title and body.
    let content = UNMutableNotificationContent::new();

    let title_ns = NSString::from_str(title);
    content.setTitle(&*title_ns);

    let body_ns = NSString::from_str(body);
    content.setBody(&*body_ns);

    // Generate a unique identifier for this notification request.
    let count = NOTIFICATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let identifier = NSString::from_str(&format!("subtidal-notification-{}", count));

    // Create notification request with immediate delivery (trigger = nil).
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &*identifier,
        &content,
        None,  // nil trigger = immediate delivery
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
    // The block must outlive the async call; we leak it intentionally since it's
    // a fire-and-forget operation. The system retains ownership during the auth flow.
    let options = objc2_user_notifications::UNAuthorizationOptions::Alert;

    let block = RcBlock::new(|_, _| {
        // No-op: ignore both result and error
    });

    center.requestAuthorizationWithOptions_completionHandler(options, &block);

    // Leak the block intentionally. The system holds a reference to it during the
    // auth flow, and this function is fire-and-forget.
    std::mem::forget(block);
}

#[cfg(test)]
#[path = "notify_test.rs"]
mod tests;
