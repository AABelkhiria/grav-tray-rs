#![deny(unsafe_op_in_unsafe_fn)]

use grav_tray_rs::launch_agent;
use grav_tray_rs::quota::{
    QuotaBucket, QuotaSummary, enabled_buckets, fetch_quota, selected_fraction, selection_key,
    validate_selection,
};
use grav_tray_rs::settings::{Settings, VALID_REFRESH_INTERVALS};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBox, NSBoxType,
    NSButton, NSCellImagePosition, NSColor, NSControlSize, NSFont, NSFontWeightRegular, NSImage,
    NSImageSymbolConfiguration, NSImageSymbolScale, NSImageView, NSLineBreakMode, NSMenu,
    NSMenuItem, NSPopUpButton, NSPopover, NSPopoverBehavior, NSProgressIndicator,
    NSProgressIndicatorStyle, NSStatusBar, NSStatusBarButton, NSStatusItem, NSTextAlignment,
    NSTextField, NSTitlePosition, NSVariableStatusItemLength, NSView, NSViewController,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRectEdge,
    NSSize, NSString, NSTimer,
};
use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

const WIDTH: f64 = 400.0;
const PADDING: f64 = 16.0;
const HEADER_HEIGHT: f64 = 28.0;
const FOOTER_HEIGHT: f64 = 42.0;
const SETTINGS_MIN_HEIGHT: f64 = 320.0;

type FetchResult = Result<(QuotaSummary, u16), String>;

struct AppState {
    _status_item: Retained<NSStatusItem>,
    status_button: Retained<NSStatusBarButton>,
    popover: Retained<NSPopover>,
    info_popover: Option<Retained<NSPopover>>,
    view_controller: Retained<NSViewController>,
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

        #[unsafe(method(applicationDidResignActive:))]
        fn application_did_resign_active(&self, _notification: &NSNotification) {
            self.close_popovers();
        }
    }

    impl Delegate {
        #[unsafe(method(poll:))]
        fn poll(&self, _timer: &NSTimer) {
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
            self.rebuild_ui();
        }

        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: &AnyObject) {
            self.start_refresh();
            self.rebuild_ui();
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
            self.rebuild_ui();
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
            self.rebuild_ui();
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
            self.rebuild_ui();
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
            self.show_info_popover(sender, title, message);
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

        let (sender, receiver) = mpsc::channel();
        let state = AppState {
            _status_item: status_item,
            status_button,
            popover,
            info_popover: None,
            view_controller,
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

        self.rebuild_ui();
        self.start_refresh();
        if std::env::var_os("GRAV_TRAY_OPEN_ON_LAUNCH").is_some() {
            self.toggle_popover();
        }
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

    fn show_info_popover(&self, sender: &NSButton, title: &str, message: &str) {
        let mtm = self.mtm();
        let minimum_width = 190.0;
        let maximum_width = 280.0;
        let horizontal_padding = 16.0;
        let vertical_padding = 16.0;
        let title_message_gap = 8.0;
        let maximum_content_width = maximum_width - horizontal_padding * 2.0;
        let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, maximum_width, 1.0));

        let title_label = add_text(
            &root,
            mtm,
            title,
            rect(0.0, 0.0, maximum_content_width, 18.0),
            &NSFont::boldSystemFontOfSize(13.0),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        let message_label = add_text(
            &root,
            mtm,
            message,
            rect(0.0, 0.0, maximum_content_width, 1.0),
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Left,
        );
        message_label.setMaximumNumberOfLines(0);
        message_label.setUsesSingleLineMode(false);
        message_label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);

        let natural_content_width = title_label
            .sizeThatFits(NSSize::new(f64::MAX, f64::MAX))
            .width
            .max(
                message_label
                    .sizeThatFits(NSSize::new(f64::MAX, f64::MAX))
                    .width,
            );
        let width =
            (natural_content_width + horizontal_padding * 2.0).clamp(minimum_width, maximum_width);
        let content_width = width - horizontal_padding * 2.0;
        message_label.setPreferredMaxLayoutWidth(content_width);

        let title_height = title_label
            .sizeThatFits(NSSize::new(content_width, f64::MAX))
            .height
            .ceil();
        let message_height = message_label
            .sizeThatFits(NSSize::new(content_width, f64::MAX))
            .height
            .ceil();
        let height = vertical_padding * 2.0 + title_height + title_message_gap + message_height;
        root.setFrame(rect(0.0, 0.0, width, height));
        title_label.setFrame(rect(
            horizontal_padding,
            vertical_padding + message_height + title_message_gap,
            content_width,
            title_height,
        ));
        message_label.setFrame(rect(
            horizontal_padding,
            vertical_padding,
            content_width,
            message_height,
        ));

        let view_controller = NSViewController::new(mtm);
        view_controller.setView(&root);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        popover.setContentSize(NSSize::new(width, height));
        popover.setContentViewController(Some(&view_controller));
        popover.showRelativeToRect_ofView_preferredEdge(sender.bounds(), sender, NSRectEdge::MaxX);

        if let Some(state) = self.ivars().state.borrow_mut().as_mut() {
            if let Some(previous) = state.info_popover.replace(popover)
                && previous.isShown()
            {
                previous.close();
            }
        }
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

        let stale = self.ivars().state.borrow().as_ref().is_some_and(|state| {
            state.last_updated.is_none_or(|updated| {
                updated.elapsed() >= Duration::from_secs(state.settings.refresh_interval_seconds)
            })
        });
        if stale {
            self.start_refresh();
        }
        self.rebuild_ui();

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
            if let Some(info_popover) = state.info_popover.as_ref() {
                info_popover.close();
            }
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
            self.rebuild_ui();
        }
    }

    fn rebuild_ui(&self) {
        let mut state_ref = self.ivars().state.borrow_mut();
        let Some(state) = state_ref.as_mut() else {
            return;
        };

        update_status_button(state);
        let height = content_height(state);
        let root = if state.showing_settings {
            build_settings_view(self.mtm(), state, self, height)
        } else {
            build_quota_view(self.mtm(), state, self, height)
        };
        state.view_controller.setView(&root);
        state.popover.setContentSize(NSSize::new(WIDTH, height));
        state.last_view_render = Some(Instant::now());
    }
}

fn content_height(state: &AppState) -> f64 {
    let body = if let Some(summary) = state.summary.as_ref() {
        let cards: Vec<usize> = summary
            .groups
            .iter()
            .map(|group| {
                group
                    .buckets
                    .iter()
                    .filter(|bucket| bucket.is_enabled())
                    .count()
            })
            .filter(|count| *count > 0)
            .collect();
        let gaps = cards.len().saturating_sub(1) as f64 * 12.0;
        cards
            .iter()
            .map(|count| 38.0 + *count as f64 * 43.0)
            .sum::<f64>()
            + gaps
    } else {
        84.0
    };
    let error = if state.quota_error.is_some() || state.action_error.is_some() {
        42.0
    } else {
        0.0
    };
    (PADDING + HEADER_HEIGHT + 14.0 + body + 14.0 + error + FOOTER_HEIGHT + PADDING)
        .max(SETTINGS_MIN_HEIGHT)
}

fn build_quota_view(
    mtm: MainThreadMarker,
    state: &AppState,
    target: &Delegate,
    height: f64,
) -> Retained<NSView> {
    let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, WIDTH, height));
    let header_y = height - PADDING - HEADER_HEIGHT + 6.0;
    add_header(&root, mtm, state, target, header_y);

    let mut top = header_y - 6.0;
    if let Some(summary) = state.summary.as_ref() {
        for group in &summary.groups {
            let buckets: Vec<_> = group
                .buckets
                .iter()
                .filter(|bucket| bucket.is_enabled())
                .collect();
            if buckets.is_empty() {
                continue;
            }
            let card_height = 38.0 + buckets.len() as f64 * 43.0;
            let bottom = top - card_height;
            add_rounded_box(
                &root,
                mtm,
                rect(PADDING, bottom, WIDTH - PADDING * 2.0, card_height),
                &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.05),
                10.0,
            );
            add_text(
                &root,
                mtm,
                &group.display_name,
                rect(PADDING + 12.0, top - 29.0, 330.0, 18.0),
                &NSFont::boldSystemFontOfSize(13.0),
                &NSColor::labelColor(),
                NSTextAlignment::Left,
            );
            let mut row_top = top - 38.0;
            for bucket in buckets {
                add_quota_row(&root, mtm, bucket, row_top);
                row_top -= 43.0;
            }
            top = bottom - 12.0;
        }
    } else {
        let bottom = top - 84.0;
        add_rounded_box(
            &root,
            mtm,
            rect(PADDING, bottom, WIDTH - PADDING * 2.0, 84.0),
            &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.08),
            10.0,
        );
        add_symbol(
            &root,
            mtm,
            "antenna.radiowaves.left.and.right",
            rect(PADDING + 12.0, top - 32.0, 18.0, 18.0),
            &NSColor::labelColor(),
        );
        add_text(
            &root,
            mtm,
            "Looking for Antigravity",
            rect(PADDING + 36.0, top - 32.0, 300.0, 18.0),
            &NSFont::boldSystemFontOfSize(12.0),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        add_text(
            &root,
            mtm,
            "Open agy and make sure you are signed in. Grav Tray connects directly to its local quota service.",
            rect(PADDING + 12.0, bottom + 10.0, WIDTH - 56.0, 34.0),
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Left,
        );
    }

    if let Some(error) = state
        .quota_error
        .as_deref()
        .or(state.action_error.as_deref())
    {
        add_symbol(
            &root,
            mtm,
            "exclamationmark.triangle.fill",
            rect(PADDING, 50.0, 16.0, 16.0),
            &NSColor::systemOrangeColor(),
        );
        add_text(
            &root,
            mtm,
            error,
            rect(PADDING + 22.0, 46.0, WIDTH - 54.0, 34.0),
            &NSFont::systemFontOfSize(10.5),
            &NSColor::systemOrangeColor(),
            NSTextAlignment::Left,
        );
    }

    add_separator(&root, mtm, 40.0);
    let refresh = image_text_button(
        mtm,
        "Refresh",
        "arrow.clockwise",
        sel!(refresh:),
        target,
        rect(0.0, 7.0, 92.0, 28.0),
    );
    refresh.setEnabled(!state.loading);
    root.addSubview(&refresh);

    if let Some(updated) = state.last_updated {
        add_text(
            &root,
            mtm,
            &freshness(updated),
            rect(WIDTH - 120.0 - PADDING, 12.0, 120.0, 17.0),
            &NSFont::systemFontOfSize(10.5),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Right,
        );
    }
    root
}

fn build_settings_view(
    mtm: MainThreadMarker,
    state: &AppState,
    target: &Delegate,
    height: f64,
) -> Retained<NSView> {
    let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, WIDTH, height));
    let header_y = height - PADDING - HEADER_HEIGHT + 6.0;
    add_header(&root, mtm, state, target, header_y);
    let mut y = header_y - 28.0;

    let general_label = add_text(
        &root,
        mtm,
        "General",
        rect(PADDING, y, 65.0, 18.0),
        &NSFont::systemFontOfSize(12.0),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );
    let general_width = general_label
        .sizeThatFits(NSSize::new(f64::MAX, 18.0))
        .width
        .ceil();
    general_label.setFrame(rect(PADDING, y, general_width, 18.0));
    let info = icon_button(
        mtm,
        "info.circle",
        sel!(showInfo:),
        target,
        rect(PADDING + general_width + 2.0, y - 3.0, 24.0, 24.0),
    );
    info.setTag(1);
    root.addSubview(&info);
    y -= 37.0;

    add_text(
        &root,
        mtm,
        "Refresh interval",
        rect(PADDING, y + 4.0, 140.0, 18.0),
        &NSFont::systemFontOfSize(11.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    let interval_picker = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        rect(WIDTH - PADDING - 138.0, y, 138.0, 26.0),
        false,
    );
    for interval in VALID_REFRESH_INTERVALS {
        interval_picker.addItemWithTitle(&NSString::from_str(refresh_interval_label(interval)));
    }
    let selected_interval = VALID_REFRESH_INTERVALS
        .iter()
        .position(|interval| *interval == state.settings.refresh_interval_seconds)
        .unwrap_or(1);
    interval_picker.selectItemAtIndex(selected_interval as isize);
    unsafe {
        interval_picker.setTarget(Some(target.as_any_object()));
        interval_picker.setAction(Some(sel!(setRefreshInterval:)));
    }
    root.addSubview(&interval_picker);
    y -= 38.0;

    add_text(
        &root,
        mtm,
        "Tray quota",
        rect(PADDING, y + 4.0, 100.0, 18.0),
        &NSFont::systemFontOfSize(11.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    let quota_picker = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        rect(WIDTH - PADDING - 210.0, y, 210.0, 26.0),
        false,
    );
    if let Some(summary) = state.summary.as_ref() {
        let buckets = enabled_buckets(summary);
        let mut selected = 0;
        for (index, (group, bucket)) in buckets.iter().enumerate() {
            quota_picker.addItemWithTitle(&NSString::from_str(&format!(
                "{} — {}",
                group.display_name, bucket.display_name
            )));
            if selection_key(group, bucket) == state.settings.menu_bar_quota {
                selected = index;
            }
        }
        quota_picker.selectItemAtIndex(selected as isize);
    } else {
        quota_picker.addItemWithTitle(&NSString::from_str("Waiting for quota data"));
        quota_picker.setEnabled(false);
    }
    unsafe {
        quota_picker.setTarget(Some(target.as_any_object()));
        quota_picker.setAction(Some(sel!(selectMenuBarQuota:)));
    }
    root.addSubview(&quota_picker);

    let system_y = 52.0;
    add_separator(&root, mtm, system_y + 54.0);
    let system_label = add_text(
        &root,
        mtm,
        "System",
        rect(PADDING, system_y + 24.0, 60.0, 18.0),
        &NSFont::systemFontOfSize(12.0),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );
    let system_width = system_label
        .sizeThatFits(NSSize::new(f64::MAX, 18.0))
        .width
        .ceil();
    system_label.setFrame(rect(PADDING, system_y + 24.0, system_width, 18.0));
    let system_info = icon_button(
        mtm,
        "info.circle",
        sel!(showInfo:),
        target,
        rect(PADDING + system_width + 2.0, system_y + 22.0, 24.0, 24.0),
    );
    system_info.setTag(2);
    root.addSubview(&system_info);
    add_text(
        &root,
        mtm,
        "Launch at Login",
        rect(PADDING, system_y, 150.0, 20.0),
        &NSFont::systemFontOfSize(12.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    let launch_checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(""),
            Some(target.as_any_object()),
            Some(sel!(toggleLaunchAtLogin:)),
            mtm,
        )
    };
    launch_checkbox.setFrame(rect(WIDTH - PADDING - 20.0, system_y, 20.0, 20.0));
    launch_checkbox.setState(if launch_agent::is_installed() { 1 } else { 0 });
    root.addSubview(&launch_checkbox);

    if let Some(error) = state.action_error.as_deref() {
        add_text(
            &root,
            mtm,
            error,
            rect(PADDING, system_y - 28.0, WIDTH - PADDING * 2.0, 24.0),
            &NSFont::systemFontOfSize(10.5),
            &NSColor::systemOrangeColor(),
            NSTextAlignment::Left,
        );
    }

    add_separator(&root, mtm, 40.0);
    let quit = image_text_button(
        mtm,
        "Quit Grav Tray",
        "power",
        sel!(quit:),
        target,
        rect(0.0, 7.0, 120.0, 28.0),
    );
    quit.setContentTintColor(Some(&NSColor::systemRedColor()));
    root.addSubview(&quit);
    let shortcut_info = icon_button(
        mtm,
        "info.circle",
        sel!(showInfo:),
        target,
        rect(WIDTH - PADDING - 24.0, 9.0, 24.0, 24.0),
    );
    shortcut_info.setTag(3);
    root.addSubview(&shortcut_info);
    root
}

fn add_header(root: &NSView, mtm: MainThreadMarker, state: &AppState, target: &Delegate, y: f64) {
    add_symbol(
        root,
        mtm,
        "gauge.with.dots.needle.50percent",
        rect(PADDING, y, 28.0, 28.0),
        &NSColor::controlAccentColor(),
    );
    add_text(
        root,
        mtm,
        "Grav Tray",
        rect(PADDING + 30.0, y + 3.0, 70.0, 20.0),
        &NSFont::boldSystemFontOfSize(13.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    add_text(
        root,
        mtm,
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        rect(PADDING + 100.0, y + 3.0, 90.0, 17.0),
        &NSFont::systemFontOfSize(9.5),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );

    if state.loading {
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            rect(WIDTH - PADDING - 54.0, y + 5.0, 16.0, 16.0),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        unsafe { spinner.startAnimation(None) };
        root.addSubview(&spinner);
    }

    let settings = icon_button(
        mtm,
        "gearshape",
        sel!(toggleSettings:),
        target,
        rect(WIDTH - PADDING - 28.0, y, 32.0, 32.0),
    );
    settings.setToolTip(Some(&NSString::from_str(if state.showing_settings {
        "Close Settings"
    } else {
        "Settings"
    })));
    root.addSubview(&settings);
}

fn add_quota_row(root: &NSView, mtm: MainThreadMarker, bucket: &QuotaBucket, top: f64) {
    let fraction = bucket.remaining_fraction.map(|value| value.clamp(0.0, 1.0));
    let color = progress_color(fraction);
    add_text(
        root,
        mtm,
        &bucket.display_name,
        rect(PADDING + 12.0, top - 19.0, 118.0, 17.0),
        &NSFont::boldSystemFontOfSize(10.5),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    add_text(
        root,
        mtm,
        &bucket.reset_label(SystemTime::now()),
        rect(PADDING + 126.0, top - 19.0, 160.0, 17.0),
        &NSFont::systemFontOfSize(9.5),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );
    let percent_text = bucket
        .percent()
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "—".to_owned());
    add_text(
        root,
        mtm,
        &percent_text,
        rect(WIDTH - PADDING - 62.0, top - 21.0, 50.0, 20.0),
        &NSFont::boldSystemFontOfSize(14.0),
        &color,
        NSTextAlignment::Right,
    );

    let track_x = PADDING + 12.0;
    let track_width = WIDTH - PADDING * 2.0 - 24.0;
    add_rounded_box(
        root,
        mtm,
        rect(track_x, top - 31.0, track_width, 5.0),
        &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.14),
        2.5,
    );
    if let Some(fraction) = fraction {
        add_rounded_box(
            root,
            mtm,
            rect(track_x, top - 31.0, track_width * fraction, 5.0),
            &color,
            2.5,
        );
    }
}

fn add_text(
    root: &NSView,
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    font: &NSFont,
    color: &NSColor,
    alignment: NSTextAlignment,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setFont(Some(font));
    label.setTextColor(Some(color));
    label.setAlignment(alignment);
    label.setMaximumNumberOfLines(2);
    root.addSubview(&label);
    label
}

fn add_symbol(root: &NSView, mtm: MainThreadMarker, name: &str, frame: NSRect, color: &NSColor) {
    if let Some(image) = system_image(name) {
        let image_view = NSImageView::imageViewWithImage(&image, mtm);
        image_view.setFrame(frame);
        image_view.setContentTintColor(Some(color));
        root.addSubview(&image_view);
    }
}

fn add_rounded_box(
    root: &NSView,
    mtm: MainThreadMarker,
    frame: NSRect,
    color: &NSColor,
    radius: f64,
) {
    if frame.size.width <= 0.0 {
        return;
    }
    let background = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    background.setBoxType(NSBoxType::Custom);
    background.setTitlePosition(NSTitlePosition::NoTitle);
    background.setBorderWidth(0.0);
    background.setFillColor(color);
    background.setCornerRadius(radius);
    root.addSubview(&background);
}

fn add_separator(root: &NSView, mtm: MainThreadMarker, y: f64) {
    add_rounded_box(
        root,
        mtm,
        rect(PADDING, y, WIDTH - PADDING * 2.0, 1.0),
        &NSColor::separatorColor(),
        0.0,
    );
}

fn icon_button(
    mtm: MainThreadMarker,
    symbol: &str,
    action: Sel,
    target: &Delegate,
    frame: NSRect,
) -> Retained<NSButton> {
    let button = if let Some(image) = system_image(symbol) {
        unsafe {
            NSButton::buttonWithTitle_image_target_action(
                &NSString::from_str(""),
                &image,
                Some(target.as_any_object()),
                Some(action),
                mtm,
            )
        }
    } else {
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(""),
                Some(target.as_any_object()),
                Some(action),
                mtm,
            )
        }
    };
    button.setFrame(frame);
    button.setBordered(false);
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    button
}

fn image_text_button(
    mtm: MainThreadMarker,
    title: &str,
    symbol: &str,
    action: Sel,
    target: &Delegate,
    frame: NSRect,
) -> Retained<NSButton> {
    let button = if let Some(image) = system_image(symbol) {
        unsafe {
            NSButton::buttonWithTitle_image_target_action(
                &NSString::from_str(title),
                &image,
                Some(target.as_any_object()),
                Some(action),
                mtm,
            )
        }
    } else {
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(target.as_any_object()),
                Some(action),
                mtm,
            )
        }
    };
    button.setFrame(frame);
    button.setBordered(false);
    button.setImagePosition(NSCellImagePosition::ImageLeading);
    button.setImageHugsTitle(true);
    button.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    button
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

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn progress_color(fraction: Option<f64>) -> Retained<NSColor> {
    match fraction {
        None => NSColor::secondaryLabelColor(),
        Some(value) if value <= 0.10 => NSColor::systemRedColor(),
        Some(value) if value <= 0.30 => NSColor::systemOrangeColor(),
        Some(_) => NSColor::systemGreenColor(),
    }
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

fn freshness(updated: Instant) -> String {
    let elapsed = updated.elapsed().as_secs();
    match elapsed {
        0..=9 => "Live".to_owned(),
        10..=59 => format!("{elapsed}s ago"),
        _ => format!("{}m ago", elapsed / 60),
    }
}

fn refresh_interval_label(seconds: u64) -> &'static str {
    match seconds {
        30 => "30 seconds",
        60 => "1 minute",
        300 => "5 minutes",
        900 => "15 minutes",
        _ => "Custom",
    }
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("Grav Tray must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
