use ratatui::layout::{Constraint, Layout, Rect};

pub struct AppLayout {
    pub header: Rect,
    pub list: Rect,
    pub details: Rect,
    pub footer: Rect,
}

pub fn build(area: Rect) -> AppLayout {
    let [header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(area);

    let [list, details] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)]).areas(main);

    AppLayout {
        header,
        list,
        details,
        footer,
    }
}
