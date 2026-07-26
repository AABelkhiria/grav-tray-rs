#![deny(unsafe_op_in_unsafe_fn)]

mod ui;

use self::ui::{AppUi, UiState, WIDTH, content_height};
use grav_tray::launch_agent;
use grav_tray::quota::{
    QuotaSummary, enabled_buckets, fetch_quota, selected_fraction, selection_key,
    validate_selection,
};
use grav_tray::settings::{Settings, VALID_REFRESH_INTERVALS};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSButton,
    NSCellImagePosition, NSFontWeightRegular, NSImage, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSMenu, NSMenuItem, NSPopUpButton, NSPopover, NSPopoverBehavior,
    NSStatusBar, NSStatusBarButton, NSStatusItem, NSVariableStatusItemLength, NSViewController,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRectEdge, NSSize, NSString,
    NSTimer,
};
use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

type FetchResult = Result<(QuotaSummary, u16), String>;

struct AppState {
    _status_item: Retained<NSStatusItem>,
    status_button: Retained<NSStatusBarButton>,
    popover: Retained<NSPopover>,
    _view_controller: Retained<NSViewController>,
    ui: AppUi,
    _main_menu: Retained<NSMenu>,
    settings: Settings,
    summary: Option<QuotaSummary>,
    quota_error: Option<String>,
    action_error: Option<String>,
    connected_port: Option<u16>,
    last_updated: Option<Instant>,
    last_refresh_started: Option<Instant>,
    last_view_render: Option<Instant>,
    last_quit_attempt: Option<Instant>,
    showing_settings: bool,
    pending_popover_open: bool,
    loading: bool,
    home_directory: PathBuf,
    sender: Sender<FetchResult>,
    receiver: Receiver<FetchResult>,
}

#[derive(Default)]
struct AppDelegateIvars {
    state: RefCell<Option<AppState>>,
    poll_timer: OnceCell<Retained<NSTimer>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. Delegate is main-thread-only
    // because every AppKit object in its state must remain on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: NSApplicationDelegate has no additional safety requirements.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            self.finish_launching();
        }
    }

    impl Delegate {
        #[unsafe(method(poll:))]
        fn poll(&self, _timer: &NSTimer) {
            let should_open = self
                .ivars()
                .state
                .borrow_mut()
                .as_mut()
                .is_some_and(|state| std::mem::take(&mut state.pending_popover_open));
            if should_open {
                self.toggle_popover();
            }
            self.poll_results();
        }

        #[unsafe(method(togglePopover:))]
        fn toggle_popover_action(&self, _sender: &AnyObject) {
            self.toggle_popover();
        }

        #[unsafe(method(toggleSettings:))]
        fn toggle_settings(&self, _sender: &AnyObject) {
            if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                state.showing_settings = !state.showing_settings;
            }
            self.update_ui();
        }

        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: &AnyObject) {
            self.start_refresh();
            self.update_ui();
        }

        #[unsafe(method(setRefreshInterval:))]
        fn set_refresh_interval(&self, sender: &NSPopUpButton) {
            let index = sender.indexOfSelectedItem().max(0) as usize;
            let Some(interval) = VALID_REFRESH_INTERVALS.get(index).copied() else {
                return;
            };
            if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                state.settings.refresh_interval_seconds = interval;
                state.action_error = state.settings.save().err().map(|error| error.to_string());
            }
            self.update_ui();
        }

        #[unsafe(method(selectMenuBarQuota:))]
        fn select_menu_bar_quota(&self, sender: &NSPopUpButton) {
            let index = sender.indexOfSelectedItem().max(0) as usize;
            if let Some(state) = self.ivars().state.borrow_mut().as_mut()
                && let Some(summary) = state.summary.as_ref()
                && let Some((group, bucket)) = enabled_buckets(summary).get(index)
            {
                state.settings.menu_bar_quota = selection_key(group, bucket);
                state.action_error = state.settings.save().err().map(|error| error.to_string());
            }
            self.update_ui();
        }

        #[unsafe(method(toggleLaunchAtLogin:))]
        fn toggle_launch_at_login(&self, _sender: &NSButton) {
            let result = if launch_agent::is_installed() {
                launch_agent::remove()
            } else {
                launch_agent::write_for_current_executable().map(|_| ())
            };
            if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                state.action_error = result
                    .err()
                    .map(|error| format!("Could not update the login item: {error}"));
            }
            self.update_ui();
        }

        #[unsafe(method(showInfo:))]
        fn show_info(&self, sender: &NSButton) {
            let (title, message) = match sender.tag() {
                1 => (
                    "General Settings",
                    "Quota data is read from the authenticated Antigravity service running locally on this Mac.",
                ),
                2 => (
                    "System Settings",
                    "Launch at Login starts Grav Tray automatically after you sign in to your Mac.",
                ),
                _ => (
                    "General Shortcuts",
                    "⌘ 0   : Toggle Settings\n⌘ R   : Refresh Quota\n⌘ Q Q : Quit App",
                ),
            };
            if let Some(state) = self.ivars().state.borrow().as_ref() {
                state.ui.show_info(sender, title, message);
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }

        #[unsafe(method(quitShortcut:))]
        fn quit_shortcut(&self, _sender: &AnyObject) {
            let now = Instant::now();
            let should_quit = self
                .ivars()
                .state
                .borrow()
                .as_ref()
                .and_then(|state| state.last_quit_attempt)
                .is_some_and(|previous| now.duration_since(previous) < Duration::from_millis(500));
            if should_quit {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            } else if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                state.last_quit_attempt = Some(now);
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        // SAFETY: The signature of NSObject's init method is correct.
        unsafe { msg_send![super(this), init] }
    }

    fn as_any_object(&self) -> &AnyObject {
        self
    }

    fn finish_launching(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let status_button = status_item
            .button(mtm)
            .expect("status items have a native button");
        status_button.setTitle(&NSString::from_str("--"));
        status_button.setImagePosition(NSCellImagePosition::ImageLeading);
        status_button.setImageHugsTitle(true);
        unsafe {
            status_button.setTarget(Some(self.as_any_object()));
            status_button.setAction(Some(sel!(togglePopover:)));
        }

        let view_controller = NSViewController::new(mtm);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        popover.setContentViewController(Some(&view_controller));

        let main_menu = self.make_shortcut_menu();
        app.setMainMenu(Some(&main_menu));
        let ui = AppUi::new(mtm, self);
        view_controller.setView(ui.root());

        let (sender, receiver) = mpsc::channel();
        let state = AppState {
            _status_item: status_item,
            status_button,
            popover,
            _view_controller: view_controller,
            ui,
            _main_menu: main_menu,
            settings: Settings::load(),
            summary: None,
            quota_error: None,
            action_error: None,
            connected_port: None,
            last_updated: None,
            last_refresh_started: None,
            last_view_render: None,
            last_quit_attempt: None,
            showing_settings: false,
            pending_popover_open: std::env::var_os("GRAV_TRAY_OPEN_ON_LAUNCH").is_some(),
            loading: false,
            home_directory: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            sender,
            receiver,
        };
        self.ivars().state.replace(Some(state));

        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.5,
                self.as_any_object(),
                sel!(poll:),
                None,
                true,
            )
        };
        self.ivars()
            .poll_timer
            .set(timer)
            .expect("poll timer is initialized once");

        self.update_ui();
        self.start_refresh();
    }

    fn make_shortcut_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let main = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Grav Tray"));
        let root = make_menu_item(mtm, "Grav Tray", None, "");
        let commands = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Grav Tray"));
        add_shortcut(
            &commands,
            mtm,
            "Toggle Settings",
            sel!(toggleSettings:),
            "0",
            self,
        );
        add_shortcut(&commands, mtm, "Refresh Quota", sel!(refresh:), "r", self);
        add_shortcut(
            &commands,
            mtm,
            "Quit Grav Tray",
            sel!(quitShortcut:),
            "q",
            self,
        );
        root.setSubmenu(Some(&commands));
        main.addItem(&root);
        main
    }

    fn toggle_popover(&self) {
        let is_shown = self
            .ivars()
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.popover.isShown());
        if is_shown {
            self.close_popovers();
            return;
        }

        let anchor_ready = self
            .ivars()
            .state
            .borrow()
            .as_ref()
            .and_then(|state| state.status_button.window())
            .is_some_and(|window| window.frame().size.height > 0.0);
        if !anchor_ready {
            if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
                state.pending_popover_open = true;
            }
            return;
        }

        let stale = self.ivars().state.borrow().as_ref().is_some_and(|state| {
            state.last_updated.is_none_or(|updated| {
                updated.elapsed() >= Duration::from_secs(state.settings.refresh_interval_seconds)
            })
        });
        if stale {
            self.start_refresh();
        }
        self.update_ui();

        if let Some(state) = self.ivars().state.borrow().as_ref() {
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
            state.popover.showRelativeToRect_ofView_preferredEdge(
                state.status_button.bounds(),
                &state.status_button,
                NSRectEdge::MinY,
            );
        }
    }

    fn close_popovers(&self) {
        if let Some(state) = self.ivars().state.borrow().as_ref() {
            state.ui.close_info();
            state.popover.close();
        }
    }

    fn start_refresh(&self) {
        let (sender, home_directory, preferred_port) = {
            let mut state_ref = self.ivars().state.borrow_mut();
            let Some(state) = state_ref.as_mut() else {
                return;
            };
            if state.loading {
                return;
            }
            state.loading = true;
            state.last_refresh_started = Some(Instant::now());
            (
                state.sender.clone(),
                state.home_directory.clone(),
                state.connected_port,
            )
        };

        std::thread::spawn(move || {
            let result = fetch_quota(&home_directory, preferred_port);
            let _ = sender.send(result);
        });
    }

    fn poll_results(&self) {
        let mut received_result = false;
        let mut refresh_due = false;
        let mut view_due = false;
        {
            let mut state_ref = self.ivars().state.borrow_mut();
            let Some(state) = state_ref.as_mut() else {
                return;
            };

            while let Ok(result) = state.receiver.try_recv() {
                received_result = true;
                state.loading = false;
                match result {
                    Ok((summary, port)) => {
                        state.summary = Some(summary);
                        if let Some(summary) = state.summary.as_ref() {
                            validate_selection(summary, &mut state.settings.menu_bar_quota);
                        }
                        state.connected_port = Some(port);
                        state.last_updated = Some(Instant::now());
                        state.quota_error = None;
                        state.action_error =
                            state.settings.save().err().map(|error| error.to_string());
                    }
                    Err(error) => state.quota_error = Some(error),
                }
            }

            if !state.loading {
                refresh_due = state.last_refresh_started.is_none_or(|started| {
                    started.elapsed()
                        >= Duration::from_secs(state.settings.refresh_interval_seconds)
                });
            }
            if state.popover.isShown() {
                view_due = state
                    .last_view_render
                    .is_none_or(|rendered| rendered.elapsed() >= Duration::from_secs(30));
            }
        }

        if refresh_due {
            self.start_refresh();
        }
        if received_result || refresh_due || view_due {
            self.update_ui();
        }
    }

    fn update_ui(&self) {
        let mut state_ref = self.ivars().state.borrow_mut();
        let Some(state) = state_ref.as_mut() else {
            return;
        };

        update_status_button(state);
        let height = content_height(state);
        let ui_state = UiState {
            settings: &state.settings,
            summary: state.summary.as_ref(),
            quota_error: state.quota_error.as_deref(),
            action_error: state.action_error.as_deref(),
            last_updated: state.last_updated,
            showing_settings: state.showing_settings,
            loading: state.loading,
        };
        if state.ui.update(ui_state, height) {
            state
                .popover
                .setContentSize(NSSize::new(WIDTH, state.ui.content_height()));
        }
        state.last_view_render = Some(Instant::now());
    }
}

fn system_image(name: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    image.setTemplate(true);
    Some(image)
}

fn status_image(name: &str) -> Option<Retained<NSImage>> {
    let image = system_image(name)?;
    let weight = unsafe { NSFontWeightRegular };
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        16.0,
        weight,
        NSImageSymbolScale::Medium,
    );
    let configured = image
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(image);
    configured.setTemplate(true);
    Some(configured)
}

fn update_status_button(state: &AppState) {
    let fraction = state
        .summary
        .as_ref()
        .and_then(|summary| selected_fraction(summary, &state.settings.menu_bar_quota));
    state.status_button.setTitle(&NSString::from_str(
        &fraction.map_or_else(|| "--".to_owned(), |value| format!("{}%", percent(value))),
    ));
    let symbol = fraction.map_or("gauge.with.dots.needle.0percent", gauge_symbol);
    state
        .status_button
        .setImage(status_image(symbol).as_deref());
}

fn percent(fraction: f64) -> u8 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u8
}

fn gauge_symbol(fraction: f64) -> &'static str {
    match fraction {
        value if value < 0.11 => "gauge.with.dots.needle.100percent",
        value if value < 0.31 => "gauge.with.dots.needle.67percent",
        _ => "gauge.with.dots.needle.33percent",
    }
}

fn make_menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

fn add_shortcut(
    menu: &NSMenu,
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key: &str,
    target: &Delegate,
) {
    let item = make_menu_item(mtm, title, Some(action), key);
    unsafe { item.setTarget(Some(target.as_any_object())) };
    menu.addItem(&item);
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("Grav Tray must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
