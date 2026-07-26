use super::{AppState, Delegate};
use grav_tray::launch_agent;
use grav_tray::quota::{QuotaBucket, QuotaGroup, QuotaSummary, enabled_buckets, selection_key};
use grav_tray::settings::{Settings, VALID_REFRESH_INTERVALS};
use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSCellImagePosition, NSColor, NSControlSize, NSFont, NSImage,
    NSImageView, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLineBreakMode, NSPopUpButton, NSPopover, NSPopoverBehavior, NSProgressIndicator,
    NSProgressIndicatorStyle, NSStackView, NSStackViewDistribution, NSTextAlignment, NSTextField,
    NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView, NSViewController,
};
use objc2_foundation::{
    MainThreadMarker, NSEdgeInsets, NSPoint, NSRect, NSRectEdge, NSSize, NSString,
};
use std::collections::HashMap;
use std::time::{Instant, SystemTime};

pub(super) const WIDTH: f64 = 400.0;
const HORIZONTAL_PADDING: f64 = 16.0;
const VERTICAL_PADDING: f64 = 8.0;
const CONTENT_HORIZONTAL_PADDING: f64 = 4.0;
const HEADER_HEIGHT: f64 = 28.0;
const FOOTER_HEIGHT: f64 = 32.0;
const SETTINGS_MIN_HEIGHT: f64 = 320.0;
const HEIGHT_EPSILON: f64 = 0.01;
const PAGE_GAP: f64 = 6.0;
const CARD_GAP: f64 = 12.0;
const CARD_HEADER_HEIGHT: f64 = 38.0;
const QUOTA_ROW_HEIGHT: f64 = 43.0;
const CONTROL_HEIGHT: f64 = 28.0;
const PICKER_HEIGHT: f64 = 26.0;
const PROGRESS_HEIGHT: f64 = 5.0;
const ERROR_HEIGHT: f64 = 42.0;
const INFO_MIN_WIDTH: f64 = 190.0;
const INFO_MAX_WIDTH: f64 = 280.0;
const INFO_PADDING: f64 = 16.0;
const INFO_GAP: f64 = 8.0;

pub(super) struct AppUi {
    root: Retained<NSView>,
    header: HeaderViews,
    quota_page: QuotaPageViews,
    settings_page: SettingsPageViews,
    quota_footer: FooterViews,
    settings_footer: FooterViews,
    info: InfoViews,
    content_height: f64,
}

pub(super) struct UiState<'a> {
    pub settings: &'a Settings,
    pub summary: Option<&'a QuotaSummary>,
    pub quota_error: Option<&'a str>,
    pub action_error: Option<&'a str>,
    pub last_updated: Option<Instant>,
    pub showing_settings: bool,
    pub loading: bool,
}

struct HeaderViews {
    root: Retained<NSStackView>,
    settings_button: Retained<NSButton>,
}

struct QuotaPageViews {
    root: Retained<NSStackView>,
    cards: Retained<NSStackView>,
    empty: Retained<NSBox>,
    error: ErrorViews,
    groups: HashMap<String, QuotaGroupViews>,
}

struct QuotaGroupViews {
    card: Retained<NSBox>,
    name: Retained<NSTextField>,
    rows_stack: Retained<NSStackView>,
    height: Retained<NSLayoutConstraint>,
    rows: HashMap<String, QuotaRowViews>,
}

struct QuotaRowViews {
    root: Retained<NSStackView>,
    name: Retained<NSTextField>,
    reset: Retained<NSTextField>,
    percent: Retained<NSTextField>,
    fill: Retained<NSBox>,
    fill_width: Retained<NSLayoutConstraint>,
}

struct SettingsPageViews {
    root: Retained<NSStackView>,
    interval_picker: Retained<NSPopUpButton>,
    quota_picker: Retained<NSPopUpButton>,
    launch_checkbox: Retained<NSButton>,
    error: ErrorViews,
}

struct FooterViews {
    root: Retained<NSStackView>,
    primary_button: Retained<NSButton>,
    freshness: Option<Retained<NSTextField>>,
    spinner: Option<Retained<NSProgressIndicator>>,
}

struct ErrorViews {
    root: Retained<NSStackView>,
    label: Retained<NSTextField>,
}

struct InfoViews {
    popover: Retained<NSPopover>,
    title: Retained<NSTextField>,
    message: Retained<NSTextField>,
}

impl AppUi {
    pub(super) fn new(mtm: MainThreadMarker, target: &Delegate) -> Self {
        let root = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 320.0)),
        );

        let main = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            0.0,
            NSLayoutAttribute::Leading,
        );
        root.addSubview(&main);
        pin_edges(&main, &root, HORIZONTAL_PADDING, VERTICAL_PADDING);

        let header = HeaderViews::new(mtm, target);
        let page_gap_top = fixed_spacer(mtm, PAGE_GAP, true);
        let page_host = NSView::new(mtm);
        page_host.setTranslatesAutoresizingMaskIntoConstraints(false);
        let page_gap_bottom = fixed_spacer(mtm, PAGE_GAP, true);
        let quota_footer = FooterViews::quota(mtm, target);
        let settings_footer = FooterViews::settings(mtm, target);
        let footer_host = NSView::new(mtm);
        footer_host.setTranslatesAutoresizingMaskIntoConstraints(false);

        main.addArrangedSubview(header.root());
        main.addArrangedSubview(&page_gap_top);
        main.addArrangedSubview(&page_host);
        main.addArrangedSubview(&page_gap_bottom);
        main.addArrangedSubview(&footer_host);
        set_height(&footer_host, FOOTER_HEIGHT);
        equal_width(header.root(), &main);
        equal_width(&page_host, &main);
        equal_width(&footer_host, &main);

        let quota_page = QuotaPageViews::new(mtm);
        let settings_page = SettingsPageViews::new(mtm, target);
        page_host.addSubview(&quota_page.root);
        page_host.addSubview(&settings_page.root);
        pin_edges(
            &quota_page.root,
            &page_host,
            CONTENT_HORIZONTAL_PADDING,
            0.0,
        );
        pin_edges(
            &settings_page.root,
            &page_host,
            CONTENT_HORIZONTAL_PADDING,
            0.0,
        );

        footer_host.addSubview(&quota_footer.root);
        footer_host.addSubview(&settings_footer.root);
        pin_edges(&quota_footer.root, &footer_host, 0.0, 0.0);
        pin_edges(&settings_footer.root, &footer_host, 0.0, 0.0);

        let info = InfoViews::new(mtm);

        Self {
            root,
            header,
            quota_page,
            settings_page,
            quota_footer,
            settings_footer,
            info,
            content_height: 0.0,
        }
    }

    pub(super) fn root(&self) -> &NSView {
        &self.root
    }

    pub(super) fn update(&mut self, state: UiState<'_>, height: f64) -> bool {
        self.header.update(state.showing_settings);
        self.show_page(state.showing_settings);
        self.quota_page.update(&state);
        self.settings_page.update(&state);
        self.quota_footer.update_quota(&state);

        if (height - self.content_height).abs() <= HEIGHT_EPSILON {
            false
        } else {
            self.content_height = height;
            true
        }
    }

    pub(super) fn content_height(&self) -> f64 {
        self.content_height
    }

    pub(super) fn show_page(&self, showing_settings: bool) {
        self.quota_page.root.setHidden(showing_settings);
        self.settings_page.root.setHidden(!showing_settings);
        self.quota_footer.root.setHidden(showing_settings);
        self.settings_footer.root.setHidden(!showing_settings);
    }

    pub(super) fn show_info(&self, sender: &NSButton, title: &str, message: &str) {
        self.info.show(sender, title, message);
    }

    pub(super) fn close_info(&self) {
        self.info.popover.close();
    }
}

impl HeaderViews {
    fn new(mtm: MainThreadMarker, target: &Delegate) -> Self {
        let root = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Horizontal,
            0.0,
            NSLayoutAttribute::CenterY,
        );
        let gauge = image_view(mtm, "gauge.with.dots.needle.50percent");
        gauge.setContentTintColor(Some(&NSColor::controlAccentColor()));
        set_size(&gauge, 28.0, 28.0);

        let title = label(
            mtm,
            "Grav Tray",
            &NSFont::boldSystemFontOfSize(13.0),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        let padding = fixed_spacer(mtm, 4.0, false);
        let version = label(
            mtm,
            &format!("v{}", env!("CARGO_PKG_VERSION")),
            &NSFont::systemFontOfSize(9.5),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Left,
        );
        let text = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Horizontal,
            0.0,
            NSLayoutAttribute::Bottom,
        );
        text.addArrangedSubview(&title);
        text.addArrangedSubview(&padding);
        text.addArrangedSubview(&version);
        let spacer = flexible_spacer(mtm, false);

        let settings_button = icon_button(
            mtm,
            "gearshape",
            sel!(toggleSettings:),
            target,
            CONTROL_HEIGHT,
        );

        root.addArrangedSubview(&gauge);
        root.addArrangedSubview(&text);
        root.addArrangedSubview(&spacer);
        root.addArrangedSubview(&settings_button);

        Self {
            root,
            settings_button,
        }
    }

    fn root(&self) -> &NSView {
        &self.root
    }

    fn update(&self, showing_settings: bool) {
        self.settings_button
            .setToolTip(Some(&NSString::from_str(if showing_settings {
                "Close Settings"
            } else {
                "Settings"
            })));
    }
}

impl QuotaPageViews {
    fn new(mtm: MainThreadMarker) -> Self {
        let root = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            0.0,
            NSLayoutAttribute::Leading,
        );
        let cards = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            CARD_GAP,
            NSLayoutAttribute::Leading,
        );
        let empty = make_empty_view(mtm);
        let flexible = flexible_spacer(mtm, true);
        let error = ErrorViews::new(mtm, ERROR_HEIGHT);
        root.addArrangedSubview(&cards);
        root.addArrangedSubview(&empty);
        root.addArrangedSubview(&flexible);
        root.addArrangedSubview(&error.root);
        equal_width(&cards, &root);
        equal_width(&empty, &root);
        equal_width(&error.root, &root);
        empty.setHidden(true);
        error.root.setHidden(true);
        Self {
            root,
            cards,
            empty,
            error,
            groups: HashMap::new(),
        }
    }

    fn update(&mut self, state: &UiState<'_>) {
        let error = state.quota_error.or(state.action_error);
        self.error.update(error);

        let Some(summary) = state.summary.as_ref() else {
            self.cards.setHidden(true);
            self.empty.setHidden(false);
            return;
        };
        self.cards.setHidden(false);
        self.empty.setHidden(true);

        let desired_groups: Vec<_> = summary
            .groups
            .iter()
            .filter(|group| group.buckets.iter().any(QuotaBucket::is_enabled))
            .collect();
        let desired_keys: Vec<_> = desired_groups
            .iter()
            .map(|group| group.display_name.clone())
            .collect();
        let obsolete: Vec<_> = self
            .groups
            .keys()
            .filter(|key| !desired_keys.contains(key))
            .cloned()
            .collect();
        for key in obsolete {
            if let Some(group) = self.groups.remove(&key) {
                self.cards.removeArrangedSubview(&group.card);
                group.card.removeFromSuperview();
            }
        }

        for (index, group) in desired_groups.into_iter().enumerate() {
            let key = group.display_name.clone();
            let is_new = !self.groups.contains_key(&key);
            if is_new {
                let views = QuotaGroupViews::new(self.root.mtm(), group);
                self.cards
                    .insertArrangedSubview_atIndex(&views.card, index as isize);
                equal_width(&views.card, &self.cards);
                self.groups.insert(key.clone(), views);
            }
            let views = self.groups.get_mut(&key).expect("quota group was inserted");
            views.update(group);
            if !is_new {
                self.cards
                    .insertArrangedSubview_atIndex(&views.card, index as isize);
            }
        }
    }
}

impl QuotaGroupViews {
    fn new(mtm: MainThreadMarker, group: &QuotaGroup) -> Self {
        let card = rounded_box(
            mtm,
            &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.05),
            10.0,
        );
        let contents = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            0.0,
            NSLayoutAttribute::Leading,
        );
        card.addSubview(&contents);
        contents
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&card.leadingAnchor(), 12.0)
            .setActive(true);
        contents
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&card.trailingAnchor(), -12.0)
            .setActive(true);
        contents
            .topAnchor()
            .constraintEqualToAnchor_constant(&card.topAnchor(), 11.0)
            .setActive(true);
        contents
            .bottomAnchor()
            .constraintEqualToAnchor(&card.bottomAnchor())
            .setActive(true);

        let name = label(
            mtm,
            &group.display_name,
            &NSFont::boldSystemFontOfSize(13.0),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        let header_gap = fixed_spacer(mtm, 9.0, true);
        let rows_stack = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            0.0,
            NSLayoutAttribute::Leading,
        );
        contents.addArrangedSubview(&name);
        contents.addArrangedSubview(&header_gap);
        contents.addArrangedSubview(&rows_stack);
        set_height(&name, 18.0);
        equal_width(&rows_stack, &contents);

        let height = card
            .heightAnchor()
            .constraintEqualToConstant(CARD_HEADER_HEIGHT);
        height.setActive(true);

        Self {
            card,
            name,
            rows_stack,
            height,
            rows: HashMap::new(),
        }
    }

    fn update(&mut self, group: &QuotaGroup) {
        self.name
            .setStringValue(&NSString::from_str(&group.display_name));
        let buckets: Vec<_> = group
            .buckets
            .iter()
            .filter(|bucket| bucket.is_enabled())
            .collect();
        let desired_keys: Vec<_> = buckets
            .iter()
            .map(|bucket| bucket.bucket_id.clone())
            .collect();
        let obsolete: Vec<_> = self
            .rows
            .keys()
            .filter(|key| !desired_keys.contains(key))
            .cloned()
            .collect();
        for key in obsolete {
            if let Some(row) = self.rows.remove(&key) {
                self.rows_stack.removeArrangedSubview(&row.root);
                row.root.removeFromSuperview();
            }
        }

        for (index, bucket) in buckets.into_iter().enumerate() {
            let key = bucket.bucket_id.clone();
            let is_new = !self.rows.contains_key(&key);
            if is_new {
                let row = QuotaRowViews::new(self.card.mtm());
                self.rows_stack
                    .insertArrangedSubview_atIndex(&row.root, index as isize);
                equal_width(&row.root, &self.rows_stack);
                self.rows.insert(key.clone(), row);
            }
            let row = self.rows.get(&key).expect("quota row was inserted");
            row.update(bucket);
            if !is_new {
                self.rows_stack
                    .insertArrangedSubview_atIndex(&row.root, index as isize);
            }
        }
        self.height
            .setConstant(CARD_HEADER_HEIGHT + self.rows.len() as f64 * QUOTA_ROW_HEIGHT);
    }
}

impl QuotaRowViews {
    fn new(mtm: MainThreadMarker) -> Self {
        let root = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            6.0,
            NSLayoutAttribute::Leading,
        );
        root.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: 0.0,
            bottom: 12.0,
            right: 0.0,
        });
        set_height(&root, QUOTA_ROW_HEIGHT);

        let labels = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Horizontal,
            8.0,
            NSLayoutAttribute::CenterY,
        );
        let name = label(
            mtm,
            "",
            &NSFont::boldSystemFontOfSize(10.5),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        let reset = label(
            mtm,
            "",
            &NSFont::systemFontOfSize(9.5),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Left,
        );
        let spacer = flexible_spacer(mtm, false);
        let percent = label(
            mtm,
            "",
            &NSFont::boldSystemFontOfSize(14.0),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Right,
        );
        set_width(&percent, 50.0);
        labels.addArrangedSubview(&name);
        labels.addArrangedSubview(&reset);
        labels.addArrangedSubview(&spacer);
        labels.addArrangedSubview(&percent);
        set_height(&labels, 20.0);

        let track = rounded_box(
            mtm,
            &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.14),
            PROGRESS_HEIGHT / 2.0,
        );
        set_height(&track, PROGRESS_HEIGHT);
        let fill = rounded_box(mtm, &NSColor::secondaryLabelColor(), PROGRESS_HEIGHT / 2.0);
        track.addSubview(&fill);
        fill.leadingAnchor()
            .constraintEqualToAnchor(&track.leadingAnchor())
            .setActive(true);
        fill.topAnchor()
            .constraintEqualToAnchor(&track.topAnchor())
            .setActive(true);
        fill.bottomAnchor()
            .constraintEqualToAnchor(&track.bottomAnchor())
            .setActive(true);
        let fill_width = fill.widthAnchor().constraintEqualToConstant(0.0);
        fill_width.setActive(true);

        root.addArrangedSubview(&labels);
        root.addArrangedSubview(&track);
        equal_width(&labels, &root);
        equal_width(&track, &root);

        Self {
            root,
            name,
            reset,
            percent,
            fill,
            fill_width,
        }
    }

    fn update(&self, bucket: &QuotaBucket) {
        let fraction = bucket.remaining_fraction.map(|value| value.clamp(0.0, 1.0));
        let color = progress_color(fraction);
        self.name
            .setStringValue(&NSString::from_str(&bucket.display_name));
        self.reset
            .setStringValue(&NSString::from_str(&bucket.reset_label(SystemTime::now())));
        self.percent.setStringValue(&NSString::from_str(
            &bucket
                .percent()
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "—".to_owned()),
        ));
        self.percent.setTextColor(Some(&color));
        self.fill.setFillColor(&color);
        self.fill.setHidden(fraction.is_none());
        self.fill_width.setConstant(
            (WIDTH - HORIZONTAL_PADDING * 2.0 - CONTENT_HORIZONTAL_PADDING * 2.0 - 24.0)
                * fraction.unwrap_or(0.0),
        );
    }
}

impl SettingsPageViews {
    fn new(mtm: MainThreadMarker, target: &Delegate) -> Self {
        let root = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            0.0,
            NSLayoutAttribute::Leading,
        );
        let general_header = section_header(mtm, "General", 1, target);
        let general_gap = fixed_spacer(mtm, 13.0, true);
        let interval_picker = popup_button(mtm, sel!(setRefreshInterval:), target);
        for interval in VALID_REFRESH_INTERVALS {
            interval_picker.addItemWithTitle(&NSString::from_str(refresh_interval_label(interval)));
        }
        let interval_row = settings_row(mtm, "Refresh interval", &interval_picker);
        let row_gap = fixed_spacer(mtm, 4.0, true);
        let quota_picker = popup_button(mtm, sel!(selectMenuBarQuota:), target);
        let quota_row = settings_row(mtm, "Tray quota", &quota_picker);
        let flexible = flexible_spacer(mtm, true);

        let separator = separator(mtm);
        let system_gap = fixed_spacer(mtm, 12.0, true);
        let system_header = section_header(mtm, "System", 2, target);
        let launch_checkbox = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(""),
                Some(target.as_any_object()),
                Some(sel!(toggleLaunchAtLogin:)),
                mtm,
            )
        };
        launch_checkbox.setTranslatesAutoresizingMaskIntoConstraints(false);
        set_size(&launch_checkbox, 20.0, 20.0);
        let launch_row = settings_row(mtm, "Launch at Login", &launch_checkbox);
        let error = ErrorViews::new(mtm, 24.0);
        error.root.setHidden(true);

        for view in [
            general_header.as_ref(),
            general_gap.as_ref(),
            interval_row.as_ref(),
            row_gap.as_ref(),
            quota_row.as_ref(),
            flexible.as_ref(),
            separator.as_ref(),
            system_gap.as_ref(),
            system_header.as_ref(),
            launch_row.as_ref(),
            error.root.as_ref(),
        ] {
            root.addArrangedSubview(view);
            equal_width(view, &root);
        }

        Self {
            root,
            interval_picker,
            quota_picker,
            launch_checkbox,
            error,
        }
    }

    fn update(&self, state: &UiState<'_>) {
        let selected_interval = VALID_REFRESH_INTERVALS
            .iter()
            .position(|interval| *interval == state.settings.refresh_interval_seconds)
            .unwrap_or(1);
        self.interval_picker
            .selectItemAtIndex(selected_interval as isize);

        self.quota_picker.removeAllItems();
        if let Some(summary) = state.summary.as_ref() {
            let buckets = enabled_buckets(summary);
            let mut selected = 0;
            for (index, (group, bucket)) in buckets.iter().enumerate() {
                self.quota_picker
                    .addItemWithTitle(&NSString::from_str(&format!(
                        "{} — {}",
                        group.display_name, bucket.display_name
                    )));
                if selection_key(group, bucket) == state.settings.menu_bar_quota {
                    selected = index;
                }
            }
            self.quota_picker.selectItemAtIndex(selected as isize);
            self.quota_picker.setEnabled(!buckets.is_empty());
        } else {
            self.quota_picker
                .addItemWithTitle(&NSString::from_str("Waiting for quota data"));
            self.quota_picker.setEnabled(false);
        }

        self.launch_checkbox
            .setState(if launch_agent::is_installed() { 1 } else { 0 });
        self.error.update(state.action_error);
    }
}

impl FooterViews {
    fn quota(mtm: MainThreadMarker, target: &Delegate) -> Self {
        let root = footer_root(mtm);
        let row = footer_row(mtm);
        let button = image_text_button(mtm, "Refresh", "arrow.clockwise", sel!(refresh:), target);
        let spacer = flexible_spacer(mtm, false);
        let freshness = label(
            mtm,
            "",
            &NSFont::systemFontOfSize(10.5),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Right,
        );
        set_width(&freshness, 120.0);
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setTranslatesAutoresizingMaskIntoConstraints(false);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        set_size(&spinner, 16.0, 16.0);
        row.addArrangedSubview(&button);
        row.addArrangedSubview(&spacer);
        row.addArrangedSubview(&freshness);
        row.addArrangedSubview(&spinner);
        root.addArrangedSubview(&separator(mtm));
        root.addArrangedSubview(&row);
        for view in root.arrangedSubviews().iter() {
            equal_width(&view, &root);
        }
        Self {
            root,
            primary_button: button,
            freshness: Some(freshness),
            spinner: Some(spinner),
        }
    }

    fn settings(mtm: MainThreadMarker, target: &Delegate) -> Self {
        let root = footer_root(mtm);
        let row = footer_row(mtm);
        let button = image_text_button(mtm, "Quit Grav Tray", "power", sel!(quit:), target);
        button.setContentTintColor(Some(&NSColor::systemRedColor()));
        let spacer = flexible_spacer(mtm, false);
        let info = icon_button(mtm, "info.circle", sel!(showInfo:), target, 24.0);
        info.setTag(3);
        row.addArrangedSubview(&button);
        row.addArrangedSubview(&spacer);
        row.addArrangedSubview(&info);
        root.addArrangedSubview(&separator(mtm));
        root.addArrangedSubview(&row);
        for view in root.arrangedSubviews().iter() {
            equal_width(&view, &root);
        }
        Self {
            root,
            primary_button: button,
            freshness: None,
            spinner: None,
        }
    }

    fn update_quota(&self, state: &UiState<'_>) {
        self.primary_button.setEnabled(!state.loading);
        if let Some(spinner) = self.spinner.as_ref() {
            spinner.setHidden(!state.loading);
            if state.loading {
                unsafe { spinner.startAnimation(None) };
            } else {
                unsafe { spinner.stopAnimation(None) };
            }
        }
        if let Some(freshness_label) = self.freshness.as_ref() {
            if state.loading {
                freshness_label.setHidden(true);
            } else if let Some(updated) = state.last_updated {
                freshness_label.setStringValue(&NSString::from_str(&freshness(updated)));
                freshness_label.setHidden(false);
            } else {
                freshness_label.setHidden(true);
            }
        }
    }
}

impl ErrorViews {
    fn new(mtm: MainThreadMarker, height: f64) -> Self {
        let root = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Horizontal,
            6.0,
            NSLayoutAttribute::CenterY,
        );
        set_height(&root, height);
        let icon = image_view(mtm, "exclamationmark.triangle.fill");
        icon.setContentTintColor(Some(&NSColor::systemOrangeColor()));
        set_size(&icon, 16.0, 16.0);
        let label = label(
            mtm,
            "",
            &NSFont::systemFontOfSize(10.5),
            &NSColor::systemOrangeColor(),
            NSTextAlignment::Left,
        );
        label.setMaximumNumberOfLines(2);
        label.setUsesSingleLineMode(false);
        label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        label.setPreferredMaxLayoutWidth(
            WIDTH - HORIZONTAL_PADDING * 2.0 - CONTENT_HORIZONTAL_PADDING * 2.0 - 22.0,
        );
        root.addArrangedSubview(&icon);
        root.addArrangedSubview(&label);
        Self { root, label }
    }

    fn update(&self, error: Option<&str>) {
        self.root.setHidden(error.is_none());
        if let Some(error) = error {
            self.label.setStringValue(&NSString::from_str(error));
        }
    }
}

impl InfoViews {
    fn new(mtm: MainThreadMarker) -> Self {
        let root = NSView::new(mtm);
        let stack = make_stack(
            mtm,
            NSUserInterfaceLayoutOrientation::Vertical,
            INFO_GAP,
            NSLayoutAttribute::Leading,
        );
        root.addSubview(&stack);
        pin_edges(&stack, &root, INFO_PADDING, INFO_PADDING);
        let title = label(
            mtm,
            "",
            &NSFont::boldSystemFontOfSize(13.0),
            &NSColor::labelColor(),
            NSTextAlignment::Left,
        );
        let message = label(
            mtm,
            "",
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            NSTextAlignment::Left,
        );
        message.setMaximumNumberOfLines(0);
        message.setUsesSingleLineMode(false);
        message.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        stack.addArrangedSubview(&title);
        stack.addArrangedSubview(&message);
        equal_width(&title, &stack);
        equal_width(&message, &stack);

        let controller = NSViewController::new(mtm);
        controller.setView(&root);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(true);
        popover.setContentViewController(Some(&controller));
        Self {
            popover,
            title,
            message,
        }
    }

    fn show(&self, sender: &NSButton, title: &str, message: &str) {
        if self.popover.isShown() {
            self.popover.close();
        }
        self.title.setStringValue(&NSString::from_str(title));
        self.message.setStringValue(&NSString::from_str(message));

        let natural_width = self
            .title
            .sizeThatFits(NSSize::new(f64::MAX, f64::MAX))
            .width
            .max(
                self.message
                    .sizeThatFits(NSSize::new(f64::MAX, f64::MAX))
                    .width,
            );
        let width = (natural_width + INFO_PADDING * 2.0).clamp(INFO_MIN_WIDTH, INFO_MAX_WIDTH);
        let content_width = width - INFO_PADDING * 2.0;
        self.message.setPreferredMaxLayoutWidth(content_width);
        let title_height = self
            .title
            .sizeThatFits(NSSize::new(content_width, f64::MAX))
            .height
            .ceil();
        let message_height = self
            .message
            .sizeThatFits(NSSize::new(content_width, f64::MAX))
            .height
            .ceil();
        let height = INFO_PADDING * 2.0 + title_height + INFO_GAP + message_height;
        self.popover.setContentSize(NSSize::new(width, height));
        self.popover.showRelativeToRect_ofView_preferredEdge(
            sender.bounds(),
            sender,
            NSRectEdge::MaxX,
        );
    }
}

fn make_empty_view(mtm: MainThreadMarker) -> Retained<NSBox> {
    let root = rounded_box(
        mtm,
        &NSColor::secondaryLabelColor().colorWithAlphaComponent(0.08),
        10.0,
    );
    set_height(&root, 84.0);
    let stack = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Vertical,
        8.0,
        NSLayoutAttribute::Leading,
    );
    root.addSubview(&stack);
    stack
        .leadingAnchor()
        .constraintEqualToAnchor_constant(&root.leadingAnchor(), 12.0)
        .setActive(true);
    stack
        .trailingAnchor()
        .constraintEqualToAnchor_constant(&root.trailingAnchor(), -12.0)
        .setActive(true);
    stack
        .topAnchor()
        .constraintEqualToAnchor_constant(&root.topAnchor(), 12.0)
        .setActive(true);
    stack
        .bottomAnchor()
        .constraintEqualToAnchor_constant(&root.bottomAnchor(), -12.0)
        .setActive(true);

    let heading = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Horizontal,
        6.0,
        NSLayoutAttribute::CenterY,
    );
    let icon = image_view(mtm, "antenna.radiowaves.left.and.right");
    icon.setContentTintColor(Some(&NSColor::labelColor()));
    set_size(&icon, 18.0, 18.0);
    let title = label(
        mtm,
        "Looking for Antigravity",
        &NSFont::boldSystemFontOfSize(12.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    heading.addArrangedSubview(&icon);
    heading.addArrangedSubview(&title);
    let message = label(
        mtm,
        "Open agy and make sure you are signed in. Grav Tray connects directly to its local quota service.",
        &NSFont::systemFontOfSize(11.0),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );
    message.setMaximumNumberOfLines(2);
    message.setUsesSingleLineMode(false);
    message.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    message.setPreferredMaxLayoutWidth(
        WIDTH - HORIZONTAL_PADDING * 2.0 - CONTENT_HORIZONTAL_PADDING * 2.0 - 24.0,
    );
    set_height(&message, 34.0);
    stack.addArrangedSubview(&heading);
    stack.addArrangedSubview(&message);
    equal_width(&heading, &stack);
    equal_width(&message, &stack);
    root
}

fn section_header(
    mtm: MainThreadMarker,
    title: &str,
    tag: isize,
    target: &Delegate,
) -> Retained<NSStackView> {
    let stack = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Horizontal,
        2.0,
        NSLayoutAttribute::CenterY,
    );
    let title = label(
        mtm,
        title,
        &NSFont::systemFontOfSize(12.0),
        &NSColor::secondaryLabelColor(),
        NSTextAlignment::Left,
    );
    let info = icon_button(mtm, "info.circle", sel!(showInfo:), target, 24.0);
    info.setTag(tag);
    let spacer = flexible_spacer(mtm, false);
    stack.addArrangedSubview(&title);
    stack.addArrangedSubview(&info);
    stack.addArrangedSubview(&spacer);
    set_height(&stack, 24.0);
    stack
}

fn settings_row(mtm: MainThreadMarker, title: &str, control: &NSView) -> Retained<NSStackView> {
    let row = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Horizontal,
        8.0,
        NSLayoutAttribute::CenterY,
    );
    let title = label(
        mtm,
        title,
        &NSFont::systemFontOfSize(11.0),
        &NSColor::labelColor(),
        NSTextAlignment::Left,
    );
    let spacer = flexible_spacer(mtm, false);
    row.addArrangedSubview(&title);
    row.addArrangedSubview(&spacer);
    row.addArrangedSubview(control);
    set_height(&row, PICKER_HEIGHT);
    row
}

fn footer_root(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let root = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Vertical,
        0.0,
        NSLayoutAttribute::Leading,
    );
    set_height(&root, FOOTER_HEIGHT);
    root
}

fn footer_row(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let row = make_stack(
        mtm,
        NSUserInterfaceLayoutOrientation::Horizontal,
        0.0,
        NSLayoutAttribute::CenterY,
    );
    set_height(&row, FOOTER_HEIGHT - 1.0);
    row
}

fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let color = NSColor::secondaryLabelColor().colorWithAlphaComponent(0.15);
    let separator = rounded_box(mtm, &color, 0.0);
    set_height(&separator, 1.0);
    separator
}

fn make_stack(
    mtm: MainThreadMarker,
    orientation: NSUserInterfaceLayoutOrientation,
    spacing: f64,
    alignment: NSLayoutAttribute,
) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    stack.setOrientation(orientation);
    stack.setSpacing(spacing);
    stack.setAlignment(alignment);
    stack.setDistribution(NSStackViewDistribution::Fill);
    stack
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    font: &NSFont,
    color: &NSColor,
    alignment: NSTextAlignment,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    label.setFont(Some(font));
    label.setTextColor(Some(color));
    label.setAlignment(alignment);
    label.setMaximumNumberOfLines(1);
    label
}

fn image_view(mtm: MainThreadMarker, symbol: &str) -> Retained<NSImageView> {
    let view = NSImageView::new(mtm);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.setImage(system_image(symbol).as_deref());
    view
}

fn rounded_box(mtm: MainThreadMarker, color: &NSColor, radius: f64) -> Retained<NSBox> {
    let background = NSBox::new(mtm);
    background.setTranslatesAutoresizingMaskIntoConstraints(false);
    background.setBoxType(NSBoxType::Custom);
    background.setTitlePosition(NSTitlePosition::NoTitle);
    background.setBorderWidth(0.0);
    background.setFillColor(color);
    background.setCornerRadius(radius);
    background
}

fn popup_button(mtm: MainThreadMarker, action: Sel, target: &Delegate) -> Retained<NSPopUpButton> {
    let button = NSPopUpButton::new(mtm);
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        button.setTarget(Some(target.as_any_object()));
        button.setAction(Some(action));
    }
    set_height(&button, PICKER_HEIGHT);
    button
}

fn icon_button(
    mtm: MainThreadMarker,
    symbol: &str,
    action: Sel,
    target: &Delegate,
    size: f64,
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
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    button.setBordered(false);
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    set_size(&button, size, size);
    button
}

fn image_text_button(
    mtm: MainThreadMarker,
    title: &str,
    symbol: &str,
    action: Sel,
    target: &Delegate,
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
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
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

fn fixed_spacer(mtm: MainThreadMarker, size: f64, vertical: bool) -> Retained<NSView> {
    let spacer = NSView::new(mtm);
    spacer.setTranslatesAutoresizingMaskIntoConstraints(false);
    if vertical {
        set_height(&spacer, size);
    } else {
        set_width(&spacer, size);
    }
    spacer
}

fn flexible_spacer(mtm: MainThreadMarker, vertical: bool) -> Retained<NSView> {
    let spacer = NSView::new(mtm);
    spacer.setTranslatesAutoresizingMaskIntoConstraints(false);
    spacer.setContentHuggingPriority_forOrientation(
        1.0,
        if vertical {
            NSLayoutConstraintOrientation::Vertical
        } else {
            NSLayoutConstraintOrientation::Horizontal
        },
    );
    spacer
}

fn set_width(view: &NSView, width: f64) {
    view.widthAnchor()
        .constraintEqualToConstant(width)
        .setActive(true);
}

fn set_height(view: &NSView, height: f64) {
    view.heightAnchor()
        .constraintEqualToConstant(height)
        .setActive(true);
}

fn set_size(view: &NSView, width: f64, height: f64) {
    set_width(view, width);
    set_height(view, height);
}

fn equal_width(view: &NSView, parent: &NSView) {
    view.widthAnchor()
        .constraintEqualToAnchor(&parent.widthAnchor())
        .setActive(true);
}

fn pin_edges(view: &NSView, parent: &NSView, horizontal: f64, vertical: f64) {
    view.leadingAnchor()
        .constraintEqualToAnchor_constant(&parent.leadingAnchor(), horizontal)
        .setActive(true);
    view.trailingAnchor()
        .constraintEqualToAnchor_constant(&parent.trailingAnchor(), -horizontal)
        .setActive(true);
    view.topAnchor()
        .constraintEqualToAnchor_constant(&parent.topAnchor(), vertical)
        .setActive(true);
    view.bottomAnchor()
        .constraintEqualToAnchor_constant(&parent.bottomAnchor(), -vertical)
        .setActive(true);
}

fn progress_color(fraction: Option<f64>) -> Retained<NSColor> {
    match fraction {
        None => NSColor::secondaryLabelColor(),
        Some(value) if value <= 0.10 => NSColor::systemRedColor(),
        Some(value) if value <= 0.30 => NSColor::systemOrangeColor(),
        Some(_) => NSColor::systemGreenColor(),
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

pub(super) fn content_height(state: &AppState) -> f64 {
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
        let gaps = cards.len().saturating_sub(1) as f64 * CARD_GAP;
        cards
            .iter()
            .map(|count| CARD_HEADER_HEIGHT + *count as f64 * QUOTA_ROW_HEIGHT)
            .sum::<f64>()
            + gaps
    } else {
        84.0
    };
    let error = if state.quota_error.is_some() || state.action_error.is_some() {
        ERROR_HEIGHT
    } else {
        0.0
    };
    (VERTICAL_PADDING
        + HEADER_HEIGHT
        + PAGE_GAP
        + body
        + PAGE_GAP
        + error
        + FOOTER_HEIGHT
        + VERTICAL_PADDING)
        .max(SETTINGS_MIN_HEIGHT)
}
