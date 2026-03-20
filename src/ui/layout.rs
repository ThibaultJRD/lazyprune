use ratatui::layout::{Constraint, Layout, Rect};

pub struct AppLayout {
    pub tab_bar: Rect,
    pub header: Rect,
    pub list: Rect,
    pub details: Rect,
    pub footer: Rect,
}

pub fn build(area: Rect) -> AppLayout {
    let [tab_bar, header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(area);

    let [list, details] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)]).areas(main);

    AppLayout {
        tab_bar,
        header,
        list,
        details,
        footer,
    }
}
