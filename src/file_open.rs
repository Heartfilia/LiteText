//! OS requests only enter the normal open path; they never save recovered buffers.
use std::path::PathBuf;

pub(crate) fn argument_paths(args: impl IntoIterator<Item = std::ffi::OsString>) -> Vec<PathBuf> {
    let mut args = args.into_iter();
    let mut paths = Vec::new();
    let mut literal = false;
    while let Some(arg) = args.next() {
        if !literal {
            if arg == "--" {
                literal = true;
                continue;
            }
            if arg == "--litetext-update-ack"
                || arg == "--litetext-install-update"
                || arg == "--litetext-window"
            {
                args.next();
                continue;
            }
            if arg.to_string_lossy().starts_with('-') {
                continue;
            }
        }
        paths.push(PathBuf::from(arg));
    }
    paths
}

#[cfg(target_os = "macos")]
pub(crate) mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{MainThreadOnly, define_class, msg_send, sel};
    use objc2_foundation::{
        MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSObject, NSObjectProtocol,
    };
    use std::sync::Mutex;

    static REQUESTS: Mutex<(Vec<PathBuf>, Option<eframe::egui::Context>)> =
        Mutex::new((Vec::new(), None));

    fn event_paths(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
        // '----' is keyDirectObject: the list of file aliases/URLs.
        let list: Option<Retained<NSAppleEventDescriptor>> =
            unsafe { msg_send![event, paramDescriptorForKeyword: u32::from_be_bytes(*b"----")] };
        let Some(list) = list else {
            return Vec::new();
        };
        (1..=list.numberOfItems())
            .filter_map(|index| {
                let url = list.descriptorAtIndex(index)?.fileURLValue()?;
                url.isFileURL().then(|| url.to_file_path()).flatten()
            })
            .collect()
    }

    define_class!(
        // NSObject has no subclassing requirements; lifetime is held by main.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        pub(crate) struct OpenHandler;
        unsafe impl NSObjectProtocol for OpenHandler {}
        impl OpenHandler {
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _notification: &objc2_foundation::NSNotification) {
                self.register();
            }
            #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
            fn open(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                let mut requests = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
                requests.0.extend(event_paths(event));
                if let Some(ctx) = &requests.1 { ctx.request_repaint(); }
            }
        }
    );

    impl OpenHandler {
        fn register(&self) {
            unsafe {
                let _: () = msg_send![&*NSAppleEventManager::sharedAppleEventManager(),
                    setEventHandler: self,
                    andSelector: sel!(handleOpenDocuments:withReplyEvent:),
                    forEventClass: u32::from_be_bytes(*b"aevt"),
                    andEventID: u32::from_be_bytes(*b"odoc")];
            }
        }
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            unsafe { msg_send![super(Self::alloc(mtm).set_ivars(())), init] }
        }
    }

    pub(crate) fn install() -> Retained<OpenHandler> {
        let mtm = MainThreadMarker::new().expect("file opening runs on the main thread");
        // Initialize AppKit before installing handlers; the launch notification
        // below handles its later startup-time registration as well.
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let handler = OpenHandler::new(mtm);
        // AppKit can replace handlers while finishing startup. Re-register at
        // WillFinishLaunching, before it dispatches the initial open-documents event.
        unsafe {
            objc2_foundation::NSNotificationCenter::defaultCenter()
                .addObserver_selector_name_object(
                    &handler,
                    sel!(willFinishLaunching:),
                    Some(&objc2_foundation::NSString::from_str(
                        "NSApplicationWillFinishLaunchingNotification",
                    )),
                    None,
                );
        }
        handler.register();
        handler
    }

    pub(crate) fn bind(ctx: &eframe::egui::Context) {
        REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).1 = Some(ctx.clone());
    }

    pub(crate) fn take() -> Vec<PathBuf> {
        std::mem::take(&mut REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_foundation::NSURL;
        #[test]
        fn apple_event_decodes_multiple_unicode_and_space_paths() {
            let paths =
                ["/Users/test/中文 file.jsonl", "/Users/test/next.ndjson"].map(PathBuf::from);
            let list = NSAppleEventDescriptor::listDescriptor();
            for (i, path) in paths.iter().enumerate() {
                let url = NSURL::from_file_path(path).unwrap();
                list.insertDescriptor_atIndex(
                    &NSAppleEventDescriptor::descriptorWithFileURL(&url),
                    i as isize + 1,
                );
            }
            let event = NSAppleEventDescriptor::recordDescriptor();
            unsafe {
                let _: () = msg_send![&*event, setParamDescriptor: &*list, forKeyword: u32::from_be_bytes(*b"----")];
            }
            assert_eq!(event_paths(&event), paths);
            assert!(event_paths(&NSAppleEventDescriptor::nullDescriptor()).is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_preserve_paths_but_skip_updater_and_finder_flags() {
        let args = [
            "--litetext-update-ack",
            "/private/ack",
            "--litetext-window",
            "window-abc",
            "-psn_0_123",
            "中文 a.jsonl",
            "b.log",
            "--",
            "-notes.txt",
        ];
        assert_eq!(
            argument_paths(args.map(Into::into)),
            ["中文 a.jsonl", "b.log", "-notes.txt"].map(PathBuf::from)
        );
    }
}
