//! The scale this app draws with: spacing and type.
//!
//! Everything is in `rem`, following the gpui-kit design guide, so type and
//! spacing share one zoom axis.

/// The guide's semantic spacing scale: 2, 4, 8, 12, 16, 24 and 32 px at the
/// default 16 px rem.
pub mod space {
    use gpui_kit::Rems;

    /// Parts of one control: icon and label, title and its description.
    pub const XS: Rems = Rems(0.25);
    /// Closely related controls: a button group, dialog actions.
    pub const SM: Rems = Rems(0.5);
    /// One content group: columns of a row, compact form rows.
    pub const MD: Rems = Rems(0.75);
    /// Separate groups in one section, and region padding.
    pub const LG: Rems = Rems(1.0);
    /// Separate sections.
    pub const XL: Rems = Rems(1.5);
}

/// Type steps. Omarchy's own sizes, so the app reads like its components.
pub mod text {
    use gpui_kit::Rems;

    /// Metadata and secondary lines.
    pub const CAPTION: Rems = Rems(0.6875);
    /// Body text and control labels.
    pub const BODY: Rems = Rems(0.75);
    /// The app name.
    pub const HEADING: Rems = Rems(1.125);
}

/// Region sizes.
pub mod size {
    use gpui_kit::Rems;

    /// The settings dropdown.
    pub const SELECT_W: Rems = Rems(14.0);
    /// The CRF readout between its two buttons.
    pub const CRF_W: Rems = Rems(5.5);
    /// The status lane of a row: "waiting", "42%", "done".
    pub const STATUS_LANE: Rems = Rems(4.5);
    /// The size lane of a row.
    pub const SIZE_LANE: Rems = Rems(5.0);
    /// A row's progress bar.
    pub const BAR_W: Rems = Rems(10.0);
    pub const BAR_H: Rems = Rems(0.3125);
    /// Start and Stop.
    pub const RUN_W: Rems = Rems(8.0);
    /// The empty drop zone.
    pub const DROP_H: Rems = Rems(14.0);
    /// The settings window, and the names down its left side.
    pub const SETTINGS_W: Rems = Rems(40.0);
    pub const LABEL_W: Rems = Rems(4.5);
    /// The Find window and its list of matches.
    pub const FINDER_W: Rems = Rems(42.0);
    pub const FINDER_H: Rems = Rems(20.0);
}
