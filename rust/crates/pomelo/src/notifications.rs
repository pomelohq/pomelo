//! macOS notifications for agent events. Notification Center only serves a bundled app, so a bare
//! binary (tests, `cargo run`) skips them instead of crashing inside the framework.

use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

const IDENTIFIER_PREFIX: &str = "pomelo-agent:";
/// Banner and sound even while the app is frontmost: the event may concern another workspace.
const PRESENT: usize = (1 << 4) | (1 << 1);

/// The workspace branch of the notification the user clicked, for the app to switch to.
static CLICKED: Mutex<Option<String>> = Mutex::new(None);

declare_class!(
    struct Delegate;

    unsafe impl ClassType for Delegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "PomeloNotificationDelegate";
    }

    impl DeclaredClass for Delegate {}

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions(PRESENT),));
        }

        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::Block<dyn Fn()>,
        ) {
            // SAFETY: plain getters on the delivered response.
            let identifier = unsafe { response.notification().request().identifier() }.to_string();
            if let Some(branch) = identifier
                .strip_prefix(IDENTIFIER_PREFIX)
                .and_then(|rest| rest.split_once(':'))
                .map(|(branch, _)| branch.to_string())
            {
                if let Ok(mut clicked) = CLICKED.lock() {
                    *clicked = Some(branch);
                }
                ui::wake();
            }
            handler.call(());
        }
    }
);

struct Center {
    center: Retained<UNUserNotificationCenter>,
    _delegate: Retained<Delegate>,
}

// SAFETY: the notification center is documented thread-safe; the delegate holds no state.
unsafe impl Send for Center {}

static CENTER: Mutex<Option<Center>> = Mutex::new(None);
static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Asks for permission once (the system remembers the answer) and installs the delegate.
pub fn start() {
    // SAFETY: reading the main bundle's identifier.
    let bundled = unsafe { NSBundle::mainBundle().bundleIdentifier() }.is_some();
    if !bundled {
        return;
    }
    // SAFETY: standard Notification Center setup from the main thread.
    unsafe {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate: Retained<Delegate> = msg_send_id![Delegate::alloc(), init];
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let done = block2::RcBlock::new(|_granted: objc2::runtime::Bool, _error: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions(
                UNAuthorizationOptions::UNAuthorizationOptionAlert.0
                    | UNAuthorizationOptions::UNAuthorizationOptionSound.0,
            ),
            &done,
        );
        if let Ok(mut slot) = CENTER.lock() {
            *slot = Some(Center {
                center,
                _delegate: delegate,
            });
        }
    }
}

/// Posts a notification; clicking it later reports `branch` through `take_clicked`.
pub fn post(title: &str, body: &str, branch: &str) {
    let Ok(slot) = CENTER.lock() else {
        return;
    };
    let Some(center) = slot.as_ref() else {
        return;
    };
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let identifier = format!("{IDENTIFIER_PREFIX}{branch}:{sequence}");
    // SAFETY: building and adding a request with owned Foundation objects.
    unsafe {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&identifier),
            &content,
            None,
        );
        center
            .center
            .addNotificationRequest_withCompletionHandler(&request, None);
    }
}

pub fn take_clicked() -> Option<String> {
    CLICKED.lock().ok().and_then(|mut clicked| clicked.take())
}
