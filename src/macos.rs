use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use eframe::egui;
use objc2::ffi;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};
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

type OpenUrlsImp = extern "C" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>);

/// Winit 0.30 owns `NSApplication.delegate`. Replacing that object makes
/// `sendEvent:` abort. Finder / Dock opens still need `application:openURLs:`,
/// so the method is added onto the live delegate class instead.
fn install_open_file_methods() {
    let mtm = MainThreadMarker::new().expect("PinkDown must start on the macOS main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate =
        unsafe { app.delegate() }.expect("winit sets NSApplication.delegate during EventLoop::build");
    let cls: &AnyClass = unsafe { msg_send![&delegate, class] };
    add_open_urls(cls);
}

fn add_open_urls(cls: &AnyClass) {
    let name = sel!(application:openURLs:);
    let cls_ptr = cls as *const AnyClass as *mut ffi::objc_class;
    let imp: Imp = unsafe { std::mem::transmute(application_open_urls as OpenUrlsImp) };
    let added = unsafe { ffi::class_addMethod(cls_ptr, name.as_ptr(), Some(imp), c"v@:@@".as_ptr()) };
    debug_assert!(
        Bool::from_raw(added).as_bool() || cls.instance_method(name).is_some(),
        "failed to add application:openURLs: on {}",
        cls.name()
    );
}

extern "C" fn application_open_urls(
    _this: &AnyObject,
    _cmd: Sel,
    _application: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    if let Some(path) = urls.iter().filter_map(|url| file_url_path(&url)).last() {
        enqueue(path);
    }
}

pub fn run(options: eframe::NativeOptions, initial_path: Option<PathBuf>) -> eframe::Result<()> {
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    install_open_file_methods();

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
