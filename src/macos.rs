use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{declare_class, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationDelegateReply};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSString, NSURL};
use winit::event_loop::EventLoop;

use crate::app::PinkDown;

static PENDING: Mutex<Option<PathBuf>> = Mutex::new(None);
static REPAINT: OnceLock<egui::Context> = OnceLock::new();

/// Last Finder / Dock open requested since the previous frame.
pub fn take_open_path() -> Option<PathBuf> {
    PENDING.lock().expect("macos open-path queue").take()
}

fn bind_repaint(ctx: &egui::Context) {
    let _ = REPAINT.set(ctx.clone());
}

fn enqueue(path: PathBuf) {
    *PENDING.lock().expect("macos open-path queue") = Some(path);
    if let Some(ctx) = REPAINT.get() {
        ctx.request_repaint();
    }
}

fn file_url_path(url: &NSURL) -> Option<PathBuf> {
    unsafe { url.isFileURL().then(|| url.path()).flatten() }
        .map(|path| PathBuf::from(path.to_string()))
}

declare_class!(
    struct AppDelegate;

    unsafe impl ClassType for AppDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "PinkDownAppDelegate";
    }

    impl DeclaredClass for AppDelegate {
        type Ivars = ();
    }

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[method(application:openURLs:)]
        fn application_open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            if let Some(path) = urls.iter().filter_map(|url| file_url_path(&url)).last() {
                enqueue(path);
            }
        }

        #[method(application:openFiles:)]
        fn application_open_files(
            &self,
            application: &NSApplication,
            filenames: &NSArray<NSString>,
        ) {
            let reply = match filenames.iter().last() {
                Some(filename) => {
                    enqueue(PathBuf::from(filename.to_string()));
                    NSApplicationDelegateReply::Success
                }
                None => NSApplicationDelegateReply::Failure,
            };
            unsafe { application.replyToOpenOrPrint(reply) };
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

pub fn run(options: eframe::NativeOptions, initial_path: Option<PathBuf>) -> eframe::Result<()> {
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;

    // Winit intentionally leaves NSApplication's delegate unset. Install ours
    // after creating its event loop and before AppKit starts dispatching events.
    // NSApplication.delegate is weak — keep `delegate` alive for the run loop.
    let mtm = MainThreadMarker::new().expect("PinkDown must start on the macOS main thread");
    let delegate = AppDelegate::new(mtm);
    let application = NSApplication::sharedApplication(mtm);
    application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let mut app = eframe::create_native(
        "PinkDown",
        options,
        Box::new(move |cc| {
            bind_repaint(&cc.egui_ctx);
            Ok(Box::new(PinkDown::new(cc, initial_path)))
        }),
        &event_loop,
    );
    event_loop.run_app(&mut app)?;

    Ok(())
}
