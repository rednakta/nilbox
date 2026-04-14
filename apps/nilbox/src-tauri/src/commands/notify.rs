use tauri::{command, AppHandle, Runtime};

/// Send an OS notification. On macOS uses UNUserNotificationCenter directly
/// so notifications appear under "nilbox" in System Settings > Notifications.
/// Setting the style to "Alerts" makes them stay until dismissed.
#[command]
pub async fn send_os_notification<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
) {
    #[cfg(target_os = "macos")]
    {
        // UNUserNotificationCenter completion handlers require the main thread's
        // NSRunLoop. Dispatch the entire notification flow to the main thread.
        let _ = app.run_on_main_thread(move || {
            macos::send_un_notification(title, body);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, title, body);
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent,
        UNNotificationRequest, UNUserNotificationCenter,
    };

    #[link(name = "UserNotifications", kind = "framework")]
    extern "C" {}

    /// Returns true if the process has a valid bundle identifier, which is
    /// required by UNUserNotificationCenter.
    fn has_bundle_identifier() -> bool {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some()
    }

    pub fn send_un_notification(title: String, body: String) {
        if !has_bundle_identifier() {
            tracing::warn!("No bundle identifier — skipping UNUserNotificationCenter");
            return;
        }

        // requestAuthorizationWithOptions is async — its completion handler is called
        // on the main queue. We send the notification inside the handler so it only
        // fires after authorization is confirmed.
        let notify_block: RcBlock<dyn Fn(Bool, *mut NSError)> =
            RcBlock::new(move |granted: Bool, _: *mut NSError| {
                if !granted.as_bool() {
                    return;
                }
                let center = objc2::exception::catch(|| {
                        UNUserNotificationCenter::currentNotificationCenter()
                    });
                let center = match center {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("UNUserNotificationCenter error: {:?}", e);
                        return;
                    }
                };
                let content = UNMutableNotificationContent::new();
                content.setTitle(&NSString::from_str(&title));
                content.setBody(&NSString::from_str(&body));

                // Reusing the same ID replaces the existing notification (no stacking).
                let id = NSString::from_str("nilbox-domain-notify");
                let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                    &id,
                    &**content,
                    None,
                );
                center.addNotificationRequest_withCompletionHandler(&request, None);
            });

        let center = objc2::exception::catch(|| {
            UNUserNotificationCenter::currentNotificationCenter()
        });
        let center = match center {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("UNUserNotificationCenter error: {:?}", e);
                return;
            }
        };
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &*notify_block,
        );
    }
}
